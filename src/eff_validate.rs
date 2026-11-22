//! Structural checks on an `.eff` before it ships.
//!
//! `effect_library`'s reader is lenient: it follows whatever offsets it is given and fills in
//! what it cannot read, so "it parses" says little about whether the game's loader survives the
//! file. These checks walk the raw bytes the way the loader does and test the invariants it
//! relies on. Every one is calibrated against the 328 game files under `effect/`: none of them
//! trips on anything Nintendo shipped (`every_game_eff_passes`), so a problem reported here is
//! something only a writer could have produced.
//!
//! Where the loader's own code is the evidence, it is the same `nn::vfx` runtime family as Super
//! Mario Odyssey's, whose `Resource::InitializeEmitterSetResource` sizes its emitter array from
//! the ESET's count field and gives each emitter a fixed table of 16 child pointers.

use std::collections::HashSet;

const NONE: u32 = u32::MAX;
/// Child-emitter slots per emitter in the loader's `EmitterResource` (0x80 bytes of pointers,
/// followed by the pointer to the next emitter, which a 17th child would overwrite).
///
/// Top-level emitters have no such limit: the loader links them in a list it allocates from the
/// set's count. No game set has more than 16, but a Banjo set raised to 24 loads and plays.
pub const MAX_CHILDREN: usize = 16;

struct Bytes<'a>(&'a [u8]);

impl Bytes<'_> {
    fn u16(&self, at: usize) -> Option<u16> {
        Some(u16::from_le_bytes(self.0.get(at..at + 2)?.try_into().ok()?))
    }
    fn u32(&self, at: usize) -> Option<u32> {
        Some(u32::from_le_bytes(self.0.get(at..at + 4)?.try_into().ok()?))
    }
    fn u64(&self, at: usize) -> Option<u64> {
        Some(u64::from_le_bytes(self.0.get(at..at + 8)?.try_into().ok()?))
    }
    /// A NUL-terminated string starting at `at`, and the offset just past its terminator.
    fn cstr(&self, at: usize, max: usize) -> Option<(String, usize)> {
        let rest = self.0.get(at..)?;
        let len = rest.iter().take(max).position(|b| *b == 0)?;
        Some((
            String::from_utf8_lossy(&rest[..len]).into_owned(),
            at + len + 1,
        ))
    }
    fn section(&self, at: usize) -> Option<Section> {
        Some(Section {
            magic: self.0.get(at..at + 4)?.try_into().ok()?,
            size: self.u32(at + 4)?,
            child: self.u32(at + 8)?,
            next: self.u32(at + 12)?,
            attr: self.u32(at + 16)?,
            binary: self.u32(at + 20)?,
            children: self.u16(at + 28)?,
        })
    }
}

/// The 32-byte header every PTCL section starts with. Offsets are relative to the section.
struct Section {
    magic: [u8; 4],
    size: u32,
    child: u32,
    next: u32,
    attr: u32,
    binary: u32,
    children: u16,
}

const UNTAGGED_TEXTURES: &str = "the texture table has no GTNT tag";
const UNTAGGED_MODELS: &str =
    "the model table has no G3NT tag, which hangs the game while it loads";

/// Everything wrong with `bytes` as a game `.eff`, in plain sentences. Empty means it passes.
pub fn problems(bytes: &[u8]) -> Vec<String> {
    let mut out = Vec::new();
    check(bytes, &mut out);
    out
}

fn check(bytes: &[u8], out: &mut Vec<String>) {
    let file = Bytes(bytes);
    if bytes.get(0..4) != Some(b"EFFN") {
        out.push("the file does not start with EFFN".into());
        return;
    }
    let (Some(entry_count), Some(model_count), Some(part_count), Some(align)) =
        (file.u16(8), file.u16(10), file.u16(12), file.u16(14))
    else {
        out.push("the EFFN header is cut short".into());
        return;
    };
    let (entry_count, model_count, part_count) = (
        entry_count as usize,
        model_count as usize,
        part_count as usize,
    );
    // kind, set id, model index, first part, part count
    let mut entries = Vec::with_capacity(entry_count);
    for index in 0..entry_count {
        let at = 16 + index * 16;
        match (
            file.u32(at + 4),
            file.u32(at + 8),
            file.u16(at + 12),
            file.u16(at + 14),
        ) {
            (Some(set), Some(model), Some(first), Some(count)) => {
                entries.push((set, model, first, count))
            }
            _ => {
                out.push("the effect table is cut short".into());
                return;
            }
        }
    }
    let parts_at = 16 + entry_count * 16;
    let parts: Vec<u16> = (0..part_count)
        .map(|i| file.u16(parts_at + i * 4 + 2).unwrap_or(u16::MAX))
        .collect();
    let mut cursor = parts_at + part_count * 4 + model_count;
    let mut names = Vec::new();
    for _ in 0..entry_count + model_count + part_count {
        match file.cstr(cursor, 512) {
            Some((name, next)) => {
                names.push(name);
                cursor = next;
            }
            None => {
                out.push("the effect name table runs off the header".into());
                return;
            }
        }
    }
    let mut seen = HashSet::new();
    for name in &names[..entry_count] {
        if !seen.insert(name.to_ascii_lowercase()) {
            out.push(format!("effect '{name}' is listed twice"));
        }
    }

    // A header-only file says so with 0xFFFF and carries no particle data at all.
    let ptcl_at = align as usize * 0x1000;
    if align == 0xFFFF || bytes.len() <= cursor && bytes.len() <= ptcl_at {
        if align != 0xFFFF {
            out.push(format!(
                "the header says particle data starts at {ptcl_at:#x}, but the file ends at {:#x}",
                bytes.len()
            ));
        }
        return;
    }
    if cursor > ptcl_at {
        out.push(format!(
            "the effect tables end at {cursor:#x}, past the particle data at {ptcl_at:#x}"
        ));
    }
    if bytes.get(ptcl_at..ptcl_at + 8) != Some(b"VFXB    ") {
        let actual = bytes.windows(8).position(|w| w == b"VFXB    ");
        out.push(match actual {
            Some(at) => format!(
                "the header says particle data starts at {ptcl_at:#x}, but it starts at {at:#x}"
            ),
            None => "there is no particle data (VFXB) in the file".into(),
        });
        return;
    }

    let ptcl = &bytes[ptcl_at..];
    let sets = check_ptcl(ptcl, out);

    for (index, (set, model, first, count)) in entries.iter().enumerate() {
        let name = &names[index];
        let set_names = &sets.names;
        let empty = |id: usize| id >= 1 && id <= set_names.len() && sets.empty[id - 1];
        if *set as usize > set_names.len() {
            out.push(format!(
                "effect '{name}' plays set {set} of {}",
                set_names.len()
            ));
        } else if empty(*set as usize) {
            out.push(format!(
                "effect '{name}' plays the empty set '{}'",
                set_names[*set as usize - 1]
            ));
        }
        if *model as usize > model_count {
            out.push(format!(
                "effect '{name}' names model {model} of {model_count}"
            ));
        }
        if *count > 0 {
            let start = (*first as usize).wrapping_sub(1);
            if *first == 0 || start + *count as usize > part_count {
                out.push(format!(
                    "effect '{name}' names parts {first}..+{count} of {part_count}"
                ));
                continue;
            }
            for part in &parts[start..start + *count as usize] {
                if *part as usize > set_names.len() {
                    out.push(format!(
                        "a part of '{name}' plays set {part} of {}",
                        set_names.len()
                    ));
                } else if empty(*part as usize) {
                    out.push(format!(
                        "a part of '{name}' plays the empty set '{}'",
                        set_names[*part as usize - 1]
                    ));
                }
            }
        }
    }

    check_references(bytes, &sets, out);
}

/// What the PTCL walk learned that the header and reference checks need.
#[derive(Default)]
struct Sets {
    names: Vec<String>,
    empty: Vec<bool>,
    /// (GUID, name) in descriptor-table order.
    textures: Option<Vec<(u64, String)>>,
    primitives: Option<Vec<u64>>,
}

