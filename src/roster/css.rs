//! The character select screen database: `ui/param/database/ui_chara_db.prc`.
//!
//! What decides a character's position on the select screen is the `disp_order` field of that
//! character's `db_root` entry, sorted ascending — **not** the list order, and not `save_no`.
//! The two `I8` fields agree for 110 of vanilla's 121 entries, which is exactly why it had to
//! be measured; they diverge on Inkling/Ridley/Simon/Richter/K. Rool, and the real select
//! screen follows `disp_order`.
//!
//! Three properties of the field shape everything here:
//!
//!  * `-1` means "not on the select screen", and every vanilla entry carrying it also has
//!    `can_select = false`. The two are written together; setting one alone produces an entry
//!    the game disagrees with itself about.
//!  * `99` is the Random slot. It is a sentinel, not a position, and is never renumbered.
//!  * The value is not unique — Pyra and Mythra share `80`. Entries sharing a value share a
//!    cell, so a reorder moves the whole group.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use anyhow::{Context, Result};
use prc::ParamKind;

/// Where the roster database lives inside a dumped arc root.
pub const CHARA_DB_PATH: &str = "ui/param/database/ui_chara_db.prc";

/// A patch for a database file: the same path with this in place of `.prc`. ARCropolis merges
/// it into the game's file, so two mods can each change their own entries, and plugins that
/// rebuild the database themselves (the CSK Collection) still see the change, which a whole
/// replaced file does not get them.
pub const PATCH_EXTENSION: &str = "prcxml";

/// The layout database beside it: where each character's portraits sit in their frames.
pub const LAYOUT_DB_PATH: &str = "ui/param/database/ui_layout_db.prc";

/// The field the CSK Collection plugin reads on an added character: the fighter costume its
/// first colour is. The game has no such field; without the plugin an added character plays
/// in the base fighter's own costumes.
pub const COLOR_START_FIELD: &str = "color_start_index";

/// `disp_order` for an entry that is not on the select screen.
pub const OFF_ROSTER: i8 = -1;

/// `disp_order` of the Random slot. Reserved: it is a sentinel, not a position.
pub const RANDOM_SLOT_ORDER: i8 = 99;

/// Find the arc root that provides the UI database.
///
/// Searched in the same precedence the rest of the tool uses for game files: the data root
/// first, then enabled mod roots in load order, so a mod that ships its own roster database
/// is picked up without a second configuration step. Returns the root, not the file, because
/// portraits and names are looked up relative to the same place.
pub fn locate_ui_root(roots: &[PathBuf]) -> Option<PathBuf> {
    roots
        .iter()
        .find(|root| root.join(CHARA_DB_PATH).is_file())
        .cloned()
}

/// The `fighter_kind` value a fighter directory name corresponds to.
///
/// Verified against real data: Mario's row carries `hash40("fighter_kind_mario")`, and the Ice
/// Climbers' carries `hash40("fighter_kind_ice_climber")`. This is the reliable link between a
/// database row and a fighter, because `name_id` is not always the directory name.
pub fn fighter_kind_hash(fighter: &str) -> u64 {
    hash40::hash40(&format!("fighter_kind_{}", fighter.to_ascii_lowercase())).0
}

/// One character select entry, read from a `db_root` element.
///
/// Only the fields the roster editor reasons about are lifted out. The full struct stays in
/// [`CharaDb::root`] and is what gets written back, so a field this type does not know about
/// survives a load-and-save untouched.
#[derive(Debug, Clone, PartialEq)]
pub struct CharaRow {
    /// Index into `db_root`. Stable for one loaded file; not the select screen position.
    pub index: usize,
    pub name_id: String,
    pub ui_chara_id: u64,
    pub fighter_kind: u64,
    pub disp_order: i8,
    pub save_no: i8,
    pub can_select: bool,
    /// Number of costume slots the select screen offers for this character.
    pub color_num: u8,
}

impl CharaRow {
    /// True when this entry occupies a cell on the select screen.
    pub fn on_roster(&self) -> bool {
        self.disp_order != OFF_ROSTER && self.disp_order != RANDOM_SLOT_ORDER && self.can_select
    }
}

/// A loaded `ui_chara_db.prc`.
pub struct CharaDb {
    /// The whole parsed file. Edits mutate this, and it is what [`CharaDb::save`] writes, so
    /// every field the editor does not model round-trips untouched.
    root: prc::ParamStruct,
    /// The lifted view, rebuilt after every edit rather than mutated alongside the tree —
    /// the tree is the model, this is a view of it.
    entries: Vec<CharaRow>,
}

impl CharaDb {
    pub fn open(path: &Path) -> Result<Self> {
        let root = prc::open(path)
            .map_err(|error| anyhow::anyhow!("{error:?}"))
            .with_context(|| format!("reading {}", path.display()))?;
        let mut db = Self {
            root,
            entries: Vec::new(),
        };
        db.refresh();
        if db.entries.is_empty() {
            anyhow::bail!(
                "{} has no db_root entries — is this a ui_chara_db?",
                path.display()
            );
        }
        Ok(db)
    }

    pub fn save(&self, path: &Path) -> Result<()> {
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)?;
        }
        prc::save(path, &self.root)
            .map_err(|error| anyhow::anyhow!("{error:?}"))
            .with_context(|| format!("writing {}", path.display()))
    }

    /// The whole parsed file, to compare against another for a patch.
    pub fn tree(&self) -> &prc::ParamStruct {
        &self.root
    }

    pub fn entries(&self) -> &[CharaRow] {
        &self.entries
    }

    pub fn row(&self, name_id: &str) -> Option<&CharaRow> {
        self.entries.iter().find(|row| row.name_id == name_id)
    }

    /// Set a character's select screen position.
    ///
    /// Writing [`OFF_ROSTER`] also clears `can_select`, and writing a real position sets it:
    /// vanilla never disagrees between the two, and an entry that does is one the game will
    /// render but refuse to pick, or list as pickable at a position that does not exist.
    pub fn set_disp_order(&mut self, name_id: &str, order: i8) -> Result<()> {
        let can_select = order != OFF_ROSTER;
        self.set_field(name_id, "disp_order", ParamKind::I8(order))?;
        self.set_field(name_id, "can_select", ParamKind::Bool(can_select))?;
        self.refresh();
        Ok(())
    }

    /// Apply a complete desired ordering, given as `name_id → position`.
    ///
    /// Applied as one pass so that a swap cannot transiently collide, and returns the names it
    /// could not find rather than skipping them silently — a project override naming a
    /// character this database does not contain is exactly the stale-override case the roster
    /// index reports, and the export path has to be able to report it too.
    pub fn apply_order(&mut self, order: &BTreeMap<String, i8>) -> Vec<String> {
        let mut missing = Vec::new();
        for (name_id, position) in order {
            if self.set_disp_order(name_id, *position).is_err() {
                missing.push(name_id.clone());
            }
        }
        missing
    }

    /// Apply per-entry `ui_chara_db` patches (color_num, save_no, ...).
    pub fn apply_chara_patches(
        &mut self,
        patches: &BTreeMap<String, crate::mod_project::CharaOverrides>,
    ) -> Vec<String> {
        let mut missing = Vec::new();
        for (name_id, patch) in patches {
            if let Some(n) = patch.color_num {
                if self
                    .set_field(name_id, "color_num", ParamKind::U8(n))
                    .is_err()
                {
                    missing.push(name_id.clone());
                    continue;
                }
            }
            if let Some(n) = patch.save_no {
                if self
                    .set_field(name_id, "save_no", ParamKind::I8(n))
                    .is_err()
                {
                    if !missing.contains(name_id) {
                        missing.push(name_id.clone());
                    }
                    continue;
                }
            }
        }
        if !missing.is_empty() || !patches.is_empty() {
            self.refresh();
        }
        missing
    }

    /// The highest position any character on the select screen holds.
    pub fn last_position(&self) -> i8 {
        self.entries.iter().filter(|row| row.on_roster()).map(|row| row.disp_order).max().unwrap_or(0)
    }

    /// Add a character of its own that plays as `fighter` in the costumes `slots`: a copy of
    /// that fighter's entry under `name_id`, at `order` on the select screen. Colour `k` of the
    /// new character is costume `slots[k]`, which the CSK Collection plugin acts on.
    pub fn add_character(&mut self, fighter: &str, name_id: &str, slots: &[u8], order: i8) -> Result<()> {
        use prc::hash40::Hash40;
        if self.row(name_id).is_some() {
            anyhow::bail!("the select screen already has a character called {name_id}");
        }
        let (Some(first), true) = (slots.first().copied(), slots.len() <= 8) else {
            anyhow::bail!("a character needs one to eight costumes");
        };
        let kind = fighter_kind_hash(fighter);
        let base = self
            .entries
            .iter()
            .find(|row| row.fighter_kind == kind)
            .map(|row| row.index)
            .with_context(|| format!("the select screen has no entry for the fighter {fighter}"))?;
        let Some(ParamKind::List(list)) = db_root_mut(&mut self.root) else {
            anyhow::bail!("db_root is missing");
        };
        let Some(ParamKind::Struct(mut entry)) = list.0.get(base).cloned() else {
            anyhow::bail!("db_root entry {base} is not a struct");
        };
        let hash = |name: &str| Hash40(hash40::hash40(name).0);
        let mut set = |name: &str, value: ParamKind| {
            let wanted = hash(name);
            match entry.0.iter_mut().find(|(field, _)| *field == wanted) {
                Some((_, slot)) => *slot = value,
                None => entry.0.push((wanted, value)),
            }
        };
        set("ui_chara_id", ParamKind::Hash(hash(&format!("ui_chara_{name_id}"))));
        set("name_id", ParamKind::Str(name_id.to_owned()));
        // Not an echo of anyone, whatever the base fighter is.
        set("alt_chara_id", ParamKind::Hash(Hash40(0)));
        set("fighter_type", ParamKind::Hash(hash("fighter_type_normal")));
        set("disp_order", ParamKind::I8(order));
        set("can_select", ParamKind::Bool(order != OFF_ROSTER));
        set("color_num", ParamKind::U8(slots.len() as u8));
        for colour in 0..8usize {
            let slot = slots.get(colour).copied().unwrap_or(first);
            set(&format!("c{colour:02}_index"), ParamKind::U8(slot));
            set(&format!("n{colour:02}_index"), ParamKind::U8(0));
            set(&format!("c{colour:02}_group"), ParamKind::U8(0));
        }
        set(COLOR_START_FIELD, ParamKind::U8(first));
        list.0.push(ParamKind::Struct(entry));
        self.refresh();
        Ok(())
    }

    fn set_field(&mut self, name_id: &str, field: &str, value: ParamKind) -> Result<()> {
        let index = self
            .entries
            .iter()
            .find(|row| row.name_id == name_id)
            .map(|row| row.index)
            .with_context(|| format!("no character select entry named {name_id}"))?;
        let field_hash = hash40::hash40(field).0;
        let Some(ParamKind::List(list)) = db_root_mut(&mut self.root) else {
            anyhow::bail!("db_root is missing");
        };
        let Some(ParamKind::Struct(entry)) = list.0.get_mut(index) else {
            anyhow::bail!("db_root entry {index} is not a struct");
        };
        for (hash, slot) in entry.0.iter_mut() {
            if hash.0 == field_hash {
                *slot = value;
                return Ok(());
            }
        }
        anyhow::bail!("entry {name_id} has no {field} field")
    }

    /// Rebuild the lifted view from the tree.
    fn refresh(&mut self) {
        self.entries.clear();
        let Some(ParamKind::List(list)) = db_root(&self.root) else {
            return;
        };
        for (index, item) in list.0.iter().enumerate() {
            let ParamKind::Struct(entry) = item else {
                continue;
            };
            let Some(name_id) = string_field(entry, "name_id") else {
                continue;
            };
            self.entries.push(CharaRow {
                index,
                name_id,
                ui_chara_id: hash_field(entry, "ui_chara_id").unwrap_or(0),
                fighter_kind: hash_field(entry, "fighter_kind").unwrap_or(0),
                disp_order: i8_field(entry, "disp_order").unwrap_or(OFF_ROSTER),
                save_no: i8_field(entry, "save_no").unwrap_or(OFF_ROSTER),
                can_select: bool_field(entry, "can_select").unwrap_or(false),
                color_num: u8_field(entry, "color_num").unwrap_or(0),
            });
        }
    }
}