fn check_ptcl(ptcl: &[u8], out: &mut Vec<String>) -> Sets {
    let data = Bytes(ptcl);
    let mut sets = Sets::default();
    let declared = data.u32(28).unwrap_or(0) as usize;
    if declared != ptcl.len() {
        out.push(format!(
            "the particle data says it is {declared:#x} bytes, {:#x} follow",
            ptcl.len()
        ));
    }
    let mut at = data.u16(22).unwrap_or(0) as usize;
    let mut pool_names: Option<Vec<String>> = None;
    let mut pool_models: Option<usize> = None;
    for _ in 0..64 {
        let Some(section) = data.section(at) else {
            out.push(format!("a section header at {at:#x} is past the end"));
            return sets;
        };
        match &section.magic {
            b"ESTA" => check_sets(&data, at, &section, &mut sets, out),
            b"GRTF" => {
                sets.textures = Some(texture_table(&data, at, &section, out));
                if section.binary != NONE {
                    let start = at + section.binary as usize;
                    if !start.is_multiple_of(4096) {
                        out.push(format!(
                            "the texture pool at {start:#x} is not 4096-aligned"
                        ));
                    }
                    let end = (start + section.size as usize).min(ptcl.len());
                    let pool = ptcl.get(start..end).unwrap_or_default();
                    match bntx_names(pool) {
                        Ok(names) => {
                            pool_names = Some(names);
                            if let Err(err) = bntx_relocations(pool) {
                                out.push(format!("texture pool: {err}"));
                            }
                        }
                        Err(err) => out.push(format!("texture pool: {err}")),
                    }
                }
            }
            b"G3PR" => {
                sets.primitives = Some(primitive_table(&data, at, &section, out));
                if section.binary != NONE {
                    let start = at + section.binary as usize;
                    if !start.is_multiple_of(4096) {
                        out.push(format!("the model pool at {start:#x} is not 4096-aligned"));
                    }
                    let end = (start + section.size as usize).min(ptcl.len());
                    pool_models = bfres_model_count(ptcl.get(start..end).unwrap_or_default());
                }
            }
            b"GRSN" if section.binary != NONE => {
                let start = at + section.binary as usize;
                if !start.is_multiple_of(4096) {
                    out.push(format!("the shader pool at {start:#x} is not 4096-aligned"));
                }
                if ptcl.get(start..start + 4) != Some(b"BNSH") {
                    out.push("the shader pool is not a BNSH".into());
                }
            }
            _ => {}
        }
        if section.next == NONE {
            break;
        }
        at += section.next as usize;
        if at >= ptcl.len() {
            out.push("a section points past the end of the file".into());
            break;
        }
    }

    if let (Some(table), Some(pool)) = (&sets.textures, &pool_names) {
        // The loader binds descriptor i to pool texture i.
        if table.len() != pool.len() {
            out.push(format!(
                "the texture table lists {} textures, the pool holds {}",
                table.len(),
                pool.len()
            ));
        } else if let Some((listed, held)) = table
            .iter()
            .map(|(_, name)| name)
            .zip(pool)
            .find(|(listed, held)| listed != held)
        {
            out.push(format!(
                "the texture table and pool disagree on order ('{listed}' against '{held}')"
            ));
        }
        let mut ids = HashSet::new();
        for (id, name) in table {
            if !ids.insert(*id) {
                out.push(format!("texture id {id:#x} ('{name}') is listed twice"));
            }
        }
    }
    if let (Some(table), Some(models)) = (&sets.primitives, pool_models) {
        if table.len() != models {
            out.push(format!(
                "the model table lists {} models, the pool holds {models}",
                table.len()
            ));
        }
    }
    sets
}

fn check_sets(data: &Bytes, at: usize, esta: &Section, sets: &mut Sets, out: &mut Vec<String>) {
    if esta.child == NONE {
        if esta.children != 0 {
            out.push("the emitter set list has sets but no offset to them".into());
        }
        return;
    }
    let mut set_at = at + esta.child as usize;
    for index in 0..esta.children as usize {
        let Some(set) = data.section(set_at) else {
            out.push(format!("emitter set {index} is past the end"));
            return;
        };
        if &set.magic != b"ESET" {
            out.push(format!("emitter set {index} is not an ESET"));
            return;
        }
        let body = set_at + set.binary as usize;
        let name = data
            .cstr(body + 16, 64)
            .map(|(name, _)| name)
            .unwrap_or_default();
        let declared = data.u32(body + 80).unwrap_or(0) as usize;
        let mut counts = Counts::default();
        match (set.children, set.child) {
            (0, NONE) => {}
            (0, _) => out.push(format!(
                "set '{name}' has no emitters but points at some — the loader reads the next set's \
                 first emitter as its own"
            )),
            (_, NONE) => out.push(format!("set '{name}' has emitters but no offset to them")),
            (count, child) => walk_emitters(
                data,
                set_at + child as usize,
                count as usize,
                0,
                &name,
                &mut counts,
                out,
            ),
        }
        // The loader allocates `declared` emitter slots and fills one per emitter and child.
        if declared != counts.top + counts.children {
            out.push(format!(
                "set '{name}' declares {declared} emitters but holds {} and {} children",
                counts.top, counts.children
            ));
        }
        sets.names.push(name);
        sets.empty.push(set.children == 0);
        if index + 1 < esta.children as usize {
            if set.next == NONE {
                out.push(format!(
                    "the set list ends after {} of {}",
                    index + 1,
                    esta.children
                ));
                return;
            }
            set_at += set.next as usize;
        }
    }
}

#[derive(Default)]
struct Counts {
    top: usize,
    children: usize,
}

fn walk_emitters(
    data: &Bytes,
    start: usize,
    count: usize,
    depth: usize,
    set: &str,
    counts: &mut Counts,
    out: &mut Vec<String>,
) {
    let mut at = start;
    for index in 0..count {
        let Some(emitter) = data.section(at) else {
            out.push(format!("set '{set}': emitter {index} is past the end"));
            return;
        };
        if &emitter.magic != b"EMTR" {
            out.push(format!("set '{set}': emitter {index} is not an EMTR"));
            return;
        }
        match depth {
            0 => counts.top += 1,
            _ => counts.children += 1,
        }
        let body = at + emitter.binary as usize;
        if !body.is_multiple_of(256) {
            out.push(format!(
                "set '{set}': emitter {index}'s data at {body:#x} is not 256-aligned"
            ));
        }
        if at + emitter.size as usize > data.0.len() {
            out.push(format!("set '{set}': emitter {index} runs past the end"));
        }
        if emitter.attr != NONE {
            let mut sub_at = at + emitter.attr as usize;
            for _ in 0..64 {
                let Some(sub) = data.section(sub_at) else {
                    out.push(format!(
                        "set '{set}': a part of emitter {index} is past the end"
                    ));
                    break;
                };
                if sub_at + sub.size as usize > at + emitter.size as usize {
                    out.push(format!(
                        "set '{set}': emitter {index}'s {} block runs past the emitter",
                        String::from_utf8_lossy(&sub.magic)
                    ));
                }
                if sub.next == NONE {
                    break;
                }
                sub_at += sub.next as usize;
            }
        }
        match (emitter.children, emitter.child) {
            (0, NONE) => {}
            (0, _) => out.push(format!(
                "set '{set}': emitter {index} has no children but points at some"
            )),
            (_, NONE) => out.push(format!(
                "set '{set}': emitter {index} has children but no offset to them"
            )),
            (children, child) => {
                if depth > 0 {
                    // The loader walks one level of children; anything deeper never plays and
                    // is not in the set's emitter count.
                    out.push(format!(
                        "set '{set}': a child emitter has children of its own, which the game \
                         never plays"
                    ));
                }
                if children as usize > MAX_CHILDREN {
                    out.push(format!(
                        "set '{set}': emitter {index} has {children} children; the game holds \
                         at most {MAX_CHILDREN}"
                    ));
                }
                walk_emitters(
                    data,
                    at + child as usize,
                    children as usize,
                    depth + 1,
                    set,
                    counts,
                    out,
                );
            }
        }
        if index + 1 < count {
            if emitter.next == NONE {
                out.push(format!(
                    "set '{set}': the emitter list ends after {} of {count}",
                    index + 1
                ));
                return;
            }
            at += emitter.next as usize;
        }
    }
}