/// Give each added character the portrait layout of the fighter it plays as: `ui_layout_db`
/// read from `source`, with a copy of that fighter's rows per `(its ui_chara_id, name_id)`.
/// Returns the file as it was and as it is with them.
pub fn add_layouts(source: &Path, added: &[(u64, String)]) -> Result<(prc::ParamStruct, prc::ParamStruct)> {
    use prc::hash40::Hash40;
    let base = prc::open(source)
        .map_err(|error| anyhow::anyhow!("{error:?}"))
        .with_context(|| format!("reading {}", source.display()))?;
    let mut root = base.clone();
    let Some(ParamKind::List(list)) = db_root_mut(&mut root) else {
        anyhow::bail!("{} has no db_root", source.display());
    };
    for (own, name_id) in added {
        let rows: Vec<prc::ParamStruct> = list
            .0
            .iter()
            .filter_map(|row| match row {
                ParamKind::Struct(row) if hash_field(row, "ui_chara_id") == Some(*own) => Some(row.clone()),
                _ => None,
            })
            .collect();
        for mut row in rows {
            let colour = u8_field(&row, "chara_color").unwrap_or(0);
            for (field, value) in row.0.iter_mut() {
                if field.0 == hash40::hash40("ui_chara_id").0 {
                    *value = ParamKind::Hash(Hash40(hash40::hash40(&format!("ui_chara_{name_id}")).0));
                } else if field.0 == hash40::hash40("ui_layout_id").0 {
                    *value = ParamKind::Hash(Hash40(hash40::hash40(&format!("ui_chara_{name_id}_{colour:02}")).0));
                }
            }
            list.0.push(ParamKind::Struct(row));
        }
    }
    Ok((base, root))
}

/// What `edited` changes in the `db_root` of `base`, as a `.prcxml` patch: each changed entry
/// by its index with only the fields that differ, and each added entry in full. `None` when
/// nothing differs. `labels` names the fields; one it does not know is written as its hash.
pub fn patch_xml(base: &prc::ParamStruct, edited: &prc::ParamStruct, labels: &std::collections::HashMap<u64, String>) -> Option<String> {
    let (Some(ParamKind::List(base)), Some(ParamKind::List(edited))) = (db_root(base), db_root(edited)) else {
        return None;
    };
    let name = |hash: u64| labels.get(&hash).cloned().unwrap_or_else(|| format!("0x{hash:010x}"));
    let escape = |text: &str| text.replace('&', "&amp;").replace('<', "&lt;").replace('>', "&gt;");
    let field = |hash: u64, value: &ParamKind| -> Option<String> {
        let (tag, text) = match value {
            ParamKind::Bool(value) => ("bool", if *value { "True".to_owned() } else { "False".to_owned() }),
            ParamKind::I8(value) => ("sbyte", value.to_string()),
            ParamKind::U8(value) => ("byte", value.to_string()),
            ParamKind::I16(value) => ("short", value.to_string()),
            ParamKind::U16(value) => ("ushort", value.to_string()),
            ParamKind::I32(value) => ("int", value.to_string()),
            ParamKind::U32(value) => ("uint", value.to_string()),
            ParamKind::Float(value) => ("float", value.to_string()),
            ParamKind::Hash(value) => ("hash40", name(value.0)),
            ParamKind::Str(value) => ("string", escape(value)),
            // No entry of these databases nests another.
            ParamKind::List(_) | ParamKind::Struct(_) => return None,
        };
        Some(format!("      <{tag} hash=\"{}\">{text}</{tag}>", name(hash)))
    };
    let mut rows = Vec::new();
    for (index, row) in edited.0.iter().enumerate() {
        let ParamKind::Struct(row) = row else { continue };
        let before = match base.0.get(index) {
            Some(ParamKind::Struct(before)) => Some(before),
            _ => None,
        };
        let fields: Vec<String> = row
            .0
            .iter()
            .filter(|(hash, value)| before.is_none_or(|before| before.0.iter().find(|(known, _)| known == hash).map(|(_, old)| old) != Some(value)))
            .filter_map(|(hash, value)| field(hash.0, value))
            .collect();
        if !fields.is_empty() {
            rows.push(format!("    <struct index=\"{index}\">\n{}\n    </struct>", fields.join("\n")));
        }
    }
    if rows.is_empty() {
        return None;
    }
    Some(format!(
        "<?xml version=\"1.0\" encoding=\"utf-8\"?>\n<struct>\n  <list hash=\"db_root\">\n{}\n  </list>\n</struct>\n",
        rows.join("\n")
    ))
}