fn texture_table(
    data: &Bytes,
    at: usize,
    grtf: &Section,
    out: &mut Vec<String>,
) -> Vec<(u64, String)> {
    let mut list = Vec::new();
    if grtf.child == NONE {
        return list;
    }
    let table_at = at + grtf.child as usize;
    let Some(table) = data.section(table_at) else {
        return list;
    };
    if &table.magic != b"GTNT" {
        out.push(UNTAGGED_TEXTURES.into());
    }
    if table.binary == NONE {
        return list;
    }
    let mut entry = table_at + table.binary as usize;
    let end = entry + table.size as usize;
    while let Some(id) = data.u64(entry) {
        let next = data.u32(entry + 8).unwrap_or(0) as usize;
        let len = data.u32(entry + 12).unwrap_or(0) as usize;
        let name = data
            .cstr(entry + 16, len.max(1))
            .map(|(name, _)| name)
            .unwrap_or_default();
        list.push((id, name));
        if next == 0 {
            break;
        }
        entry += next;
        if entry >= end {
            out.push("the texture table's last entry points past the table".into());
            break;
        }
    }
    list
}

fn primitive_table(data: &Bytes, at: usize, g3pr: &Section, out: &mut Vec<String>) -> Vec<u64> {
    let mut list = Vec::new();
    if g3pr.child == NONE {
        return list;
    }
    let table_at = at + g3pr.child as usize;
    let Some(table) = data.section(table_at) else {
        return list;
    };
    // An untagged model table hangs the stage load (The Sand Bird, bisected in game).
    if &table.magic != b"G3NT" {
        out.push(UNTAGGED_MODELS.into());
    }
    if table.binary == NONE {
        return list;
    }
    let mut entry = table_at + table.binary as usize;
    let end = entry + table.size as usize;
    while entry + 24 <= end {
        let Some(id) = data.u64(entry) else { break };
        list.push(id);
        match data.u32(entry + 8) {
            Some(0) | None => break,
            Some(next) => entry += next as usize,
        }
    }
    list
}

/// Texture names of a BNTX, in pool order.
fn bntx_names(pool: &[u8]) -> Result<Vec<String>, String> {
    let data = Bytes(pool);
    if pool.get(0..4) != Some(b"BNTX") {
        return Err("not a BNTX".into());
    }
    let size = data.u32(0x1C).ok_or("cut short")? as usize;
    if size > pool.len() {
        return Err(format!(
            "says it is {size:#x} bytes, holds {:#x}",
            pool.len()
        ));
    }
    let count = data.u32(0x24).ok_or("cut short")? as usize;
    let table = data.u64(0x28).ok_or("cut short")? as usize;
    let mut names = Vec::with_capacity(count);
    for index in 0..count {
        let brti = data
            .u64(table + index * 8)
            .ok_or("texture table cut short")? as usize;
        if pool.get(brti..brti + 4) != Some(b"BRTI") {
            return Err(format!("texture {index} is not a BRTI block"));
        }
        let name_at = data.u64(brti + 0x60).ok_or("texture name cut short")? as usize;
        let len = data.u16(name_at).ok_or("texture name cut short")? as usize;
        let name = pool
            .get(name_at + 2..name_at + 2 + len)
            .ok_or("texture name cut short")?;
        names.push(String::from_utf8_lossy(name).into_owned());
    }
    Ok(names)
}

/// The pointer slots a BNTX's relocation table (`_RLT`) lists.
///
/// The game loads a texture pool by adding its base address to every slot listed here, so a
/// pointer the table misses stays a raw offset and a listed slot that is not a pointer is
/// corrupted. Entries name `structs` runs of `offsets` consecutive pointers, `padding` words
/// apart (nn::util::RelocationTable).
fn relocated_slots(pool: &[u8]) -> Result<HashSet<usize>, String> {
    let data = Bytes(pool);
    let table = data.u32(0x18).ok_or("cut short")? as usize;
    if pool.get(table..table + 4) != Some(b"_RLT") {
        return Err("has no relocation table".into());
    }
    let sections = data.u32(table + 8).ok_or("relocation table cut short")? as usize;
    let entries = table + 0x10 + sections * 0x18;
    let mut slots = HashSet::new();
    for section in 0..sections {
        let at = table + 0x10 + section * 0x18;
        let first = data.u32(at + 0x10).ok_or("relocation table cut short")? as usize;
        let count = data.u32(at + 0x14).ok_or("relocation table cut short")? as usize;
        for entry in first..first + count {
            let at = entries + entry * 8;
            let position = data.u32(at).ok_or("relocation entry cut short")? as usize;
            let structs = data.u16(at + 4).ok_or("relocation entry cut short")? as usize;
            let offsets = *pool.get(at + 6).ok_or("relocation entry cut short")? as usize;
            let padding = *pool.get(at + 7).ok_or("relocation entry cut short")? as usize;
            for run in 0..structs {
                for word in 0..offsets {
                    slots.insert(position + (run * (offsets + padding) + word) * 8);
                }
            }
        }
    }
    Ok(slots)
}

/// Every pointer the BNTX's structure holds is relocated, and every relocated slot points inside
/// the pool.
fn bntx_relocations(pool: &[u8]) -> Result<(), String> {
    let data = Bytes(pool);
    let slots = relocated_slots(pool)?;
    if let Some(slot) = slots
        .iter()
        .find(|slot| data.u64(**slot).is_none_or(|ptr| ptr as usize > pool.len()))
    {
        return Err(format!(
            "relocated slot {slot:#x} does not hold a pointer into the pool"
        ));
    }
    // texture array, data block, dictionary and memory pool of the NX header
    let mut pointers = vec![0x28usize, 0x30, 0x38, 0x40];
    let count = data.u32(0x24).ok_or("cut short")? as usize;
    let table = data.u64(0x28).ok_or("cut short")? as usize;
    for index in 0..count {
        pointers.push(table + index * 8);
        let brti = data
            .u64(table + index * 8)
            .ok_or("texture table cut short")? as usize;
        // name, parent, mip table, user data, and the four runtime slots after them
        pointers.extend((0x60..=0x98).step_by(8).map(|field| brti + field));
        let mips = data.u16(brti + 0x16).ok_or("texture header cut short")? as usize;
        if let Some(mip_table) = data.u64(brti + 0x70).filter(|at| *at != 0) {
            pointers.extend((0..mips).map(|level| mip_table as usize + level * 8));
        }
    }
    match pointers
        .into_iter()
        .find(|slot| !slots.contains(slot) && data.u64(*slot).is_some_and(|ptr| ptr != 0))
    {
        Some(slot) => Err(format!(
            "pointer at {slot:#x} is missing from the relocation table"
        )),
        None => Ok(()),
    }
}

/// Model count of a BFRES, from its model dictionary.
fn bfres_model_count(pool: &[u8]) -> Option<usize> {
    if pool.get(0..4) != Some(b"FRES") {
        return None;
    }
    let data = Bytes(pool);
    match data.u64(0x30)? as usize {
        0 => Some(0),
        dict => Some(data.u32(dict + 4)? as usize),
    }
}