/// What `edited` changes in the `db_root` of `base`, as entries for the CSK Collection's own
/// database files (`database/*.json` in a mod). That plugin rebuilds these databases from the
/// game's files and its own entries, so it is the one place a change reaches it. A changed
/// entry carries its id and the fields that differ; an added one names the entry it is closest
/// to as the one to copy, and the fields that differ from that. `id_field` is the entries' id
/// (`ui_chara_id`, `ui_layout_id`).
pub fn csk_entries(
    base: &prc::ParamStruct,
    edited: &prc::ParamStruct,
    labels: &std::collections::HashMap<u64, String>,
    id_field: &str,
) -> Vec<serde_json::Value> {
    use serde_json::{json, Map, Value};
    let (Some(ParamKind::List(base)), Some(ParamKind::List(edited))) = (db_root(base), db_root(edited)) else {
        return Vec::new();
    };
    let id_hash = hash40::hash40(id_field).0;
    let name = |hash: u64| match hash {
        0 => String::new(),
        hash => labels.get(&hash).cloned().unwrap_or_else(|| format!("0x{hash:010x}")),
    };
    // Per-colour tables (`c00_index`, `characall_label_c00`, ...) are maps in the plugin's
    // format, which `color_start_index` fills; they are not written field by field.
    let per_colour = |field: &str| {
        field.starts_with("characall_label")
            || (field.len() > 4 && matches!(field.as_bytes()[0], b'c' | b'n') && field[1..3].bytes().all(|b| b.is_ascii_digit()) && field.as_bytes()[3] == b'_')
    };
    let value = |value: &ParamKind| -> Option<Value> {
        Some(match value {
            ParamKind::Bool(value) => json!(value),
            ParamKind::I8(value) => json!(value),
            ParamKind::U8(value) => json!(value),
            ParamKind::I16(value) => json!(value),
            ParamKind::U16(value) => json!(value),
            ParamKind::I32(value) => json!(value),
            ParamKind::U32(value) => json!(value),
            ParamKind::Float(value) => json!(value),
            ParamKind::Hash(value) => json!(name(value.0)),
            ParamKind::Str(value) => json!(value),
            ParamKind::List(_) | ParamKind::Struct(_) => return None,
        })
    };
    let differing = |row: &prc::ParamStruct, from: &prc::ParamStruct| -> Vec<(String, Value)> {
        row.0
            .iter()
            .filter(|(hash, _)| hash.0 != id_hash)
            .filter(|(hash, new)| from.0.iter().find(|(known, _)| known == hash).map(|(_, old)| old) != Some(new))
            .filter_map(|(hash, new)| Some((name(hash.0), value(new)?)))
            .filter(|(field, _)| !per_colour(field))
            // The plugin keeps the first costume in its map of per-colour numbers (seen in
            // game: as a field of the entry it is ignored and the base costumes load).
            .map(|(field, value)| if field == COLOR_START_FIELD { ("extra_index_maps".to_owned(), json!({ COLOR_START_FIELD: value })) } else { (field, value) })
            .collect()
    };
    let rows = |list: &prc::ParamList| -> Vec<prc::ParamStruct> {
        list.0.iter().filter_map(|row| if let ParamKind::Struct(row) = row { Some(row.clone()) } else { None }).collect()
    };
    let (base, edited) = (rows(base), rows(edited));
    let mut entries = Vec::new();
    for (index, row) in edited.iter().enumerate() {
        let Some(id) = hash_field(row, id_field) else { continue };
        let mut entry = Map::new();
        entry.insert(id_field.to_owned(), json!(name(id)));
        let fields = match base.get(index) {
            Some(before) => differing(row, before),
            None => {
                let Some((like, fields)) = base.iter().map(|like| (like, differing(row, like))).min_by_key(|(_, fields)| fields.len()) else {
                    continue;
                };
                entry.insert(format!("clone_from_{id_field}"), json!(name(hash_field(like, id_field).unwrap_or(0))));
                fields
            }
        };
        if fields.is_empty() && entry.len() == 1 {
            continue;
        }
        entry.extend(fields);
        entries.push(Value::Object(entry));
    }
    entries
}

fn db_root(root: &prc::ParamStruct) -> Option<&ParamKind> {
    let wanted = hash40::hash40("db_root").0;
    root.0
        .iter()
        .find(|(hash, _)| hash.0 == wanted)
        .map(|(_, value)| value)
}

fn db_root_mut(root: &mut prc::ParamStruct) -> Option<&mut ParamKind> {
    let wanted = hash40::hash40("db_root").0;
    root.0
        .iter_mut()
        .find(|(hash, _)| hash.0 == wanted)
        .map(|(_, value)| value)
}

fn field<'a>(entry: &'a prc::ParamStruct, name: &str) -> Option<&'a ParamKind> {
    let wanted = hash40::hash40(name).0;
    entry
        .0
        .iter()
        .find(|(hash, _)| hash.0 == wanted)
        .map(|(_, value)| value)
}

fn string_field(entry: &prc::ParamStruct, name: &str) -> Option<String> {
    match field(entry, name)? {
        ParamKind::Str(value) => Some(value.clone()),
        _ => None,
    }
}

fn hash_field(entry: &prc::ParamStruct, name: &str) -> Option<u64> {
    match field(entry, name)? {
        ParamKind::Hash(value) => Some(value.0),
        _ => None,
    }
}

fn i8_field(entry: &prc::ParamStruct, name: &str) -> Option<i8> {
    match field(entry, name)? {
        ParamKind::I8(value) => Some(*value),
        _ => None,
    }
}

fn u8_field(entry: &prc::ParamStruct, name: &str) -> Option<u8> {
    match field(entry, name)? {
        ParamKind::U8(value) => Some(*value),
        _ => None,
    }
}

fn bool_field(entry: &prc::ParamStruct, name: &str) -> Option<bool> {
    match field(entry, name)? {
        ParamKind::Bool(value) => Some(*value),
        _ => None,
    }
}

/// Build a database in memory from `(name_id, disp_order, fighter_kind, can_select)`.
///
/// Test-only, and shared with `index.rs` so the roster index is exercised against the same
/// row model the real file produces rather than a second hand-written stand-in.
#[cfg(test)]
pub(crate) fn test_db(rows: &[(&str, i8, u64, bool)]) -> CharaDb {
    use prc::hash40::Hash40;
    use prc::{ParamList, ParamStruct};

    let entries = rows
        .iter()
        .map(|(name_id, disp_order, fighter_kind, can_select)| {
            ParamKind::Struct(ParamStruct(vec![
                (
                    Hash40(hash40::hash40("name_id").0),
                    ParamKind::Str((*name_id).into()),
                ),
                (
                    Hash40(hash40::hash40("ui_chara_id").0),
                    ParamKind::Hash(Hash40(hash40::hash40(&format!("ui_chara_{name_id}")).0)),
                ),
                (
                    Hash40(hash40::hash40("fighter_kind").0),
                    ParamKind::Hash(Hash40(*fighter_kind)),
                ),
                (
                    Hash40(hash40::hash40("disp_order").0),
                    ParamKind::I8(*disp_order),
                ),
                (Hash40(hash40::hash40("save_no").0), ParamKind::I8(0)),
                (
                    Hash40(hash40::hash40("can_select").0),
                    ParamKind::Bool(*can_select),
                ),
                (Hash40(hash40::hash40("color_num").0), ParamKind::U8(8)),
            ]))
        })
        .collect();
    let mut db = CharaDb {
        root: ParamStruct(vec![(
            Hash40(hash40::hash40("db_root").0),
            ParamKind::List(ParamList(entries)),
        )]),
        entries: Vec::new(),
    };
    db.refresh();
    db
}

#[cfg(test)]
mod tests {
    use super::*;
    use prc::hash40::Hash40;
    use prc::{ParamList, ParamStruct};

    fn entry(name: &str, disp_order: i8, save_no: i8, can_select: bool) -> ParamKind {
        ParamKind::Struct(ParamStruct(vec![
            (
                Hash40(hash40::hash40("name_id").0),
                ParamKind::Str(name.into()),
            ),
            (
                Hash40(hash40::hash40("ui_chara_id").0),
                ParamKind::Hash(Hash40(hash40::hash40(name).0)),
            ),
            (
                Hash40(hash40::hash40("fighter_kind").0),
                ParamKind::Hash(Hash40(hash40::hash40(name).0)),
            ),
            (
                Hash40(hash40::hash40("disp_order").0),
                ParamKind::I8(disp_order),
            ),
            (Hash40(hash40::hash40("save_no").0), ParamKind::I8(save_no)),
            (
                Hash40(hash40::hash40("can_select").0),
                ParamKind::Bool(can_select),
            ),
            (Hash40(hash40::hash40("color_num").0), ParamKind::U8(8)),
        ]))
    }

    /// Select screen order, the way the roster index computes it: the rows that occupy a cell,
    /// by position, ties keeping file order because they share a cell.
    fn roster_order(db: &CharaDb) -> Vec<&CharaRow> {
        let mut rows: Vec<&CharaRow> = db.entries().iter().filter(|row| row.on_roster()).collect();
        rows.sort_by_key(|row| (row.disp_order, row.index));
        rows
    }