/// Every emitter's texture, model and shader references resolve against the file's own pools.
fn check_references(bytes: &[u8], sets: &Sets, out: &mut Vec<String>) {
    let file = match effect_library::NamcoEffectFile::load(bytes) {
        Ok(file) => file,
        Err(err) => {
            out.push(format!("effect_library cannot read the file: {err}"));
            return;
        }
    };
    let Some(ptcl) = file.ptcl_file.as_ref() else {
        out.push("effect_library cannot read the particle data".into());
        return;
    };
    if ptcl.emitter_list.emitter_sets.len() != sets.names.len() {
        out.push(format!(
            "effect_library reads {} sets where the list holds {}",
            ptcl.emitter_list.emitter_sets.len(),
            sets.names.len()
        ));
    }
    let textures: HashSet<u64> = sets.textures.iter().flatten().map(|(id, _)| *id).collect();
    let models: HashSet<u64> = sets.primitives.iter().flatten().copied().collect();
    let variations = |binary: Option<&Vec<u8>>| {
        binary
            .and_then(|b| effect_library::bnsh::BnshFile::read(b).ok())
            .map(|b| b.variations.len())
    };
    let shader = ptcl.shader_info.as_ref();
    let standard = variations(shader.and_then(|s| s.binary_data.as_ref()));
    let compute = variations(shader.and_then(|s| s.compute_binary.as_ref())).unwrap_or(0);
    for set in &ptcl.emitter_list.emitter_sets {
        let mut stack: Vec<&effect_library::structs::Emitter> = set.emitters.iter().collect();
        while let Some(emitter) = stack.pop() {
            stack.extend(emitter.children.iter());
            let d = &emitter.data;
            let who = || format!("{}/{}", set.name, d.display_name());
            for (slot, sampler) in [
                &d.sampler0,
                &d.sampler1,
                &d.sampler2,
                &d.sampler3,
                &d.sampler4,
                &d.sampler5,
            ]
            .into_iter()
            .enumerate()
            {
                if let Some(sampler) = sampler {
                    if sampler.texture_id != u64::MAX && !textures.contains(&sampler.texture_id) {
                        out.push(format!(
                            "{}: texture slot {slot} names {:#x}, which the pool does not hold",
                            who(),
                            sampler.texture_id
                        ));
                    }
                }
            }
            // Only the particle model: the game's own files leave dangling ids in the extra and
            // volume model slots (193 of them), so those are not something the loader resolves.
            let model = d.particle_data.primitive_id;
            if model != u64::MAX && model != 0 && !models.contains(&model) {
                out.push(format!(
                    "{}: model {model:#x} is not in the model pool",
                    who()
                ));
            }
            let s = &d.emitter_static;
            for (table, keys) in [
                ("colour 0", s.num_color0_keys),
                ("alpha 0", s.num_alpha0_keys),
                ("colour 1", s.num_color1_keys),
                ("alpha 1", s.num_alpha1_keys),
                ("scale", s.num_scale_keys),
                ("shader coefficient", s.num_param_keys),
            ] {
                if keys > crate::eff_attrs::KEY_SLOTS {
                    out.push(format!(
                        "{}: {keys} {table} keys in a table of {}",
                        who(),
                        crate::eff_attrs::KEY_SLOTS
                    ));
                }
            }
            let refs = &d.shader_references;
            if let Some(count) = standard {
                for index in [
                    refs.shader_index,
                    refs.user_shader_index1,
                    refs.user_shader_index2,
                ] {
                    if index >= 0 && index as usize >= count {
                        out.push(format!("{}: shader {index} of {count}", who()));
                    }
                }
            }
            if refs.compute_shader_index >= 0 && refs.compute_shader_index as usize >= compute {
                out.push(format!(
                    "{}: compute shader {} of {compute}",
                    who(),
                    refs.compute_shader_index
                ));
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::problems;
    use crate::mod_project::{
        AuthoredEdit, EffMod, EmitterFieldEdits, EmitterRoster, EmitterSlot, EntryEdit,
        TextureAddition, TextureImport, TransplantOp, VariantEdit,
    };
    use std::path::{Path, PathBuf};

    /// The extracted game dump: `VISIONARY_EFF_ROOT` holds `effect/`.
    fn dump_root() -> Option<PathBuf> {
        let root = std::env::var_os("VISIONARY_EFF_ROOT").map(PathBuf::from);
        if root.is_none() {
            eprintln!("skipped: set VISIONARY_EFF_ROOT to the folder holding the effect/ tree");
        }
        root
    }

    /// Every game `.eff` under `effect/`, as paths relative to the dump root. Names starting
    /// with `_` are this tool's own outputs, not game data.
    fn game_effs(root: &Path) -> Vec<String> {
        fn walk(dir: &Path, root: &Path, out: &mut Vec<String>) {
            let Ok(entries) = std::fs::read_dir(dir) else {
                return;
            };
            for entry in entries.flatten() {
                let path = entry.path();
                if path.is_dir() {
                    walk(&path, root, out);
                } else if path.extension().is_some_and(|e| e == "eff")
                    && !path
                        .file_name()
                        .is_some_and(|n| n.to_string_lossy().starts_with('_'))
                {
                    let rel = path.strip_prefix(root).unwrap_or(&path);
                    out.push(rel.to_string_lossy().replace('\\', "/"));
                }
            }
        }
        let mut out = Vec::new();
        walk(&root.join("effect"), root, &mut out);
        out.sort();
        out
    }

    fn names_of(set: &effect_library::structs::EmitterSet) -> Vec<(String, u8)> {
        fn walk(es: &[effect_library::structs::Emitter], depth: u8, out: &mut Vec<(String, u8)>) {
            for e in es {
                out.push((e.data.display_name(), depth));
                walk(&e.children, depth + 1, out);
            }
        }
        let mut out = Vec::new();
        walk(&set.emitters, 0, &mut out);
        out
    }

    /// One edit of each kind the editor offers, aimed at `file`'s biggest effect.
    fn edits_for(
        rel: &str,
        file: &effect_library::NamcoEffectFile,
        png: &Path,
    ) -> Vec<(&'static str, EffMod)> {
        let base = || EffMod {
            source_rel: rel.to_string(),
            ..Default::default()
        };
        let mut out = vec![("unchanged", base())];
        let Some(ptcl) = file.ptcl_file.as_ref() else {
            return out;
        };
        let sets = &ptcl.emitter_list.emitter_sets;
        let Some((set_idx, set)) = sets
            .iter()
            .enumerate()
            .filter(|(i, _)| {
                file.entries
                    .iter()
                    .any(|e| e.emitter_set_id as usize == i + 1)
            })
            .max_by_key(|(_, s)| s.emitters.len())
        else {
            return out;
        };
        let entry_idx = file
            .entries
            .iter()
            .position(|e| e.emitter_set_id as usize == set_idx + 1)
            .unwrap();
        let entry = file.entry_names[entry_idx].clone();
        let names = names_of(set);
        let first = names[0].0.clone();
        let textures: Vec<String> = ptcl
            .texture_info
            .as_ref()
            .map(|t| t.descriptors.iter().map(|d| d.name.clone()).collect())
            .unwrap_or_default();
        let edit = |fields: EmitterFieldEdits| AuthoredEdit {
            set_name: set.name.clone(),
            entry_name: entry.clone(),
            set_idx,
            emitter_name: first.clone(),
            emitter_idx: 0,
            fields,
        };

        let mut fields = EmitterFieldEdits {
            color0: Some(vec![[1.0, 0.2, 0.1, 0.0]]),
            alpha0: Some(vec![[0.5, 0.0]]),
            scale: Some(3.0),
            emission_rate: Some(4.0),
            lifetime: Some(40.0),
            emitter_scale: Some([2.0, 2.0, 2.0]),
            color_scale: Some(2.0),
            ..Default::default()
        };
        fields.attrs.insert(
            "emission.rate".into(),
            crate::eff_attrs::AttrValue::Float(6.0),
        );
        out.push((
            "fields",
            EffMod {
                authored: vec![edit(fields)],
                ..base()
            },
        ));
        if let Some(other) = textures.last() {
            out.push((
                "texture swap",
                EffMod {
                    authored: vec![edit(EmitterFieldEdits {
                        texture_name: Some(other.clone()),
                        ..Default::default()
                    })],
                    ..base()
                },
            ));
        }

        let slot = |i: usize, name: &str, depth: u8| EmitterSlot {
            source_idx: i,
            source_name: names[i].0.clone(),
            name: name.to_string(),
            depth,
        };
        let same: Vec<EmitterSlot> = names
            .iter()
            .enumerate()
            .map(|(i, (n, d))| slot(i, n, *d))
            .collect();
        let roster = |slots: Vec<EmitterSlot>| EmitterRoster {
            set_name: set.name.clone(),
            entry_name: entry.clone(),
            set_idx,
            slots,
        };
        let mut copied = same.clone();
        copied.push(slot(0, &format!("{first}_copy"), 0));
        out.push((
            "copy an emitter",
            EffMod {
                rosters: vec![roster(copied)],
                ..base()
            },
        ));
        if same.len() > 1 {
            out.push((
                "remove an emitter",
                EffMod {
                    rosters: vec![roster(same[..same.len() - 1].to_vec())],
                    ..base()
                },
            ));
            let mut nested = same.clone();
            if nested[1].depth == 0 && nested[0].depth == 0 {
                nested[1].depth = 1;
                out.push((
                    "nest an emitter",
                    EffMod {
                        rosters: vec![roster(nested)],
                        ..base()
                    },
                ));
            }
        }
        out.push((
            "remove every emitter",
            EffMod {
                rosters: vec![roster(Vec::new())],
                ..base()
            },
        ));

        if let Some(other) = sets
            .iter()
            .find(|s| s.name != set.name && !s.emitters.is_empty())
        {
            out.push((
                "spawn edit",
                EffMod {
                    entry_edits: vec![EntryEdit {
                        entry_name: entry.clone(),
                        emitter_set: Some(other.name.clone()),
                        variants: Some(vec![VariantEdit {
                            start_frame: 5,
                            set_name: set.name.clone(),
                            bone: "top".into(),
                        }]),
                        model: None,
                    }],
                    ..base()
                },
            ));
        }
        out.push((
            "same-file copy",
            EffMod {
                transplants: vec![TransplantOp {
                    new_entry_name: format!("{entry}_vcopy"),
                    src_file_rel: rel.to_string(),
                    src_set_name: entry.clone(),
                    src_set_idx: set_idx,
                    one_slot_slots: Vec::new(),
                    replace_entry: None,
                }],
                ..base()
            },
        ));
        const DONOR: &str = "effect/fighter/pickel/ef_pickel.eff";
        if rel != DONOR {
            out.push((
                "borrowed effect",
                EffMod {
                    transplants: vec![TransplantOp {
                        new_entry_name: "pickel_tnt".into(),
                        src_file_rel: DONOR.into(),
                        src_set_name: "pickel_tnt".into(),
                        src_set_idx: 0,
                        one_slot_slots: Vec::new(),
                        replace_entry: None,
                    }],
                    ..base()
                },
            ));
            out.push((
                "borrowed over an effect",
                EffMod {
                    transplants: vec![TransplantOp {
                        new_entry_name: entry.clone(),
                        src_file_rel: DONOR.into(),
                        src_set_name: "pickel_tnt".into(),
                        src_set_idx: 0,
                        one_slot_slots: Vec::new(),
                        replace_entry: Some(entry.clone()),
                    }],
                    ..base()
                },
            ));
        }
        let png = match ptcl
            .texture_info
            .as_ref()
            .and_then(|t| t.binary_data.as_ref())
            .and_then(|pool| crate::texture_import::describe(pool, 0, "").ok())
        {
            Some(desc) => {
                let path = png.join(format!("import_{}x{}.png", desc.width, desc.height));
                if !path.exists() {
                    image::RgbaImage::from_fn(desc.width, desc.height, |x, y| {
                        image::Rgba([(x * 7) as u8, (y * 5) as u8, 128, 255])
                    })
                    .save(&path)
                    .expect("write a test png");
                }
                path
            }
            None => png.join("none.png"),
        };
        if let Some(first_texture) = textures.first() {
            out.push((
                "texture import",
                EffMod {
                    textures: vec![TextureImport {
                        texture_name: first_texture.clone(),
                        png_path: png.to_string_lossy().into_owned(),
                        raw: false,
                    }],
                    ..base()
                },
            ));
            out.push((
                "texture added and used",
                EffMod {
                    textures_added: vec![TextureAddition {
                        texture_name: "visionary_new00".into(),
                        template_name: first_texture.clone(),
                        png_path: png.to_string_lossy().into_owned(),
                        raw: false,
                    }],
                    authored: vec![edit(EmitterFieldEdits {
                        texture_name: Some("visionary_new00".into()),
                        ..Default::default()
                    })],
                    ..base()
                },
            ));
        }
        out
    }

    #[test]
    fn every_edit_kind_writes_a_file_the_game_can_load() {
        let Some(root) = dump_root() else {
            return;
        };
        let png = std::env::temp_dir().join("visionary_validate_pngs");
        std::fs::create_dir_all(&png).expect("png folder");
        let only = std::env::var("VISIONARY_SWEEP_ONLY").ok();
        let dump = std::env::var_os("VISIONARY_SWEEP_OUT").map(PathBuf::from);
        let mut failures = Vec::new();
        let mut errors = Vec::new();
        let mut checked = 0usize;
        for rel in game_effs(&root) {
            if only.as_deref().is_some_and(|only| !rel.contains(only)) {
                continue;
            }
            let bytes = std::fs::read(root.join(&rel)).expect("read game eff");
            let Ok(file) = effect_library::NamcoEffectFile::load(&bytes) else {
                continue;
            };
            // Header-only files (Joker's cut-in) have no particle data to edit.
            if file.ptcl_file.is_none() {
                continue;
            }
            for (kind, eff) in edits_for(&rel, &file, &png) {
                checked += 1;
                match crate::eff_export::rebuild_eff_bytes(&bytes, &eff, Some(&root)) {
                    Ok(out) => {
                        if kind == "unchanged" && out != bytes {
                            failures.push(format!("{rel} [{kind}]: not byte-identical"));
                        }
                        let found = problems(&out);
                        if let Some(dir) = &dump {
                            let stem = Path::new(&rel).file_stem().unwrap().to_string_lossy();
                            let name = format!("{stem}__{}.eff", kind.replace(' ', "_"));
                            std::fs::create_dir_all(dir).unwrap();
                            std::fs::write(dir.join(name), &out).unwrap();
                        }
                        if !found.is_empty() {
                            failures.push(format!("{rel} [{kind}]: {}", found.join("; ")));
                        }
                    }
                    Err(err) => errors.push(format!("{rel} [{kind}]: {err:#}")),
                }
            }
        }
        eprintln!("checked {checked} writes");
        for e in &errors {
            eprintln!("refused: {e}");
        }
        for f in &failures {
            eprintln!("INVALID: {f}");
        }
        assert!(
            failures.is_empty() && errors.is_empty(),
            "{} written files break the game's invariants, {} edits were refused",
            failures.len(),
            errors.len()
        );
    }

    #[test]
    fn every_game_eff_passes() {
        let Some(root) = dump_root() else {
            return;
        };
        let mut failures = Vec::new();
        let files = game_effs(&root);
        for rel in &files {
            let bytes = std::fs::read(root.join(rel)).expect("read game eff");
            let found = problems(&bytes);
            if !found.is_empty() {
                failures.push(format!("{rel}: {}", found.join("; ")));
            }
        }
        assert_eq!(files.len(), 328, "the dump should hold every game eff");
        assert!(failures.is_empty(), "{failures:#?}");
    }

    fn game(root: &Path, rel: &str) -> Vec<u8> {
        std::fs::read(root.join(rel)).expect("read game eff")
    }

    const BUDDY: &str = "effect/fighter/buddy/ef_buddy.eff";
    /// Banjo & Kazooie's Final Smash ground effect, the one with the sand: 14 emitters.
    const SAND_SET: &str = "P_BuddyFinalAppearGround";

    /// A roster for Banjo's sand effect that keeps every emitter and adds `copies` of `sand1`.
    fn sand_copies(file: &effect_library::NamcoEffectFile, copies: usize) -> EffMod {
        let sets = &file.ptcl_file.as_ref().unwrap().emitter_list.emitter_sets;
        let set_idx = sets
            .iter()
            .position(|s| s.name == SAND_SET)
            .expect("sand set");
        let names = names_of(&sets[set_idx]);
        let sand = names.iter().position(|(n, _)| n == "sand1").expect("sand1");
        let slot = |i: usize, name: String| EmitterSlot {
            source_idx: i,
            source_name: names[i].0.clone(),
            name,
            depth: names[i].1,
        };
        let mut slots: Vec<EmitterSlot> = (0..names.len())
            .map(|i| slot(i, names[i].0.clone()))
            .collect();
        for n in 0..copies {
            slots.push(slot(sand, format!("sand1_copy{n}")));
        }
        EffMod {
            source_rel: BUDDY.into(),
            rosters: vec![EmitterRoster {
                set_name: SAND_SET.into(),
                entry_name: "BUDDY_FINAL_APPEAR_GROUND".into(),
                set_idx,
                slots,
            }],
            ..Default::default()
        }
    }

    /// The loader gives each emitter 16 child slots, with the pointer to the next emitter right
    /// after them. Hanging 16 copies of Banjo's sand emitter off another exports; a 17th is
    /// refused instead of being written over that pointer.
    #[test]
    fn a_seventeenth_child_is_refused() {
        let Some(root) = dump_root() else {
            return;
        };
        let bytes = game(&root, BUDDY);
        let file = effect_library::NamcoEffectFile::load(&bytes).unwrap();
        let with_children = |count: usize| {
            let mut eff = sand_copies(&file, 0);
            let slots = &mut eff.rosters[0].slots;
            let sand = slots.iter().position(|s| s.name == "sand1").unwrap();
            let child = EmitterSlot {
                depth: 1,
                ..slots[sand].clone()
            };
            for n in 0..count {
                slots.insert(
                    1,
                    EmitterSlot {
                        name: format!("sand1_child{n}"),
                        ..child.clone()
                    },
                );
            }
            eff
        };
        let ok = crate::eff_export::rebuild_eff_bytes(&bytes, &with_children(16), Some(&root))
            .expect("16 children export");
        assert!(problems(&ok).is_empty());
        let refused = crate::eff_export::rebuild_eff_bytes(&bytes, &with_children(17), Some(&root))
            .expect_err("17 children must not export");
        assert!(
            format!("{refused:#}").contains("has 17 children"),
            "{refused:#}"
        );
    }

    /// Parts on their own have no such limit: Banjo's Wonderwing start effect raised to 24
    /// emitters loads and plays in game. Copying the sand emitter past 16 exports.
    #[test]
    fn more_than_sixteen_emitters_export() {
        let Some(root) = dump_root() else {
            return;
        };
        let bytes = game(&root, BUDDY);
        let file = effect_library::NamcoEffectFile::load(&bytes).unwrap();
        let out = crate::eff_export::rebuild_eff_bytes(&bytes, &sand_copies(&file, 6), Some(&root))
            .expect("20 emitters export");
        assert!(problems(&out).is_empty());
    }

    /// An export with no edits is the game's file, byte for byte; `effect_library`'s own writer
    /// re-encodes every pool and is not.
    #[test]
    fn an_unedited_export_is_the_game_file() {
        let Some(root) = dump_root() else {
            return;
        };
        let bytes = game(&root, BUDDY);
        let raw = effect_library::NamcoEffectFile::load(&bytes)
            .unwrap()
            .save()
            .unwrap();
        assert_ne!(raw, bytes, "the library's own writer re-encodes the pools");
        let eff = EffMod {
            source_rel: BUDDY.into(),
            ..Default::default()
        };
        let ours = crate::eff_export::rebuild_eff_bytes(&bytes, &eff, Some(&root)).unwrap();
        assert!(ours == bytes, "an export with no edits changed the file");
    }

    /// Sections of a written file as (magic, bytes), with the region each one spans.
    fn sections_of(bytes: &[u8]) -> Vec<([u8; 4], Vec<u8>)> {
        let pages = u16::from_le_bytes(bytes[14..16].try_into().unwrap()) as usize;
        let ptcl = &bytes[pages * 0x1000..];
        let mut at = u16::from_le_bytes(ptcl[22..24].try_into().unwrap()) as usize;
        let mut out = Vec::new();
        loop {
            let next = u32::from_le_bytes(ptcl[at + 12..at + 16].try_into().unwrap());
            let end = if next == u32::MAX {
                ptcl.len()
            } else {
                at + next as usize
            };
            let mut region = ptcl[at..end].to_vec();
            region[12..16].fill(0); // where the next section starts depends on padding
            out.push((ptcl[at..at + 4].try_into().unwrap(), region));
            if next == u32::MAX {
                return out;
            }
            at = end;
        }
    }

    /// Recolouring one emitter changes the emitter list and nothing else: the texture, model and
    /// shader pools ship exactly as the game has them.
    #[test]
    fn a_colour_edit_leaves_the_pools_as_the_game_shipped_them() {
        let Some(root) = dump_root() else {
            return;
        };
        let bytes = game(&root, BUDDY);
        let file = effect_library::NamcoEffectFile::load(&bytes).unwrap();
        let sets = &file.ptcl_file.as_ref().unwrap().emitter_list.emitter_sets;
        let set_idx = sets.iter().position(|s| s.name == SAND_SET).unwrap();
        let eff = EffMod {
            source_rel: BUDDY.into(),
            authored: vec![AuthoredEdit {
                set_name: SAND_SET.into(),
                entry_name: "BUDDY_FINAL_APPEAR_GROUND".into(),
                set_idx,
                emitter_name: names_of(&sets[set_idx])[7].0.clone(),
                emitter_idx: 7,
                fields: EmitterFieldEdits {
                    color0: Some(vec![[0.1, 0.4, 1.0, 0.0]]),
                    ..Default::default()
                },
            }],
            ..Default::default()
        };
        let out = crate::eff_export::rebuild_eff_bytes(&bytes, &eff, Some(&root)).unwrap();
        let (before, after) = (sections_of(&bytes), sections_of(&out));
        assert_eq!(before.len(), after.len());
        for ((magic, a), (_, b)) in before.iter().zip(&after) {
            let name = String::from_utf8_lossy(magic);
            if magic == b"ESTA" {
                assert!(a != b, "the edit must reach the emitter list");
            } else {
                assert!(
                    a == b,
                    "{name} changed, but the edit only recoloured an emitter"
                );
            }
        }
    }

    /// Copying a multi-part effect within its own file copies the parts' bone names with them.
    /// Mario's Final Smash bullets are four parts on four bones.
    #[test]
    fn a_copied_multi_part_effect_keeps_its_bones() {
        let Some(root) = dump_root() else {
            return;
        };
        const MARIO: &str = "effect/fighter/mario/ef_mario.eff";
        let bytes = game(&root, MARIO);
        let eff = EffMod {
            source_rel: MARIO.into(),
            transplants: vec![TransplantOp {
                new_entry_name: "mario_final_bullet_copy".into(),
                src_file_rel: String::new(),
                src_set_name: "MARIO_FINAL_BULLET".into(),
                src_set_idx: 0,
                one_slot_slots: Vec::new(),
                replace_entry: None,
            }],
            ..Default::default()
        };
        let out = crate::eff_export::rebuild_eff_bytes(&bytes, &eff, Some(&root)).unwrap();
        let file = effect_library::NamcoEffectFile::load(&out).unwrap();
        let bones = |name: &str| {
            let at = file.entry_names.iter().position(|n| n == name).unwrap();
            let entry = &file.entries[at];
            let start = entry.variant_start_idx as usize - 1;
            file.external_bone_names[start..start + entry.variant_count as usize].to_vec()
        };
        let original = bones("MARIO_FINAL_BULLET");
        assert_eq!(original, ["bulletA1", "bulletA3", "bulletB1", "bulletB3"]);
        assert_eq!(bones("mario_final_bullet_copy"), original);
        assert!(problems(&out).is_empty());
    }

    /// Borrowing enough effects into a big file pushes its header past two pages. The header
    /// must then say where the particle data really is.
    #[test]
    fn a_header_past_two_pages_names_where_the_particles_start() {
        let Some(root) = dump_root() else {
            return;
        };
        const ITEM: &str = "effect/system/item/ef_item.eff";
        let bytes = game(&root, ITEM);
        let file = effect_library::NamcoEffectFile::load(&bytes).unwrap();
        let donor = file
            .entries
            .iter()
            .position(|e| e.variant_count == 0 && e.emitter_set_id > 0)
            .map(|i| file.entry_names[i].clone())
            .unwrap();
        let copies = |n: usize| EffMod {
            source_rel: ITEM.into(),
            transplants: (0..n)
                .map(|i| TransplantOp {
                    new_entry_name: format!("visionary_borrowed_item_effect_number_{i:03}"),
                    src_file_rel: String::new(),
                    src_set_name: donor.clone(),
                    src_set_idx: 0,
                    one_slot_slots: Vec::new(),
                    replace_entry: None,
                })
                .collect(),
            ..Default::default()
        };
        let page = |bytes: &[u8]| u16::from_le_bytes(bytes[14..16].try_into().unwrap());
        // Few copies: still two pages, as the game has it.
        let small = crate::eff_export::rebuild_eff_bytes(&bytes, &copies(2), Some(&root)).unwrap();
        assert_eq!(page(&small), 2);
        assert!(problems(&small).is_empty());
        // Enough copies to need a third page.
        let big = crate::eff_export::rebuild_eff_bytes(&bytes, &copies(40), Some(&root)).unwrap();
        assert_eq!(page(&big), 3, "the tables now need a third page");
        assert_eq!(
            &big[0x3000..0x3008],
            b"VFXB    ",
            "and the particle data is on it"
        );
        // The library's own writer puts it somewhere else and says page 2.
        let mut raw = effect_library::NamcoEffectFile::load(&bytes).unwrap();
        for i in 0..40 {
            raw.entries.push(raw.entries[0].clone());
            raw.entry_names
                .push(format!("visionary_borrowed_item_effect_number_{i:03}"));
        }
        let raw = raw.save().unwrap();
        assert_eq!(page(&raw), 2);
        assert!(
            problems(&raw)
                .iter()
                .any(|p| p.contains("particle data starts")),
            "{:?}",
            problems(&raw)
        );
        assert!(problems(&big).is_empty(), "{:?}", problems(&big));
        let reread = effect_library::NamcoEffectFile::load(&big).unwrap();
        assert!(reread.ptcl_file.is_some(), "the particle data reads back");
    }

    /// A header-only file (Joker's cut-in) has no particle data; an export keeps it that way.
    #[test]
    fn a_header_only_file_stays_header_only() {
        let Some(root) = dump_root() else {
            return;
        };
        let rel = game_effs(&root)
            .into_iter()
            .find(|rel| rel.ends_with("ef_jack_cutin.eff"))
            .expect("Joker's cut-in eff");
        let bytes = game(&root, &rel);
        assert_eq!(
            u16::from_le_bytes(bytes[14..16].try_into().unwrap()),
            0xFFFF
        );
        let raw = effect_library::NamcoEffectFile::load(&bytes)
            .unwrap()
            .save()
            .unwrap();
        assert!(
            !problems(&raw).is_empty(),
            "the library's writer names particle data that is not there"
        );
        let eff = EffMod {
            source_rel: rel.clone(),
            ..Default::default()
        };
        // The exporter has no particle data to edit; what it would write must still be the file.
        match crate::eff_export::rebuild_eff_bytes(&bytes, &eff, Some(&root)) {
            Ok(out) => assert!(out == bytes),
            Err(err) => assert!(format!("{err:#}").contains("no embedded PTCL"), "{err:#}"),
        }
    }

    /// A child of a child never plays: the game walks one level. A stale roster asking for one
    /// is nested under the top-level emitter instead, and the checker flags a file that has one.
    #[test]
    fn a_grandchild_is_nested_one_level_and_flagged_when_written_raw() {
        let Some(root) = dump_root() else {
            return;
        };
        let bytes = game(&root, BUDDY);
        let file = effect_library::NamcoEffectFile::load(&bytes).unwrap();
        let mut eff = sand_copies(&file, 0);
        let slots = &mut eff.rosters[0].slots;
        slots[1].depth = 1;
        slots[2].depth = 2;
        let out = crate::eff_export::rebuild_eff_bytes(&bytes, &eff, Some(&root)).unwrap();
        assert!(problems(&out).is_empty(), "{:?}", problems(&out));
        let written = effect_library::NamcoEffectFile::load(&out).unwrap();
        let set = written
            .ptcl_file
            .as_ref()
            .unwrap()
            .emitter_list
            .emitter_sets
            .iter()
            .find(|s| s.name == SAND_SET)
            .unwrap();
        assert_eq!(
            set.emitters[0].children.len(),
            2,
            "both nest under the first emitter"
        );

        // The same shape written straight through the library has a grandchild.
        let mut raw = effect_library::NamcoEffectFile::load(&bytes).unwrap();
        let set = raw
            .ptcl_file
            .as_mut()
            .unwrap()
            .emitter_list
            .emitter_sets
            .iter_mut()
            .find(|s| s.name == SAND_SET)
            .unwrap();
        let second = set.emitters.remove(2);
        let mut first_child = set.emitters.remove(1);
        first_child.children.push(second);
        set.emitters[0].children.push(first_child);
        let raw = raw.save().unwrap();
        assert!(
            problems(&raw)
                .iter()
                .any(|p| p.contains("children of its own")),
            "{:?}",
            problems(&raw)
        );
    }

    /// Damage to a game file the checker must notice, one fault at a time.
    #[test]
    fn the_checker_flags_each_kind_of_damage() {
        let Some(root) = dump_root() else {
            return;
        };
        let bytes = game(&root, BUDDY);
        assert!(problems(&bytes).is_empty(), "the undamaged file passes");
        let ptcl_at = 0x1000;
        let ptcl = &bytes[ptcl_at..];
        let esta = u16::from_le_bytes(ptcl[22..24].try_into().unwrap()) as usize;
        let first_set =
            esta + u32::from_le_bytes(ptcl[esta + 8..esta + 12].try_into().unwrap()) as usize;
        let set_body = first_set
            + u32::from_le_bytes(ptcl[first_set + 20..first_set + 24].try_into().unwrap()) as usize;
        let damaged = |at: usize, value: &[u8]| {
            let mut copy = bytes.clone();
            copy[at..at + value.len()].copy_from_slice(value);
            problems(&copy)
        };
        let flags = |found: Vec<String>, what: &str| {
            assert!(
                found.iter().any(|p| p.contains(what)),
                "expected '{what}' in {found:?}"
            );
        };
        // The first set's emitter count, one too many: the loader would size its array wrong.
        let count = u32::from_le_bytes(ptcl[set_body + 80..set_body + 84].try_into().unwrap());
        flags(
            damaged(ptcl_at + set_body + 80, &(count + 1).to_le_bytes()),
            "declares",
        );
        // The header naming the wrong page for the particle data.
        flags(damaged(14, &2u16.to_le_bytes()), "particle data starts");
        // A PTCL size that does not match.
        flags(damaged(ptcl_at + 28, &0u32.to_le_bytes()), "bytes, ");
        // An effect playing a set that does not exist.
        flags(damaged(16 + 4, &9999u32.to_le_bytes()), "plays set 9999");
    }

    /// Where the model table of a written file starts, and its four-byte tag.
    fn model_table_tag(bytes: &[u8]) -> [u8; 4] {
        let pages = u16::from_le_bytes(bytes[14..16].try_into().unwrap()) as usize;
        let ptcl = &bytes[pages * 0x1000..];
        let mut at = u16::from_le_bytes(ptcl[22..24].try_into().unwrap()) as usize;
        loop {
            let next = u32::from_le_bytes(ptcl[at + 12..at + 16].try_into().unwrap());
            if &ptcl[at..at + 4] == b"G3PR" {
                let child = u32::from_le_bytes(ptcl[at + 8..at + 12].try_into().unwrap()) as usize;
                return ptcl[at + child..at + child + 4].try_into().unwrap();
            }
            assert_ne!(next, u32::MAX, "no model section");
            at += next as usize;
        }
    }

    /// An effect in `file` none of whose emitters uses a model.
    fn entry_with_no_models(file: &effect_library::NamcoEffectFile) -> Option<String> {
        let sets = &file.ptcl_file.as_ref()?.emitter_list.emitter_sets;
        let uses_no_model = |set: &effect_library::structs::EmitterSet| {
            let mut stack: Vec<&effect_library::structs::Emitter> = set.emitters.iter().collect();
            let mut none = !stack.is_empty();
            while let Some(e) = stack.pop() {
                stack.extend(e.children.iter());
                let d = &e.data;
                for id in [
                    d.particle_data.primitive_id,
                    d.particle_data.primitive_ex_id,
                    d.shape_info.primitive_index,
                ] {
                    none &= id == 0 || id == u64::MAX;
                }
            }
            none
        };
        file.entries
            .iter()
            .enumerate()
            .find(|(_, e)| {
                e.variant_count == 0
                    && sets
                        .get((e.emitter_set_id as usize).wrapping_sub(1))
                        .is_some_and(uses_no_model)
            })
            .map(|(i, _)| file.entry_names[i].clone())
    }

    /// Mario's file cut down to one effect that uses no model: the shape The Sand Bird (a
    /// GameBanana stage over Delfino Plaza) ships, an empty model table that keeps its G3NT tag.
    fn with_no_models(root: &Path) -> Vec<u8> {
        let bytes = game(root, NO_MODELS);
        let file = effect_library::NamcoEffectFile::load(&bytes).unwrap();
        let entry = entry_with_no_models(&file).expect("an effect with no models");
        crate::eff_export::strip_donor_eff_bytes(&bytes, &[entry.as_str()]).unwrap()
    }

    const NO_MODELS: &str = "effect/fighter/mario/ef_mario.eff";

    /// The Sand Bird's stage hung on the loading screen after any Visionary edit: every export
    /// re-wrote its empty model table without the G3NT tag (bisected in game to those four
    /// bytes). An edited export of a file shaped like it keeps the tag, and the checker flags a
    /// table that lost it.
    #[test]
    fn an_empty_model_table_keeps_its_tag() {
        let Some(root) = dump_root() else {
            return;
        };
        let source = with_no_models(&root);
        assert_eq!(&model_table_tag(&source), b"G3NT");
        assert!(problems(&source).is_empty(), "{:?}", problems(&source));

        // What the old export wrote for any edit: the library's own save.
        let raw = effect_library::NamcoEffectFile::load(&source)
            .unwrap()
            .save()
            .unwrap();
        assert_eq!(&model_table_tag(&raw), &[0u8; 4]);
        assert!(
            problems(&raw).iter().any(|p| p.contains("no G3NT tag")),
            "{:?}",
            problems(&raw)
        );

        // A colour edit through the exporter, and one that rewrites the model section too.
        let file = effect_library::NamcoEffectFile::load(&source).unwrap();
        let sets = &file.ptcl_file.as_ref().unwrap().emitter_list.emitter_sets;
        let set_idx = sets.iter().position(|s| !s.emitters.is_empty()).unwrap();
        let set = &sets[set_idx];
        let colour = EffMod {
            source_rel: NO_MODELS.into(),
            authored: vec![AuthoredEdit {
                set_name: set.name.clone(),
                entry_name: String::new(),
                set_idx,
                emitter_name: names_of(set)[0].0.clone(),
                emitter_idx: 0,
                fields: EmitterFieldEdits {
                    color0: Some(vec![[0.9, 0.7, 0.2, 0.0]]),
                    ..Default::default()
                },
            }],
            ..Default::default()
        };
        let out = crate::eff_export::rebuild_eff_bytes(&source, &colour, Some(&root)).unwrap();
        assert_eq!(&model_table_tag(&out), b"G3NT");
        assert!(problems(&out).is_empty(), "{:?}", problems(&out));
        let borrowed = EffMod {
            transplants: vec![TransplantOp {
                new_entry_name: "pickel_tnt".into(),
                src_file_rel: "effect/fighter/pickel/ef_pickel.eff".into(),
                src_set_name: "pickel_tnt".into(),
                src_set_idx: 0,
                one_slot_slots: Vec::new(),
                replace_entry: None,
            }],
            ..colour
        };
        let out = crate::eff_export::rebuild_eff_bytes(&source, &borrowed, Some(&root)).unwrap();
        assert_eq!(&model_table_tag(&out), b"G3NT");
        assert!(problems(&out).is_empty(), "{:?}", problems(&out));
    }

    /// Pruning a file down to effects that use no models empties its model table through the
    /// library's writer, which drops the tag; the export puts it back.
    #[test]
    fn a_table_pruned_to_nothing_keeps_its_tag() {
        let Some(root) = dump_root() else {
            return;
        };
        const MARIO: &str = "effect/fighter/mario/ef_mario.eff";
        let bytes = game(&root, MARIO);
        let file = effect_library::NamcoEffectFile::load(&bytes).unwrap();
        let entry = entry_with_no_models(&file).expect("an effect with no models");
        let stripped = crate::eff_export::strip_donor_eff_bytes(&bytes, &[entry.as_str()]).unwrap();
        let kept = effect_library::NamcoEffectFile::load(&stripped).unwrap();
        let models = kept
            .ptcl_file
            .as_ref()
            .unwrap()
            .primitive_info
            .as_ref()
            .unwrap();
        assert!(models.descriptors.is_empty(), "every model was pruned");
        assert_eq!(&model_table_tag(&stripped), b"G3NT");
        assert!(problems(&stripped).is_empty(), "{:?}", problems(&stripped));
    }

    /// A keyframe table has eight slots. A count past that is clamped on the way in, and a file
    /// that carries one anyway is flagged.
    #[test]
    fn a_key_count_past_the_table_is_clamped_and_flagged() {
        let Some(root) = dump_root() else {
            return;
        };
        let bytes = game(&root, BUDDY);
        let file = effect_library::NamcoEffectFile::load(&bytes).unwrap();
        let sets = &file.ptcl_file.as_ref().unwrap().emitter_list.emitter_sets;
        let set_idx = sets.iter().position(|s| s.name == SAND_SET).unwrap();
        let mut fields = EmitterFieldEdits::default();
        fields.attrs.insert(
            "emitter_static.num_color0_keys".into(),
            crate::eff_attrs::AttrValue::Int(20),
        );
        let eff = EffMod {
            source_rel: BUDDY.into(),
            authored: vec![AuthoredEdit {
                set_name: SAND_SET.into(),
                entry_name: "BUDDY_FINAL_APPEAR_GROUND".into(),
                set_idx,
                emitter_name: "sand1".into(),
                emitter_idx: names_of(&sets[set_idx])
                    .iter()
                    .position(|(n, _)| n == "sand1")
                    .unwrap(),
                fields,
            }],
            ..Default::default()
        };
        let out = crate::eff_export::rebuild_eff_bytes(&bytes, &eff, Some(&root)).unwrap();
        let written = effect_library::NamcoEffectFile::load(&out).unwrap();
        let sand = written
            .ptcl_file
            .as_ref()
            .unwrap()
            .emitter_list
            .emitter_sets[set_idx]
            .emitters
            .iter()
            .find(|e| e.data.display_name() == "sand1")
            .unwrap();
        assert_eq!(sand.data.emitter_static.num_color0_keys, 8);

        let mut raw = effect_library::NamcoEffectFile::load(&bytes).unwrap();
        raw.ptcl_file.as_mut().unwrap().emitter_list.emitter_sets[set_idx].emitters[0]
            .data
            .emitter_static
            .num_color0_keys = 20;
        let raw = raw.save().unwrap();
        assert!(
            problems(&raw)
                .iter()
                .any(|p| p.contains("20 colour 0 keys")),
            "{:?}",
            problems(&raw)
        );
    }

    /// Every effect file under `VISIONARY_VALIDATE_DIR` (an exported mod's folder) passes and
    /// every costume effect file is listed for its costume: the checks an export runs, for a
    /// folder written without the editor's file dialog.
    #[test]
    fn a_folders_effect_files_pass() {
        let Some(root) = std::env::var_os("VISIONARY_VALIDATE_DIR").map(std::path::PathBuf::from)
        else {
            eprintln!("skipped: set VISIONARY_VALIDATE_DIR to an exported mod folder");
            return;
        };
        fn walk(dir: &std::path::Path, out: &mut Vec<std::path::PathBuf>) {
            for e in std::fs::read_dir(dir).into_iter().flatten().flatten() {
                let path = e.path();
                if path.is_dir() {
                    walk(&path, out);
                } else if path.extension().is_some_and(|x| x == "eff") {
                    out.push(path);
                }
            }
        }
        let mut files = Vec::new();
        walk(&root, &mut files);
        assert!(!files.is_empty(), "no effect files in {}", root.display());
        let unlisted = crate::roster::costume_config::undeclared_costume_effects(&root);
        assert!(
            unlisted.is_empty(),
            "costume effect files not listed in config.json: {unlisted:?}"
        );
        for path in &files {
            let bytes = std::fs::read(path).unwrap();
            let found = problems(&bytes);
            assert!(found.is_empty(), "{}: {found:?}", path.display());
            eprintln!("valid: {} ({} bytes)", path.display(), bytes.len());
        }
    }
}