    /// A stand-in for the real file with the properties that matter: a Random sentinel, an
    /// off-roster entry, a shared position, and a `save_no` that disagrees with `disp_order`.
    fn db() -> CharaDb {
        let root = ParamStruct(vec![(
            Hash40(hash40::hash40("db_root").0),
            ParamKind::List(ParamList(vec![
                entry("random", RANDOM_SLOT_ORDER, -1, false),
                entry("mario", 0, 0, true),
                entry("richter", 67, 64, true),
                entry("inkling", 64, 65, true),
                entry("simon", 66, 68, true),
                entry("eflame_first", 80, 83, true),
                entry("elight_first", 80, 83, true),
                entry("masterhand", OFF_ROSTER, -1, false),
            ])),
        )]);
        let mut db = CharaDb {
            root,
            entries: Vec::new(),
        };
        db.refresh();
        db
    }

    /// The whole reason this module exists. Following `save_no` would put Richter before
    /// Simon, which is not what the game shows.
    #[test]
    fn roster_order_follows_disp_order_not_save_no_or_list_order() {
        let db = db();
        let names: Vec<&str> = roster_order(&db)
            .iter()
            .map(|row| row.name_id.as_str())
            .collect();
        assert_eq!(
            names,
            vec![
                "mario",
                "inkling",
                "simon",
                "richter",
                "eflame_first",
                "elight_first"
            ]
        );
    }

    #[test]
    fn the_random_slot_and_off_roster_entries_are_not_positions() {
        let db = db();
        // The Random slot is a sentinel, not a position, and must never be drawn as a cell.
        assert_eq!(db.row("random").unwrap().disp_order, RANDOM_SLOT_ORDER);
        assert!(!db.row("random").unwrap().on_roster());
        assert!(!db.row("masterhand").unwrap().on_roster());
    }

    /// Two entries sharing a position share a cell, so neither may be dropped or reordered
    /// past the other by the tie-break.
    #[test]
    fn entries_sharing_a_position_stay_adjacent_in_file_order() {
        let db = db();
        let shared: Vec<&str> = roster_order(&db)
            .iter()
            .filter(|row| row.disp_order == 80)
            .map(|row| row.name_id.as_str())
            .collect();
        assert_eq!(shared, vec!["eflame_first", "elight_first"]);
    }

    /// Vanilla never disagrees between these two fields, and an entry that does is one the
    /// game will render but refuse to pick.
    #[test]
    fn hiding_and_restoring_keeps_can_select_in_step_with_disp_order() {
        let mut db = db();
        db.set_disp_order("mario", OFF_ROSTER).unwrap();
        let row = db.row("mario").unwrap();
        assert_eq!(row.disp_order, OFF_ROSTER);
        assert!(!row.can_select);
        assert!(!row.on_roster());

        db.set_disp_order("mario", 3).unwrap();
        let row = db.row("mario").unwrap();
        assert_eq!(row.disp_order, 3);
        assert!(row.can_select);
        assert!(row.on_roster());
    }

    #[test]
    fn an_added_character_is_a_copy_of_its_fighter_in_its_own_costumes() {
        let mut db = test_db(&[("mario", 0, fighter_kind_hash("mario"), true), ("lucina", 23, fighter_kind_hash("lucina"), true)]);
        db.add_character("lucina", "shade", &[8, 9, 10], 24).unwrap();
        let row = db.row("shade").unwrap().clone();
        assert_eq!(row.fighter_kind, fighter_kind_hash("lucina"));
        assert_eq!(row.ui_chara_id, hash40::hash40("ui_chara_shade").0);
        assert_eq!((row.disp_order, row.can_select, row.color_num), (24, true, 3));
        let Some(ParamKind::List(list)) = db_root(&db.root) else { panic!() };
        let Some(ParamKind::Struct(entry)) = list.0.last() else { panic!() };
        assert_eq!(u8_field(entry, COLOR_START_FIELD), Some(8));
        assert_eq!(u8_field(entry, "c02_index"), Some(10));
        // The base fighter's own entry is untouched, and a second one of the same name refused.
        assert_eq!(db.row("lucina").unwrap().disp_order, 23);
        assert!(db.add_character("lucina", "shade", &[8], 25).is_err());
        assert!(db.add_character("nobody", "x", &[8], 25).is_err());
    }

    #[test]
    fn an_order_naming_an_absent_character_is_reported_rather_than_skipped() {
        let mut db = db();
        let mut order = BTreeMap::new();
        order.insert("mario".to_string(), 5);
        order.insert("nosuchfighter".to_string(), 6);
        let missing = db.apply_order(&order);
        assert_eq!(missing, vec!["nosuchfighter"]);
        assert_eq!(db.row("mario").unwrap().disp_order, 5);
    }

    /// Fields this editor does not model must survive a load-and-save. The real file carries
    /// 75 fields per entry and models seven of them.
    #[test]
    fn unmodelled_fields_survive_a_round_trip() {
        let file = tempfile::NamedTempFile::new().unwrap();
        let mut root = ParamStruct(vec![(
            Hash40(hash40::hash40("db_root").0),
            ParamKind::List(ParamList(vec![entry("mario", 0, 0, true)])),
        )]);
        // A field the lifted view knows nothing about.
        if let Some(ParamKind::List(list)) = db_root_mut(&mut root) {
            if let Some(ParamKind::Struct(st)) = list.0.get_mut(0) {
                st.0.push((
                    Hash40(hash40::hash40("exhibit_year").0),
                    ParamKind::I16(1981),
                ));
            }
        }
        prc::save(file.path(), &root).unwrap();

        let mut db = CharaDb::open(file.path()).unwrap();
        db.set_disp_order("mario", 7).unwrap();
        db.save(file.path()).unwrap();

        let reloaded = prc::open(file.path()).unwrap();
        let ParamKind::List(list) = db_root(&reloaded).unwrap() else {
            panic!("db_root lost");
        };
        let ParamKind::Struct(st) = &list.0[0] else {
            panic!("entry lost");
        };
        assert_eq!(i8_field(st, "disp_order"), Some(7));
        assert_eq!(
            field(st, "exhibit_year"),
            Some(&ParamKind::I16(1981)),
            "a field the editor does not model was dropped by a save"
        );
    }

    /// Reading and writing without editing must produce the same file. Until this holds,
    /// nothing about an edited file can be trusted.
    #[test]
    fn an_untouched_load_and_save_is_byte_identical() {
        let file = tempfile::NamedTempFile::new().unwrap();
        let source = db();
        prc::save(file.path(), &source.root).unwrap();
        let original = std::fs::read(file.path()).unwrap();

        let db = CharaDb::open(file.path()).unwrap();
        let out = tempfile::NamedTempFile::new().unwrap();
        db.save(out.path()).unwrap();
        assert_eq!(std::fs::read(out.path()).unwrap(), original);
    }

    /// The synthetic fixture above pins the logic; only a real file pins the *format*. A
    /// hand-built `ParamStruct` round-trips through `prc` trivially — the question that
    /// matters is whether a 121-entry, 75-field vanilla database does, and whether the
    /// ordering rule reproduces the select screen everyone has seen.
    ///
    /// Skipped unless `VISIONARY_TEST_CHARA_DB` points at one, because the file cannot be
    /// committed. Run it whenever this module changes:
    ///
    /// ```text
    /// VISIONARY_TEST_CHARA_DB=/path/to/ui_chara_db.prc cargo test --bin visionary against_a_real
    /// ```
    #[test]
    fn against_a_real_chara_db() {
        let Some(path) = std::env::var_os("VISIONARY_TEST_CHARA_DB") else {
            return;
        };
        let path = PathBuf::from(path);
        let db = CharaDb::open(&path).expect("a real ui_chara_db must load");

        let out = tempfile::NamedTempFile::new().unwrap();
        db.save(out.path()).unwrap();
        assert_eq!(
            std::fs::read(out.path()).unwrap(),
            std::fs::read(&path).unwrap(),
            "an untouched load-and-save of a real ui_chara_db changed its bytes"
        );

        let order: Vec<&str> = roster_order(&db)
            .iter()
            .map(|row| row.name_id.as_str())
            .collect();
        assert_eq!(order.first(), Some(&"mario"));

        // The five entries whose `disp_order` disagrees with `save_no`. This sequence is what
        // the game shows and what following `save_no` would get wrong.
        let tail: Vec<&&str> = order
            .iter()
            .skip_while(|name| **name != "inkling")
            .take(5)
            .collect();
        assert_eq!(
            tail,
            vec![&"inkling", &"ridley", &"simon", &"richter", &"krool"],
            "the select screen sequence around the Ultimate newcomers is wrong"
        );

        // The properties the rest of this module is built on, asserted against real data
        // rather than against the fixture that was written to have them.
        assert_eq!(
            db.row("random").map(|row| row.disp_order),
            Some(RANDOM_SLOT_ORDER)
        );
        assert!(db
            .entries()
            .iter()
            .any(|row| row.disp_order == OFF_ROSTER && !row.can_select));
        let mut positions: Vec<i8> = roster_order(&db).iter().map(|row| row.disp_order).collect();
        let total = positions.len();
        positions.dedup();
        assert!(
            positions.len() < total,
            "a real database has at least one shared position (Pyra/Mythra)"
        );
    }
}
