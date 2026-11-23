//! Adding a part to an effect: a copy of any part in the open file or the shared file, or one of
//! a few starters (glow, sparks, smoke, ring, flash, trail), each a real game part.
//!
//! A part copied from another effect is recorded in the effect's emitter list as a slot naming
//! its effect and file (`EmitterSlot::source_set` / `source_file`); the exporter and the live
//! carrier bring in what it needs from there (its textures, models and shaders) through the same
//! merge a borrowed effect uses. In the editor its file's own values are kept as an extra
//! pristine snapshot at the end of the set's list, so every edit, reset and undo that works on a
//! copied part works on it.

use super::*;
use crate::effects::{EmitterSet, LoadedEffect};

use std::sync::Arc;

/// Where a part added from another effect or file came from.
#[derive(Clone, Debug, PartialEq)]
pub(super) struct PartOrigin {
    /// The file it is in, relative to the game folder.
    pub file: String,
    /// The emitter set it is in.
    pub set_name: String,
    /// Its name and flat index in that set, as the file has it.
    pub name: String,
    pub index: usize,
}

/// What adding parts from elsewhere put beside the open file's own data, kept with its undo
/// history: the steps hold parts by their pristine number and textures by their pool number, so
/// the file coming back must number them the same.
#[derive(Clone, Default)]
pub(super) struct AddedData {
    /// Each added part's (set, pristine index), origin and own values.
    parts: Vec<((usize, usize), PartOrigin, EmitterSnapshot)>,
    /// The textures brought in with them, in pool order, each with its pixels.
    textures: Vec<(TextureInfo, Vec<u8>)>,
}

/// A kind of part a modder thinks in, and the game part it starts from.
pub(super) struct Starter {
    pub label: &'static str,
    /// The name the added part gets.
    pub name: &'static str,
    /// The shared file's emitter set and the part in it: its name and flat index.
    pub set: &'static str,
    pub part: &'static str,
    pub index: usize,
}

/// The shared effects file every fighter plays, relative to the game folder.
pub(super) const SHARED_FILE_REL: &str = "effect/system/common/ef_common.eff";

/// Each a part of the shared file, which every game folder has. Picked by what each part is in
/// that file (its texture, blend and how it is drawn), and all of them stop on their own: a part
/// that never stops keeps an effect its move does not remove on screen for good.
///
/// - Glow: `sys_muzzleflash` flare1, the soft `ef_cmn_flare01` added on top, 7 frames.
/// - Sparks: `sys_slip_spark` spark1, `ef_cmn_line03` streaks stretched along their flight,
///   added on top, 2.5 a frame for 3 frames.
/// - Smoke: `sys_dash_smoke` smoke1, `ef_cmn_smoke01`, 20 frames.
/// - Ring: `sys_flash` ring1, `ef_cmn_ring05` facing the camera, 8 frames.
/// - Flash: `sys_flash` flash1, `ef_cmn_flash03`, 10 frames.
/// - Trail: `sys_recovery` spline1, the shared file's one trail stripe (`ef_cmn_line01`).
pub(super) const STARTERS: &[Starter] = &[
    Starter {
        label: "Glow",
        name: "glow",
        set: "P_CmnMuzzleflash",
        part: "flare1",
        index: 6,
    },
    Starter {
        label: "Sparks",
        name: "sparks",
        set: "P_CmnSlipSpark",
        part: "spark1",
        index: 1,
    },
    Starter {
        label: "Smoke",
        name: "smoke",
        set: "P_CmnDashSmoke",
        part: "smoke1",
        index: 3,
    },
    Starter {
        label: "Ring",
        name: "ring",
        set: "P_CmnFlash",
        part: "ring1",
        index: 1,
    },
    Starter {
        label: "Flash",
        name: "flash",
        set: "P_CmnFlash",
        part: "flash1",
        index: 2,
    },
    Starter {
        label: "Trail",
        name: "trail",
        set: "P_CmnRecovery",
        part: "spline1",
        index: 2,
    },
];

/// Another effect file, read for its parts: the shared file for the picker, or the file a saved
/// part came from.
pub(super) struct PartFile {
    pub rel: String,
    pub loaded: LoadedEffect,
    /// Its effects, as the effect list holds them.
    pub entries: Vec<EffEntry>,
    /// The picker's rows for it, made once: the file does not change.
    groups: Vec<Group>,
}

impl PartFile {
    fn read(path: &Path, rel: String) -> Result<Self, String> {
        let loaded = load_effect(path).map_err(|e| format!("{e:#}"))?;
        let entries = effect_entries(&loaded.index, loaded.ptcl.emitter_sets.len());
        let groups = groups(&entries, &loaded.ptcl);
        Ok(Self {
            rel,
            loaded,
            entries,
            groups,
        })
    }

    fn set(&self, name: &str) -> Option<&EmitterSet> {
        let sets = &self.loaded.ptcl.emitter_sets;
        sets.iter()
            .find(|s| s.name == name)
            .or_else(|| sets.iter().find(|s| s.name.eq_ignore_ascii_case(name)))
    }
}

/// The shared file, read on a thread the first time the picker opens.
#[derive(Default)]
pub(super) enum SharedFile {
    #[default]
    NotRead,
    Reading(std::sync::mpsc::Receiver<Result<PartFile, String>>),
    Read(Arc<PartFile>),
    Failed(String),
}

/// The open Add part window.
pub(super) struct AddPartPicker {
    pub set_idx: usize,
    pub search: String,
    /// Add it as a part made from the selected part.
    pub made_from: bool,
    /// Where the window opens: by the button that opened it.
    pub at: egui::Pos2,
    /// Why the last add did not happen.
    pub error: Option<String>,
}

/// A part to add.
pub(super) struct PartPick<'a> {
    /// Its values, with texture numbers in `textures`' pool.
    pub part: EmitterDef,
    /// Its file's own values for it: what an edit to it is measured from.
    pub file_part: EmitterDef,
    /// Where it is from, or None for a copy of a part of the same effect, whose `source_idx`
    /// already names its file part.
    pub origin: Option<PartOrigin>,
    /// The texture list and pool its texture numbers are in, when that is another file's.
    pub textures: Option<(&'a [TextureInfo], Option<&'a [u8]>)>,
    /// The name it gets, before it is made unique in the effect.
    pub name: String,
    /// How the step reads in Undo: "Glow", "Flare 1".
    pub label: String,
}

/// `base`, or `base` with the first number that makes it a name none of `taken` is. Parts are
/// addressed by name by every path that ships an edit, so two may not share one.
pub(super) fn unique_part_name(base: &str, taken: &[&str]) -> String {
    let is_taken = |name: &str| taken.contains(&name);
    if !base.is_empty() && !is_taken(base) {
        return base.to_owned();
    }
    let base = if base.is_empty() { "part" } else { base };
    if !is_taken(base) {
        return base.to_owned();
    }
    // "flare1" becomes "flare1_2", not "flare12".
    let join = if base.ends_with(|c: char| c.is_ascii_digit()) {
        "_"
    } else {
        ""
    };
    (2..)
        .map(|n| format!("{base}{join}{n}"))
        .find(|name| !is_taken(name))
        .unwrap_or_else(|| base.to_owned())
}

/// A name as typed, made fit for a part: spaces become `_`, characters the file cannot hold are
/// dropped, and it fits the file's 63 bytes.
pub(super) fn part_name_from_typed(typed: &str) -> String {
    let kept: String = typed
        .chars()
        .filter(|c| c.is_ascii_graphic() || *c == ' ')
        .collect();
    kept.split_whitespace()
        .collect::<Vec<_>>()
        .join("_")
        .chars()
        .take(63)
        .collect()
}

/// The colour a part's picture is tinted with: the hue of its first colour, at full brightness.
/// Many parts keep a dark colour that brightness or blending lifts, and a picture tinted with
/// it as it is reads as an empty square.
fn part_tint(em: &EmitterDef) -> egui::Color32 {
    let Some(key) = em.color0.first() else {
        return egui::Color32::WHITE;
    };
    let top = key.r.max(key.g).max(key.b);
    if top <= 0.001 || !top.is_finite() {
        return egui::Color32::WHITE;
    }
    let byte = |v: f32| ((v / top).clamp(0.0, 1.0) * 255.0).round() as u8;
    egui::Color32::from_rgb(byte(key.r), byte(key.g), byte(key.b))
}

/// One row of the picker.
struct Row {
    /// Index of the effect's set in its file, and of the part in the set.
    set_idx: usize,
    part: usize,
    label: String,
    raw: String,
    texture: Option<usize>,
    texture_name: String,
    tint: egui::Color32,
}

/// One effect of the picker, with its parts.
struct Group {
    label: String,
    raw: String,
    rows: Vec<Row>,
}

/// The picker's effects, in list order, each with its parts. After them come the sets no effect
/// is listed by: the later parts of effects linked together, under their own names.
fn groups(entries: &[EffEntry], ptcl: &PtclFile) -> Vec<Group> {
    let labels = entry_list_names(entries);
    let listed: std::collections::HashSet<usize> = entries.iter().map(|e| e.set_idx).collect();
    let linked = ptcl
        .emitter_sets
        .iter()
        .enumerate()
        .filter(|(at, set)| !listed.contains(at) && !set.emitters.is_empty())
        .map(|(at, set)| (at, set.name.clone(), set.name.clone()));
    entries
        .iter()
        .zip(labels)
        .map(|(entry, label)| (entry.set_idx, entry.name.clone(), label))
        .chain(linked)
        .filter_map(|(set_idx, raw, label)| {
            let set = ptcl.emitter_sets.get(set_idx)?;
            let names: Vec<&str> = set.emitters.iter().map(|e| e.name.as_str()).collect();
            let part_labels = part_list_names(&names);
            let rows = set
                .emitters
                .iter()
                .zip(part_labels)
                .enumerate()
                .map(|(part, (em, label))| Row {
                    set_idx,
                    part,
                    label,
                    raw: em.name.clone(),
                    texture: em
                        .texture_index
                        .map(|t| t as usize)
                        .filter(|t| *t < ptcl.bntx_textures.len()),
                    texture_name: em
                        .texture_index
                        .and_then(|t| ptcl.bntx_textures.get(t as usize))
                        .map(|t| t.tex_name.clone())
                        .unwrap_or_default(),
                    tint: part_tint(em),
                })
                .collect();
            Some(Group { label, raw, rows })
        })
        .collect()
}

/// What the picker asked for this frame.
enum Picked {
    /// A part of the open file: (set, part).
    Here(usize, usize),
    /// A part of the shared file: (set, part).
    Shared(usize, usize),
    /// A starter, by its place in [`STARTERS`].
    Starter(usize),
}

/// The most rows a search lists; the rest are counted.
const MAX_ROWS: usize = 300;

impl EffEditor {
    /// The first part of the set at or after which the file's own parts end and the parts added
    /// from elsewhere begin, in the set's pristine snapshots.
    pub(super) fn file_part_count(&self, set_idx: usize) -> usize {
        let len = self.pristine.get(set_idx).map_or(0, Vec::len);
        self.part_origins
            .keys()
            .filter(|(set, _)| *set == set_idx)
            .map(|(_, at)| *at)
            .min()
            .unwrap_or(len)
            .min(len)
    }

    /// The hover line saying where a part of the list came from, when it was added or moved.
    pub(super) fn part_note(&self, set_idx: usize, em: &EmitterDef, moved: bool) -> String {
        match self.part_origins.get(&(set_idx, em.source_idx)) {
            Some(origin) => {
                let file = if self.is_open_file(&origin.file) {
                    String::new()
                } else {
                    format!(
                        " in {}",
                        Path::new(&origin.file)
                            .file_name()
                            .map(|n| n.to_string_lossy().into_owned())
                            .unwrap_or_else(|| origin.file.clone())
                    )
                };
                format!("\nAdded from {} of {}{file}", origin.name, origin.set_name)
            }
            None => part_place_note(moved, em.source_idx),
        }
    }

    /// Whether `rel` names the open file.
    fn is_open_file(&self, rel: &str) -> bool {
        rel.is_empty()
            || self
                .loaded_rel()
                .is_some_and(|open| open.eq_ignore_ascii_case(rel))
    }

    /// Whether texture `index` came into the file with an added part, and is not one the part
    /// `snapshot` is of has of its own. It is in the written file only while a part that brings
    /// it is, so no other part is offered it.
    pub(super) fn texture_belongs_to_added_part(
        &self,
        index: usize,
        snapshot: Option<&EmitterSnapshot>,
    ) -> bool {
        let Some(info) = self.ptcl.as_ref().and_then(|p| p.bntx_textures.get(index)) else {
            return false;
        };
        if !self.carried_textures.iter().any(|(t, _)| t.id == info.id) {
            return false;
        }
        let own = snapshot.is_some_and(|s| {
            s.texture_index == Some(index as u32)
                || TEXTURE_SLOT_IDS.iter().skip(1).any(|id| {
                    crate::eff_attrs::index_of(id)
                        .and_then(|at| s.attrs.get(at).copied().flatten())
                        .map(|v| v.as_u64())
                        .is_some_and(|guid| info.id == Some(guid))
                })
        });
        !own
    }

    /// What added parts put beside the file's data, to come back with its history. None when the
    /// textures they brought are not the last in the pool: they could not be put back under the
    /// same numbers.
    pub(super) fn added_data(&self) -> Option<AddedData> {
        let pool = &self.ptcl.as_ref()?.bntx_textures;
        let tail = &pool[pool.len().checked_sub(self.carried_textures.len())?..];
        let in_place = tail
            .iter()
            .zip(&self.carried_textures)
            .all(|(a, (b, _))| a.id == b.id);
        in_place.then(|| {
            let mut parts: Vec<_> = self
                .part_origins
                .iter()
                .filter_map(|(&(set, at), origin)| {
                    let snapshot = self.pristine.get(set)?.get(at)?.clone();
                    Some(((set, at), origin.clone(), snapshot))
                })
                .collect();
            parts.sort_by_key(|(at, _, _)| *at);
            AddedData {
                parts,
                textures: self.carried_textures.clone(),
            }
        })
    }

    /// Put [`Self::added_data`] back beside a freshly loaded file. False, with nothing changed,
    /// when it does not fit it.
    pub(super) fn restore_added_data(&mut self, added: AddedData) -> bool {
        let mut pristine = self.pristine.clone();
        for ((set, at), _, snapshot) in &added.parts {
            match pristine.get_mut(*set) {
                Some(list) if list.len() == *at => list.push(snapshot.clone()),
                _ => return false,
            }
        }
        if !added.textures.is_empty() {
            let blobs = added
                .textures
                .iter()
                .map(|(_, blob)| blob.clone())
                .collect();
            let merged =
                crate::eff_export::merge_pool_textures(self.texture_pool.as_deref(), blobs);
            let (Ok(merged), Some(ptcl)) = (merged, self.ptcl.as_mut()) else {
                return false;
            };
            ptcl.bntx_textures
                .extend(added.textures.iter().map(|(info, _)| info.clone()));
            self.texture_pool = Some(merged);
            self.texture_pool_gen += 1;
        }
        self.pristine = pristine;
        self.part_origins = added
            .parts
            .into_iter()
            .map(|(at, origin, _)| (at, origin))
            .collect();
        self.carried_textures = added.textures;
        true
    }

    /// Open the Add part window for the effect playing set `set_idx`, by `at`.
    pub(super) fn open_add_part(&mut self, set_idx: usize, at: egui::Pos2) {
        self.add_part = Some(AddPartPicker {
            set_idx,
            search: String::new(),
            made_from: false,
            at,
            error: None,
        });
        self.start_reading_shared();
    }

    /// The shared file's path in the game folder.
    fn shared_path(&self) -> PathBuf {
        self.export_root.join(SHARED_FILE_REL)
    }

    /// Whether the open file is the shared file, whose parts are already "this file".
    fn open_file_is_shared(&self) -> bool {
        self.loaded_path.as_deref().is_some_and(is_shared_file)
    }

    fn start_reading_shared(&mut self) {
        if !matches!(self.shared_parts, SharedFile::NotRead) {
            return;
        }
        let path = self.shared_path();
        let (send, receive) = std::sync::mpsc::channel();
        std::thread::spawn(move || {
            let _ = send.send(PartFile::read(&path, SHARED_FILE_REL.to_owned()));
        });
        self.shared_parts = SharedFile::Reading(receive);
    }

    /// The shared file, once read. `wait` blocks until a read under way ends (a project being
    /// opened needs it now); otherwise a read under way is only looked at.
    fn shared_file(&mut self, wait: bool) -> Option<Arc<PartFile>> {
        if wait {
            self.start_reading_shared();
        }
        if let SharedFile::Reading(receive) = &self.shared_parts {
            let got = if wait {
                receive.recv().ok()
            } else {
                receive.try_recv().ok()
            };
            if let Some(result) = got {
                self.shared_parts = match result {
                    Ok(file) => SharedFile::Read(Arc::new(file)),
                    Err(e) => SharedFile::Failed(e),
                };
            }
        }
        match &self.shared_parts {
            SharedFile::Read(file) => Some(file.clone()),
            _ => None,
        }
    }

    /// A file a saved part comes from: the shared file (kept once read), or another read now.
    fn part_file(&mut self, rel: &str) -> Option<Arc<PartFile>> {
        if rel.eq_ignore_ascii_case(SHARED_FILE_REL) {
            return self.shared_file(true);
        }
        // Found as the export and the live carrier find it: a part added from a costume's own
        // effect file is in the mod's files, not in the game folder.
        let path = crate::app::find_effect_file(&self.effect_roots, rel)
            .unwrap_or_else(|| self.export_root.join(rel));
        PartFile::read(&path, rel.to_owned()).ok().map(Arc::new)
    }

    /// Make the textures `part` samples, numbered in `textures`/`pool`, ones this file has:
    /// a texture it lacks is brought into its pool. `file_part` is renumbered with it.
    fn carry_textures(
        &mut self,
        part: &mut EmitterDef,
        file_part: &mut EmitterDef,
        textures: &[TextureInfo],
        pool: Option<&[u8]>,
    ) -> Result<(), String> {
        // Slot 1 by number, slots 2 and 3 by id: every id the part names, with the donor's
        // number of it.
        let mut wanted: Vec<usize> = part.texture_index.map(|t| t as usize).into_iter().collect();
        for id in TEXTURE_SLOT_IDS.iter().skip(1) {
            if let Some(guid) = attr(part, id).map(|v| v.as_u64()) {
                if let Some(at) = textures.iter().position(|t| t.id == Some(guid)) {
                    wanted.push(at);
                }
            }
        }
        let mut error = None;
        let mut renumber: Vec<(usize, Option<usize>)> = Vec::new();
        for donor_at in wanted {
            let Some(info) = textures.get(donor_at) else {
                continue;
            };
            let here = self.ptcl.as_ref().and_then(|p| {
                p.bntx_textures.iter().position(|t| {
                    (info.id.is_some() && t.id == info.id) || t.tex_name == info.tex_name
                })
            });
            let here = match here {
                Some(at) => Some(at),
                None => match pool
                    .ok_or_else(|| "its file's textures could not be read".to_owned())
                    .and_then(|pool| {
                        crate::eff_export::extract_pool_texture(pool, donor_at, &info.tex_name)
                            .and_then(|blob| {
                                let merged = crate::eff_export::merge_pool_textures(
                                    self.texture_pool.as_deref(),
                                    vec![blob.clone()],
                                )?;
                                Ok((merged, blob))
                            })
                            .map_err(|e| format!("{e:#}"))
                    }) {
                    Ok((merged, blob)) => {
                        self.carried_textures.push((info.clone(), blob));
                        self.texture_pool = Some(merged);
                        self.texture_pool_gen += 1;
                        let ptcl = self.ptcl.as_mut().ok_or("no file open")?;
                        ptcl.bntx_textures.push(info.clone());
                        Some(ptcl.bntx_textures.len() - 1)
                    }
                    Err(e) => {
                        error = Some(format!("texture {}: {e}", info.tex_name));
                        None
                    }
                },
            };
            renumber.push((donor_at, here));
        }
        for em in [part, file_part] {
            if let Some(t) = em.texture_index {
                em.texture_index = renumber
                    .iter()
                    .find(|(from, _)| *from == t as usize)
                    .and_then(|(_, to)| *to)
                    .map(|to| to as u32);
            }
        }
        error.map_or(Ok(()), Err)
    }

    /// Add `pick` to the effect playing `set_idx`: after the selected part and what is made from
    /// it, on its own or (`made_from`) made from the selected part. Selects it and returns where
    /// it went.
    pub(super) fn add_part(
        &mut self,
        set_idx: usize,
        pick: PartPick,
        made_from: bool,
    ) -> Result<usize, String> {
        let PartPick {
            mut part,
            mut file_part,
            origin,
            textures,
            name,
            label,
        } = pick;
        // Its own step, apart from an edit still open.
        self.history_close_step();
        let (at, depth) = {
            let set = self
                .ptcl
                .as_ref()
                .and_then(|p| p.emitter_sets.get(set_idx))
                .ok_or("no effect open")?;
            let depths: Vec<u8> = set.emitters.iter().map(|e| e.depth).collect();
            let selected = self.selected_emitter.min(depths.len().saturating_sub(1));
            if depths.is_empty() {
                (0, 0)
            } else if made_from {
                if depths[selected] != 0
                    || children_of(&depths, selected) >= crate::eff_validate::MAX_CHILDREN
                {
                    return Err("The selected part cannot make another part".into());
                }
                (subtree(&set.emitters, selected).end, 1)
            } else {
                let root = (0..=selected)
                    .rev()
                    .find(|&k| depths[k] == 0)
                    .unwrap_or(selected);
                (subtree(&set.emitters, root).end, 0)
            }
        };
        if let Some(origin) = origin {
            if let Some((list, pool)) = textures {
                self.carry_textures(&mut part, &mut file_part, list, pool)?;
            }
            let existing = self
                .part_origins
                .iter()
                .find(|((set, _), o)| *set == set_idx && **o == origin)
                .map(|((_, at), _)| *at);
            part.source_idx = match existing {
                Some(at) => at,
                None => {
                    let snapshots = self.pristine.get_mut(set_idx).ok_or("no effect open")?;
                    snapshots.push(EmitterSnapshot::of(&file_part));
                    let at = snapshots.len() - 1;
                    self.part_origins.insert((set_idx, at), origin);
                    at
                }
            };
        }
        let set = self
            .ptcl
            .as_mut()
            .and_then(|p| p.emitter_sets.get_mut(set_idx))
            .ok_or("no effect open")?;
        let taken: Vec<&str> = set.emitters.iter().map(|e| e.name.as_str()).collect();
        part.name = unique_part_name(&name, &taken);
        part.depth = depth;
        set.emitters.insert(at, part);
        self.selected_emitter = at;
        self.touch();
        let effect = self
            .selected_entry
            .and_then(|i| entry_list_names(&self.entries).get(i).cloned())
            .unwrap_or_default();
        self.history.next_label = Some(format!("Add {label} to {effect}"));
        Ok(at)
    }

    /// Where a part of a borrowed effect is from: the file it was borrowed from. The game's own
    /// file, which the live send reads parts out of, does not hold the borrowed copy.
    fn borrowed_origin(&mut self, set_idx: usize, part: &str, index: usize) -> Option<PartOrigin> {
        let entry = self
            .entries
            .iter()
            .find(|e| e.set_idx == set_idx)?
            .name
            .clone();
        let (_, item) = self.current_transplant_for_entry(&entry)?;
        let file = self.part_file(&item.donor_file)?;
        let donor = file
            .entries
            .iter()
            .find(|e| e.name.eq_ignore_ascii_case(&item.donor_name))?;
        let set = file.loaded.ptcl.emitter_sets.get(donor.set_idx)?;
        (set.emitters.get(index)?.name == part).then(|| PartOrigin {
            file: file.rel.clone(),
            set_name: set.name.clone(),
            name: part.to_owned(),
            index,
        })
    }

    /// A part of the open file, as a pick for the effect playing `into`.
    fn pick_here(&mut self, into: usize, set_idx: usize, part: usize) -> Option<PartPick<'static>> {
        let set = self.ptcl.as_ref()?.emitter_sets.get(set_idx)?;
        let set_name = set.name.clone();
        let em = set.emitters.get(part)?.clone();
        let label = part_list_names(
            &set.emitters
                .iter()
                .map(|e| e.name.as_str())
                .collect::<Vec<_>>(),
        )
        .get(part)
        .cloned()
        .unwrap_or_default();
        let file_part = self
            .pristine
            .get(set_idx)?
            .get(em.source_idx)?
            .to_def(em.source_idx);
        let origin = match self.part_origins.get(&(set_idx, em.source_idx)) {
            // Itself added from elsewhere: from where it came from.
            Some(origin) => Some(origin.clone()),
            None if set_idx == into => None,
            None => Some(
                self.borrowed_origin(set_idx, &file_part.name, em.source_idx)
                    .unwrap_or(PartOrigin {
                        file: self.loaded_rel().unwrap_or_default(),
                        set_name,
                        name: file_part.name.clone(),
                        index: em.source_idx,
                    }),
            ),
        };
        Some(PartPick {
            name: em.name.clone(),
            part: em,
            file_part,
            origin,
            // The open file's own numbers.
            textures: None,
            label,
        })
    }

    /// Run the picker's choice.
    fn add_picked(&mut self, picked: Picked) {
        let Some(picker) = self.add_part.as_ref() else {
            return;
        };
        let (into, made_from) = (picker.set_idx, picker.made_from);
        let result = match picked {
            Picked::Here(set_idx, part) => match self.pick_here(into, set_idx, part) {
                Some(pick) => self.add_part(into, pick, made_from),
                None => Err("That part is gone".into()),
            },
            Picked::Shared(set_idx, part) => match self.shared_file(false) {
                Some(file) => {
                    let set = &file.loaded.ptcl.emitter_sets[set_idx];
                    let em = set.emitters[part].clone();
                    let label = part_list_names(
                        &set.emitters
                            .iter()
                            .map(|e| e.name.as_str())
                            .collect::<Vec<_>>(),
                    )
                    .get(part)
                    .cloned()
                    .unwrap_or_default();
                    let pick = PartPick {
                        name: em.name.clone(),
                        file_part: em.clone(),
                        origin: Some(PartOrigin {
                            file: file.rel.clone(),
                            set_name: set.name.clone(),
                            name: em.name.clone(),
                            index: part,
                        }),
                        part: em,
                        textures: Some((
                            &file.loaded.ptcl.bntx_textures,
                            file.loaded.texture_pool.as_deref(),
                        )),
                        label,
                    };
                    self.add_part(into, pick, made_from)
                }
                None => Err("The shared effects are not read yet".into()),
            },
            Picked::Starter(i) => self.add_starter(into, &STARTERS[i], made_from),
        };
        match result {
            Ok(_) => self.add_part = None,
            Err(e) => {
                if let Some(picker) = self.add_part.as_mut() {
                    picker.error = Some(e);
                }
            }
        }
    }

    /// Add a starter to the effect playing `into`.
    pub(super) fn add_starter(
        &mut self,
        into: usize,
        starter: &Starter,
        made_from: bool,
    ) -> Result<usize, String> {
        // From this file when it is the shared one: it is already here.
        if self.open_file_is_shared() {
            let set_idx = self
                .ptcl
                .as_ref()
                .and_then(|p| p.emitter_sets.iter().position(|s| s.name == starter.set))
                .ok_or("This file has no such part")?;
            let mut pick = self
                .pick_here(into, set_idx, starter.index)
                .ok_or("This file has no such part")?;
            pick.name = starter.name.to_owned();
            pick.label = starter.label.to_owned();
            return self.add_part(into, pick, made_from);
        }
        let file = self
            .shared_file(true)
            .ok_or("The shared effects file could not be read")?;
        let set = file
            .set(starter.set)
            .ok_or("The shared file has no such part")?;
        let em = set
            .emitters
            .get(starter.index)
            .filter(|em| em.name == starter.part)
            .ok_or("The shared file has no such part")?
            .clone();
        let pick = PartPick {
            name: starter.name.to_owned(),
            file_part: em.clone(),
            origin: Some(PartOrigin {
                file: file.rel.clone(),
                set_name: set.name.clone(),
                name: em.name.clone(),
                index: starter.index,
            }),
            part: em,
            textures: Some((
                &file.loaded.ptcl.bntx_textures,
                file.loaded.texture_pool.as_deref(),
            )),
            label: starter.label.to_owned(),
        };
        self.add_part(into, pick, made_from)
    }

    /// A saved part added from another effect or file, as the slot records it: its file's own
    /// values, and its pristine number in `set_idx` (added on first use). None when its effect,
    /// file or part is gone.
    pub(super) fn foreign_slot_part(
        &mut self,
        set_idx: usize,
        slot: &crate::mod_project::EmitterSlot,
    ) -> Option<EmitterDef> {
        let origin = PartOrigin {
            file: slot.source_file.clone(),
            set_name: slot.source_set.clone(),
            name: slot.source_name.clone(),
            index: slot.source_idx,
        };
        if let Some((&(_, at), _)) = self
            .part_origins
            .iter()
            .find(|((set, _), o)| *set == set_idx && **o == origin)
        {
            return Some(self.pristine.get(set_idx)?.get(at)?.to_def(at));
        }
        let resolve = |emitters: &[EmitterDef]| {
            let named = |i: usize| {
                !origin.name.is_empty() && emitters.get(i).is_some_and(|e| e.name == origin.name)
            };
            if named(origin.index) {
                Some(origin.index)
            } else {
                (0..emitters.len())
                    .find(|i| named(*i))
                    .or((origin.name.is_empty() && origin.index < emitters.len())
                        .then_some(origin.index))
            }
        };
        let mut pick = if self.is_open_file(&origin.file) {
            let from = self
                .ptcl
                .as_ref()?
                .emitter_sets
                .iter()
                .position(|s| s.name == origin.set_name)?;
            let count = self.file_part_count(from);
            let snapshots = &self.pristine.get(from)?[..count];
            let defs: Vec<EmitterDef> = snapshots
                .iter()
                .enumerate()
                .map(|(i, s)| s.to_def(i))
                .collect();
            let at = resolve(&defs)?;
            let em = defs[at].clone();
            PartPick {
                name: em.name.clone(),
                file_part: em.clone(),
                part: em,
                origin: Some(origin),
                textures: None,
                label: String::new(),
            }
        } else {
            let file = self.part_file(&origin.file)?;
            let set = file.set(&origin.set_name)?;
            let at = resolve(&set.emitters)?;
            let em = set.emitters[at].clone();
            let mut part = em.clone();
            let mut file_part = em;
            self.carry_textures(
                &mut part,
                &mut file_part,
                &file.loaded.ptcl.bntx_textures,
                file.loaded.texture_pool.as_deref(),
            )
            .ok();
            PartPick {
                name: part.name.clone(),
                part,
                file_part,
                origin: Some(origin),
                textures: None,
                label: String::new(),
            }
        };
        let origin = pick.origin.take()?;
        let snapshots = self.pristine.get_mut(set_idx)?;
        snapshots.push(EmitterSnapshot::of(&pick.file_part));
        let at = snapshots.len() - 1;
        self.part_origins.insert((set_idx, at), origin);
        let mut em = pick.part;
        em.source_idx = at;
        Some(em)
    }

    /// The Add part window, while it is open.
    pub(super) fn draw_add_part(&mut self, ctx: &egui::Context) {
        let Some(picker) = self.add_part.as_ref() else {
            return;
        };
        let set_idx = picker.set_idx;
        let at = picker.at;
        // It adds to the effect it was opened on: another effect opened closes it.
        if self
            .selected_entry
            .and_then(|i| self.entries.get(i))
            .map(|e| e.set_idx)
            != Some(set_idx)
        {
            self.add_part = None;
            return;
        }
        let open_is_shared = self.open_file_is_shared();
        let shared = if open_is_shared {
            None
        } else {
            let file = self.shared_file(false);
            if file.is_none() && matches!(self.shared_parts, SharedFile::Reading(_)) {
                ctx.request_repaint_after(std::time::Duration::from_millis(100));
            }
            file
        };
        let shared_error = match &self.shared_parts {
            SharedFile::Failed(e) => Some(e.clone()),
            _ => None,
        };
        let Some(ptcl) = self.ptcl.as_ref() else {
            self.add_part = None;
            return;
        };
        let mut here = groups(&self.entries, ptcl);
        // The open effect's own parts first.
        if let Some(at) = here
            .iter()
            .position(|g| g.rows.first().is_some_and(|r| r.set_idx == set_idx))
        {
            let open = here.remove(at);
            here.insert(0, open);
        }
        let open_set = self
            .entries
            .get(self.selected_entry.unwrap_or(usize::MAX))
            .map(|e| e.set_idx);
        let selected_part = ptcl.emitter_sets.get(set_idx).and_then(|set| {
            let names: Vec<&str> = set.emitters.iter().map(|e| e.name.as_str()).collect();
            let depths: Vec<u8> = set.emitters.iter().map(|e| e.depth).collect();
            let i = self.selected_emitter.min(names.len().checked_sub(1)?);
            let can = depths[i] == 0 && children_of(&depths, i) < crate::eff_validate::MAX_CHILDREN;
            Some((part_list_names(&names)[i].clone(), can))
        });

        let replaced: Vec<(String, String)> = self
            .texture_imports
            .iter()
            .filter(|t| !t.png_path.is_empty())
            .map(|t| (t.texture_name.clone(), t.png_path.clone()))
            .collect();
        self.part_thumbs.keep_for((
            self.loaded_path.clone(),
            self.texture_pool_gen,
            replaced.clone(),
        ));
        if let Some(file) = &shared {
            self.shared_thumbs
                .keep_for((Some(PathBuf::from(&file.rel)), 0, Vec::new()));
        }
        let here_pool = self.texture_pool.as_deref();
        let here_names: Vec<String> = ptcl
            .bntx_textures
            .iter()
            .map(|t| t.tex_name.clone())
            .collect();
        let part_thumbs = &mut self.part_thumbs;
        let shared_thumbs = &mut self.shared_thumbs;
        let Some(picker) = self.add_part.as_mut() else {
            return;
        };
        let mut open = true;
        let mut picked: Option<Picked> = None;
        egui::Window::new("Add part")
            .open(&mut open)
            .collapsible(false)
            .resizable(true)
            .default_pos(at)
            .default_size([380.0, 520.0])
            .show(ctx, |ui| {
                ui.horizontal_wrapped(|ui| {
                    for (i, starter) in STARTERS.iter().enumerate() {
                        if ui
                            .button(starter.label)
                            .on_hover_text(format!(
                                "{} of the shared effects ({})",
                                starter.part, starter.set
                            ))
                            .clicked()
                        {
                            picked = Some(Picked::Starter(i));
                        }
                    }
                });
                if let Some((name, can)) = &selected_part {
                    ui.horizontal_wrapped(|ui| {
                        ui.radio_value(&mut picker.made_from, false, "On its own")
                            .on_hover_text("A part of the effect like the others");
                        let made = ui.add_enabled(
                            *can,
                            egui::RadioButton::new(picker.made_from, format!("Made from {name}")),
                        );
                        if made.clicked() {
                            picker.made_from = true;
                        }
                        made.on_hover_text(format!(
                            "Its particles come out of the particles of {name}"
                        ))
                        .on_disabled_hover_text(
                            "Only a part on its own can make parts, and one part makes at most 16",
                        );
                    });
                    if !*can {
                        picker.made_from = false;
                    }
                }
                if let Some(error) = &picker.error {
                    ui.colored_label(egui::Color32::from_rgb(230, 110, 110), error);
                }
                let search = ui.add(
                    egui::TextEdit::singleline(&mut picker.search)
                        .hint_text("Search parts")
                        .desired_width(f32::INFINITY),
                );
                if !search.has_focus() && !ui.memory(|m| m.focused().is_some()) {
                    search.request_focus();
                }
                let query = picker.search.clone();
                let mut shown = 0usize;
                let mut more = 0usize;
                egui::ScrollArea::vertical()
                    .auto_shrink([false, false])
                    .show(ui, |ui| {
                        let mut list =
                            |ui: &mut Ui,
                             groups: &[Group],
                             thumbs: &mut TextureThumbs,
                             pool: Option<&[u8]>,
                             names: &[String],
                             shared: bool,
                             open_set: Option<usize>| {
                                for group in groups {
                                    let rows: Vec<&Row> = group
                                        .rows
                                        .iter()
                                        .filter(|row| {
                                            search_rank(
                                                &query,
                                                &[
                                                    &row.label,
                                                    &row.raw,
                                                    &group.label,
                                                    &group.raw,
                                                    &row.texture_name,
                                                ],
                                            )
                                            .is_some()
                                        })
                                        .collect();
                                    if rows.is_empty() {
                                        continue;
                                    }
                                    if shown >= MAX_ROWS {
                                        more += rows.len();
                                        continue;
                                    }
                                    let is_open =
                                        !shared && open_set == rows.first().map(|r| r.set_idx);
                                    let header = egui::CollapsingHeader::new(format!(
                                        "{} ({})",
                                        group.label,
                                        rows.len()
                                    ))
                                    .id_salt(("add_part_group", shared, &group.raw))
                                    .default_open(is_open)
                                    .open((!query.trim().is_empty()).then_some(true));
                                    let response = header.show(ui, |ui| {
                                        for row in rows {
                                            shown += 1;
                                            // One row, picture and name, as the texture picker's.
                                            let background = ui.painter().add(egui::Shape::Noop);
                                            let drawn =
                                                ui.horizontal(|ui| {
                                                    let index = row.texture.unwrap_or(usize::MAX);
                                                    let name =
                                                        names.get(index).map_or("", String::as_str);
                                                    let png = (!shared)
                                                        .then(|| {
                                                            replaced.iter().find(|(t, _)| t == name)
                                                        })
                                                        .flatten();
                                                    thumbs.draw_tinted(ui, index, row.tint, || {
                                                        match png {
                                                Some((_, png)) => {
                                                    let png = std::fs::read(png).ok()?;
                                                    crate::texture_import::decode_png_rgba(
                                                        &png,
                                                        Some(THUMB),
                                                    )
                                                    .ok()
                                                }
                                                None => crate::texture_import::decode_preview(
                                                    pool?,
                                                    row.texture?,
                                                    name,
                                                    crate::texture_import::Form::Editable,
                                                    Some(THUMB),
                                                )
                                                .ok(),
                                            }
                                                    });
                                                    ui.add(
                                                        egui::Label::new(&row.label)
                                                            .selectable(false),
                                                    );
                                                });
                                            let rect = drawn
                                                .response
                                                .rect
                                                .with_max_x(ui.max_rect().right());
                                            let response = ui.interact(
                                                rect,
                                                ui.scope_id().with((
                                                    "add_part_row",
                                                    shared,
                                                    &group.raw,
                                                    row.part,
                                                )),
                                                egui::Sense::click(),
                                            );
                                            if response.hovered() {
                                                let look = ui.style().interact(&response);
                                                ui.painter().set(
                                                    background,
                                                    egui::epaint::RectShape::filled(
                                                        rect.expand(1.0),
                                                        look.corner_radius,
                                                        look.weak_bg_fill,
                                                    ),
                                                );
                                            }
                                            let response = response.on_hover_text(format!(
                                                "{} of {}{}",
                                                row.raw,
                                                group.raw,
                                                if shared { ", shared effects" } else { "" }
                                            ));
                                            if response.clicked() {
                                                picked = Some(if shared {
                                                    Picked::Shared(row.set_idx, row.part)
                                                } else {
                                                    Picked::Here(row.set_idx, row.part)
                                                });
                                            }
                                        }
                                    });
                                    response.header_response.on_hover_text(&group.raw);
                                }
                            };
                        list(
                            ui,
                            &here,
                            part_thumbs,
                            here_pool,
                            &here_names,
                            false,
                            open_set,
                        );
                        if let Some(there) = shared.as_ref().map(|f| &f.groups) {
                            let names: Vec<String> = shared
                                .as_ref()
                                .map(|f| {
                                    f.loaded
                                        .ptcl
                                        .bntx_textures
                                        .iter()
                                        .map(|t| t.tex_name.clone())
                                        .collect()
                                })
                                .unwrap_or_default();
                            let pool = shared
                                .as_ref()
                                .and_then(|f| f.loaded.texture_pool.as_deref());
                            ui.add_space(6.0);
                            ui.label(
                                egui::RichText::new("Shared effects")
                                    .strong()
                                    .color(SHARED_TINT),
                            )
                            .on_hover_text(SHARED_EFFECT_NOTE);
                            list(ui, there, shared_thumbs, pool, &names, true, None);
                        } else if let Some(error) = &shared_error {
                            ui.colored_label(
                                egui::Color32::from_rgb(230, 110, 110),
                                format!("Shared effects: {error}"),
                            );
                        } else if !open_is_shared {
                            ui.weak("Reading the shared effects");
                        }
                        if more > 0 {
                            ui.weak(format!("{more} more"));
                        }
                    });
            });
        if !open {
            self.add_part = None;
            return;
        }
        if let Some(picked) = picked {
            self.add_picked(picked);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::effects::EmitterSet;
    #[allow(unused_imports)]
    use crate::ui::TestFrame;

    /// A part named `name`, with every attribute present and the tuned one at `scale`.
    fn part(name: &str, source_idx: usize, scale: f32) -> EmitterDef {
        let mut em = EmitterDef {
            name: name.into(),
            attrs: crate::eff_attrs::read_all(&crate::eff_attrs::blank_emitter_data(
                crate::eff_attrs::SSBU_VFX_VERSION,
            )),
            subsections: Vec::new(),
            depth: 0,
            source_idx,
            color0: Vec::new(),
            color1: Vec::new(),
            alpha0_keys: Vec::new(),
            texture_index: None,
        };
        set_attr(&mut em, "particle_scale.scale_x", AttrValue::Float(scale));
        em
    }

    /// Two effects: "dash" playing P_A (a0, a1) and "jump" playing P_B (b0), "dash" open on a0.
    fn two_effects() -> EffEditor {
        let a = vec![part("a0", 0, 1.0), part("a1", 1, 1.0)];
        let b = vec![part("b0", 0, 3.0)];
        let snapshots = |list: &[EmitterDef]| list.iter().map(EmitterSnapshot::of).collect();
        EffEditor {
            pristine: vec![snapshots(&a), snapshots(&b)],
            ptcl: Some(PtclFile {
                emitter_sets: vec![
                    EmitterSet {
                        name: "P_A".into(),
                        emitters: a,
                    },
                    EmitterSet {
                        name: "P_B".into(),
                        emitters: b,
                    },
                ],
                bntx_textures: Vec::new(),
            }),
            entries: vec![
                EffEntry {
                    name: "dash".into(),
                    hash: 0,
                    set_idx: 0,
                },
                EffEntry {
                    name: "jump".into(),
                    hash: 0,
                    set_idx: 1,
                },
            ],
            selected_entry: Some(0),
            loaded_path: Some(PathBuf::from("/fx/ef_test.eff")),
            ..Default::default()
        }
    }

    fn names(editor: &EffEditor, set: usize) -> Vec<String> {
        editor.ptcl.as_ref().unwrap().emitter_sets[set]
            .emitters
            .iter()
            .map(|e| format!("{}@{}", e.name, e.depth))
            .collect()
    }

    /// Add part b0 of "jump" to "dash" through the picker's own path.
    fn add_b0(editor: &mut EffEditor, made_from: bool) {
        editor.open_add_part(0, egui::Pos2::ZERO);
        editor.add_part.as_mut().unwrap().made_from = made_from;
        editor.add_picked(Picked::Here(1, 0));
    }

    #[test]
    fn names_are_made_unique_and_fit_for_the_file() {
        assert_eq!(unique_part_name("glow", &["a0"]), "glow");
        assert_eq!(unique_part_name("glow", &["glow"]), "glow2");
        assert_eq!(unique_part_name("glow", &["glow", "glow2"]), "glow3");
        // A name ending in a number keeps it apart from the count.
        assert_eq!(unique_part_name("flare1", &["flare1"]), "flare1_2");
        assert_eq!(unique_part_name("", &[]), "part");
        assert_eq!(part_name_from_typed("  big glow é "), "big_glow");
        assert_eq!(part_name_from_typed(&"x".repeat(80)).len(), 63);
    }

    /// The added part goes after the open part, is selected, keeps its name, plays the other
    /// effect's values, and is saved as a slot naming that effect.
    #[test]
    fn a_part_from_another_effect_is_added_selected_and_recorded() {
        let mut editor = two_effects();
        assert!(editor.collect_rosters().is_empty());
        add_b0(&mut editor, false);
        assert!(editor.add_part.is_none(), "the window closes on a pick");
        assert_eq!(names(&editor, 0), ["a0@0", "b0@0", "a1@0"]);
        assert_eq!(editor.selected_emitter, 1);
        let added = &editor.ptcl.as_ref().unwrap().emitter_sets[0].emitters[1];
        assert_eq!(attr_f32(added, "particle_scale.scale_x"), 3.0);
        assert_eq!(editor.edited_entries(), vec![true, false]);
        // Not a value edit: the part has its file's values.
        assert!(editor.edited_emitters(0).is_empty());
        assert!(editor.collect_authored_edits().is_empty());
        assert_eq!(editor.moved_parts(0), vec![false, true, false]);
        let rosters = editor.collect_rosters();
        assert_eq!(rosters.len(), 1);
        let slot = &rosters[0].slots[1];
        assert_eq!(
            (slot.source_set.as_str(), slot.source_file.as_str()),
            ("P_B", "effect/fighter/test/ef_test.eff")
        );
        assert_eq!((slot.source_name.as_str(), slot.source_idx), ("b0", 0));
        assert_eq!(slot.name, "b0");
        // Its file's own parts are still plain clones.
        assert!(!rosters[0].slots[0].is_foreign() && !rosters[0].slots[2].is_foreign());

        // Again: a second name, the same file part behind it.
        add_b0(&mut editor, false);
        assert_eq!(names(&editor, 0), ["a0@0", "b0@0", "b0_2@0", "a1@0"]);
        assert_eq!(
            editor.pristine[0].len(),
            3,
            "one snapshot of b0, shared by both copies"
        );
    }

    /// An edit to the added part is measured from the part it came from, and ships under the
    /// added part's name.
    #[test]
    fn an_added_part_is_edited_against_its_source() {
        let mut editor = two_effects();
        add_b0(&mut editor, false);
        let added = &mut editor.ptcl.as_mut().unwrap().emitter_sets[0].emitters[1];
        set_attr(added, "particle_scale.scale_x", AttrValue::Float(5.0));
        let edits = editor.collect_authored_edits();
        assert_eq!(edits.len(), 1);
        assert_eq!(
            (edits[0].set_name.as_str(), edits[0].emitter_name.as_str()),
            ("P_A", "b0")
        );
        // Reset this part puts it back to b0's values, not a0's.
        editor.reset_part_fields(0, 1);
        let added = &editor.ptcl.as_ref().unwrap().emitter_sets[0].emitters[1];
        assert_eq!(attr_f32(added, "particle_scale.scale_x"), 3.0);
        assert!(editor.collect_authored_edits().is_empty());
    }

    /// A part picked with edits on it comes with them, as edits of the added part.
    #[test]
    fn an_edited_part_is_added_as_it_looks() {
        let mut editor = two_effects();
        let b0 = &mut editor.ptcl.as_mut().unwrap().emitter_sets[1].emitters[0];
        set_attr(b0, "particle_scale.scale_x", AttrValue::Float(9.0));
        add_b0(&mut editor, false);
        let added = &editor.ptcl.as_ref().unwrap().emitter_sets[0].emitters[1];
        assert_eq!(attr_f32(added, "particle_scale.scale_x"), 9.0);
        let edits = editor.collect_authored_edits();
        assert_eq!(
            edits
                .iter()
                .map(|e| e.set_name.as_str())
                .collect::<Vec<_>>(),
            ["P_A", "P_B"]
        );
    }

    /// An edit still open is its own step: undoing the add leaves it.
    #[test]
    fn an_edit_just_before_an_add_is_its_own_step() {
        let t0 = Instant::now();
        let mut editor = two_effects();
        editor.history_begin_frame(false, t0);
        let a0 = &mut editor.ptcl.as_mut().unwrap().emitter_sets[0].emitters[0];
        set_attr(a0, "particle_scale.scale_x", AttrValue::Float(2.0));
        editor.touch();
        editor.history_note_edit(t0);
        editor.history_begin_frame(false, t0);
        add_b0(&mut editor, false);
        editor.history_note_edit(t0);
        editor.undo();
        assert_eq!(names(&editor, 0), ["a0@0", "a1@0"]);
        let a0 = &editor.ptcl.as_ref().unwrap().emitter_sets[0].emitters[0];
        assert_eq!(attr_f32(a0, "particle_scale.scale_x"), 2.0);
        assert!(editor.can_undo(), "the edit before it");
    }

    /// The picker lists every part of the file, those of sets no effect is listed by too.
    #[test]
    fn the_picker_lists_sets_no_effect_names() {
        let mut editor = two_effects();
        editor.entries.truncate(1);
        let groups = groups(&editor.entries, editor.ptcl.as_ref().unwrap());
        let listed: Vec<_> = groups
            .iter()
            .map(|g| (g.label.as_str(), g.rows.len()))
            .collect();
        assert_eq!(listed, [("dash", 2), ("P_B", 1)]);
    }

    /// The window draws, and clicking a row anywhere (its picture too) adds that part and closes
    /// it.
    #[test]
    fn clicking_a_row_adds_its_part() {
        let mut editor = two_effects();
        editor.shared_parts = SharedFile::Failed("none here".into());
        editor.open_add_part(0, egui::pos2(10.0, 10.0));
        editor.add_part.as_mut().unwrap().search = "b0".into();
        let context = egui::Context::default();
        let frame = |editor: &mut EffEditor, events: Vec<egui::Event>| {
            context.run_test_ui(
                egui::RawInput {
                    screen_rect: Some(egui::Rect::from_min_size(
                        egui::Pos2::ZERO,
                        egui::vec2(800.0, 800.0),
                    )),
                    events,
                    ..Default::default()
                },
                |ui| editor.draw_add_part(ui.ctx()),
            )
        };
        for _ in 0..3 {
            frame(&mut editor, Vec::new());
        }
        let output = frame(&mut editor, Vec::new());
        fn find(shape: &egui::epaint::Shape, label: &str) -> Option<egui::Pos2> {
            match shape {
                egui::epaint::Shape::Text(t) if t.galley.job.text == label => Some(t.pos),
                egui::epaint::Shape::Vec(v) => v.iter().find_map(|s| find(s, label)),
                _ => None,
            }
        }
        let text = output
            .shapes
            .iter()
            .find_map(|s| find(&s.shape, "B 0"))
            .expect("the row for b0");
        // On the picture, left of the name.
        let at = text - egui::vec2(20.0, 0.0);
        for pressed in [true, false] {
            frame(
                &mut editor,
                vec![
                    egui::Event::PointerMoved(at),
                    egui::Event::PointerButton {
                        pos: at,
                        button: egui::PointerButton::Primary,
                        pressed,
                        modifiers: egui::Modifiers::NONE,
                    },
                ],
            );
        }
        assert_eq!(names(&editor, 0), ["a0@0", "b0@0", "a1@0"]);
        assert!(editor.add_part.is_none());
    }

    /// Made from the open part: one level down, after what it already makes; refused under a
    /// part that is itself made from another.
    #[test]
    fn a_part_can_be_made_from_the_selected_part() {
        let mut editor = two_effects();
        add_b0(&mut editor, true);
        assert_eq!(names(&editor, 0), ["a0@0", "b0@1", "a1@0"]);
        assert_eq!(editor.collect_rosters()[0].slots[1].depth, 1);
        // The child is selected now; a part made from it is not possible.
        assert_eq!(editor.selected_emitter, 1);
        add_b0(&mut editor, true);
        assert!(editor.add_part.as_ref().unwrap().error.is_some());
        assert_eq!(names(&editor, 0), ["a0@0", "b0@1", "a1@0"]);
        // On its own from the child: after its parent's group, at the top level.
        editor.add_part.as_mut().unwrap().made_from = false;
        editor.add_picked(Picked::Here(1, 0));
        assert_eq!(names(&editor, 0), ["a0@0", "b0@1", "b0_2@0", "a1@0"]);
    }

    /// One undo step takes it out and one redo puts it back; Reset effect takes it out too.
    #[test]
    fn adding_is_one_undo_step_and_reset_removes_it() {
        let t0 = Instant::now();
        let ms = |n: u64| t0 + std::time::Duration::from_millis(n);
        let mut editor = two_effects();
        editor.history_begin_frame(false, ms(0));
        let rev = editor.edit_rev;
        add_b0(&mut editor, false);
        assert_ne!(editor.edit_rev, rev);
        editor.history_note_edit(ms(0));
        editor.history_begin_frame(false, ms(1000));
        assert_eq!(editor.undo_label().as_deref(), Some("Add B 0 to dash"));
        editor.undo();
        assert_eq!(names(&editor, 0), ["a0@0", "a1@0"]);
        assert!(editor.collect_rosters().is_empty());
        editor.redo();
        assert_eq!(names(&editor, 0), ["a0@0", "b0@0", "a1@0"]);
        assert_eq!(editor.collect_rosters()[0].slots[1].source_set, "P_B");

        editor.reset_effect(0, "dash");
        assert_eq!(names(&editor, 0), ["a0@0", "a1@0"]);
        assert!(editor.collect_rosters().is_empty());
        assert_eq!(editor.edited_entries(), vec![false, false]);
    }

    /// A saved list with an added part opens again as it was saved, and saves the same.
    #[test]
    fn a_saved_added_part_comes_back_on_reload() {
        let mut editor = two_effects();
        add_b0(&mut editor, true);
        let added = &mut editor.ptcl.as_mut().unwrap().emitter_sets[0].emitters[1];
        set_attr(added, "particle_scale.scale_x", AttrValue::Float(7.0));
        let rosters = editor.collect_rosters();
        let edits = editor.collect_authored_edits();
        // Through the project file, as a reload reads it.
        let json = serde_json::to_string(&rosters).unwrap();
        let rosters: Vec<crate::mod_project::EmitterRoster> = serde_json::from_str(&json).unwrap();

        let mut reloaded = two_effects();
        reloaded.apply_structure_edits(&rosters, &[]);
        reloaded.apply_authored_edits(&edits);
        assert_eq!(names(&reloaded, 0), ["a0@0", "b0@1", "a1@0"]);
        let part = &reloaded.ptcl.as_ref().unwrap().emitter_sets[0].emitters[1];
        assert_eq!(attr_f32(part, "particle_scale.scale_x"), 7.0);
        assert_eq!(reloaded.collect_rosters(), rosters);
        assert_eq!(
            serde_json::to_string(&reloaded.collect_authored_edits()).unwrap(),
            serde_json::to_string(&edits).unwrap()
        );
        // Applied twice (a second project over the same file): no second copy of anything.
        reloaded.apply_structure_edits(&rosters, &[]);
        assert_eq!(names(&reloaded, 0), ["a0@0", "b0@1", "a1@0"]);
        assert_eq!(reloaded.pristine[0].len(), 3);
    }

    /// A saved part whose source is not here is shown as no other part, its edits land on none,
    /// and both are saved again as they were. Reset effect takes them out.
    #[test]
    fn a_part_whose_source_is_missing_is_kept_in_the_project() {
        let mut editor = two_effects();
        add_b0(&mut editor, false);
        let added = &mut editor.ptcl.as_mut().unwrap().emitter_sets[0].emitters[1];
        set_attr(added, "particle_scale.scale_x", AttrValue::Float(7.0));
        let mut rosters = editor.collect_rosters();
        rosters[0].slots[1].source_set = "P_Missing".into();
        let edits = editor.collect_authored_edits();
        let text = |e: &EffEditor| serde_json::to_string(&e.collect_authored_edits()).unwrap();

        let mut other = two_effects();
        other.apply_structure_edits(&rosters, &[]);
        other.apply_authored_edits(&edits);
        assert_eq!(names(&other, 0), ["a0@0", "a1@0"]);
        assert!(other.edited_emitters(0).is_empty(), "no part has its edit");
        assert_eq!(other.edited_entries(), vec![true, false]);
        assert_eq!(other.collect_rosters(), rosters);
        assert_eq!(text(&other), serde_json::to_string(&edits).unwrap());
        // Applied again: kept once.
        other.apply_structure_edits(&rosters, &[]);
        other.apply_authored_edits(&edits);
        assert_eq!(other.collect_rosters(), rosters);
        assert_eq!(text(&other), serde_json::to_string(&edits).unwrap());

        other.reset_effect(0, "dash");
        assert!(other.collect_rosters().is_empty());
        assert!(other.collect_authored_edits().is_empty());
    }

    /// A part renamed in the part list keeps the name in the saved list, made unique against
    /// the effect's other parts, and opens again under it.
    #[test]
    fn a_renamed_part_is_saved_by_its_new_name() {
        let mut editor = two_effects();
        add_b0(&mut editor, false);
        let taken = ["a0", "a1"];
        let name = unique_part_name(&part_name_from_typed("a0"), &taken);
        assert_eq!(name, "a0_2");
        editor.ptcl.as_mut().unwrap().emitter_sets[0].emitters[1].name = name;
        let rosters = editor.collect_rosters();
        assert_eq!(rosters[0].slots[1].name, "a0_2");
        assert_eq!(rosters[0].slots[1].source_name, "b0");
        let mut reloaded = two_effects();
        reloaded.apply_structure_edits(&rosters, &[]);
        assert_eq!(names(&reloaded, 0), ["a0@0", "a0_2@0", "a1@0"]);
    }

    /// Each starter is the part named, in the shared file of the game folder, and stops on its
    /// own.
    #[test]
    fn every_starter_is_a_one_shot_part_of_the_shared_file() {
        let Some(root) = std::env::var_os("VISIONARY_EFF_ROOT").map(PathBuf::from) else {
            eprintln!("skipped: set VISIONARY_EFF_ROOT to the extracted effect/ tree");
            return;
        };
        let file = PartFile::read(&root.join(SHARED_FILE_REL), SHARED_FILE_REL.into())
            .expect("shared file");
        for starter in STARTERS {
            let set = file.set(starter.set).expect(starter.set);
            let em = set
                .emitters
                .get(starter.index)
                .unwrap_or_else(|| panic!("{}: no part {}", starter.label, starter.index));
            assert_eq!(em.name, starter.part, "{}", starter.label);
            assert!(
                attr(em, "emission.is_one_time").is_some_and(|v| v.as_bool()),
                "{} never stops",
                starter.label
            );
            assert!(
                em.texture_index.is_some(),
                "{} has no texture",
                starter.label
            );
            assert!(
                file.entries
                    .iter()
                    .any(|e| file.loaded.ptcl.emitter_sets[e.set_idx].name == starter.set),
                "{}: no effect plays {}",
                starter.label,
                starter.set
            );
        }
    }

    /// A part saved from a costume's own effect file is read from the mod that has it, which the
    /// game folder doesn't: the editor looks where the export and the live carrier look.
    #[test]
    fn a_part_from_a_costume_file_is_found_in_the_mod() {
        let Some(root) = std::env::var_os("VISIONARY_EFF_ROOT").map(PathBuf::from) else {
            eprintln!("skipped: set VISIONARY_EFF_ROOT to the extracted effect/ tree");
            return;
        };
        const COSTUME: &str = "effect/fighter/pacman/ef_pacman_c40.eff";
        let mod_root = tempfile::tempdir().unwrap();
        let file = mod_root.path().join(COSTUME);
        std::fs::create_dir_all(file.parent().unwrap()).unwrap();
        std::fs::copy(root.join("effect/fighter/pacman/ef_pacman.eff"), file).unwrap();
        let mut editor = EffEditor {
            export_root: root.clone(),
            ..Default::default()
        };
        assert!(editor.part_file(COSTUME).is_none(), "only the mod has it");
        editor.effect_roots = vec![root, mod_root.path().to_path_buf()];
        assert!(editor.part_file(COSTUME).is_some());
    }

    /// A starter added to a fighter's effect brings its texture into the file: listed, numbered,
    /// readable for its picture, and still there after the project is opened again.
    #[test]
    fn a_starter_brings_its_texture_into_the_file() {
        let Some(root) = std::env::var_os("VISIONARY_EFF_ROOT").map(PathBuf::from) else {
            eprintln!("skipped: set VISIONARY_EFF_ROOT to the extracted effect/ tree");
            return;
        };
        const SRC: &str = "effect/fighter/mario/ef_mario.eff";
        let open = |editor: &mut EffEditor| {
            editor.export_root = root.clone();
            editor.load_eff(&root.join(SRC));
            editor.selected_entry = editor
                .entries
                .iter()
                .position(|e| e.name == "mario_fb_shoot");
            assert!(editor.selected_entry.is_some(), "mario_fb_shoot");
        };
        let mut editor = EffEditor::default();
        open(&mut editor);
        let set_idx = editor.entries[editor.selected_entry.unwrap()].set_idx;
        let had = editor.ptcl.as_ref().unwrap().bntx_textures.len();
        let has_line03 = |editor: &EffEditor| {
            editor
                .ptcl
                .as_ref()
                .unwrap()
                .bntx_textures
                .iter()
                .position(|t| t.tex_name == "ef_cmn_line03")
        };
        assert_eq!(
            has_line03(&editor),
            None,
            "Mario's file has no spark texture"
        );
        let sparks = STARTERS.iter().find(|s| s.label == "Sparks").unwrap();
        let at = editor
            .add_starter(set_idx, sparks, false)
            .expect("add sparks");
        let line03 = has_line03(&editor).expect("the spark texture came with it");
        assert_eq!(line03, had, "appended after the file's own");
        let added = &editor.ptcl.as_ref().unwrap().emitter_sets[set_idx].emitters[at];
        assert_eq!(added.name, "sparks");
        assert_eq!(added.texture_index, Some(line03 as u32));
        crate::texture_import::decode_preview(
            editor.texture_pool.as_deref().unwrap(),
            line03,
            "ef_cmn_line03",
            crate::texture_import::Form::Editable,
            Some(THUMB),
        )
        .expect("its picture decodes from the file's pool");
        assert!(
            editor.collect_authored_edits().is_empty(),
            "carrying the texture is not a texture edit"
        );
        let rosters = editor.collect_rosters();
        assert_eq!(rosters[0].slots[at].source_file, SHARED_FILE_REL);

        let mut reloaded = EffEditor::default();
        open(&mut reloaded);
        reloaded.apply_structure_edits(&rosters, &[]);
        let part = &reloaded.ptcl.as_ref().unwrap().emitter_sets[set_idx].emitters[at];
        assert_eq!(part.name, "sparks");
        assert_eq!(part.texture_index, has_line03(&reloaded).map(|t| t as u32));
        assert!(part.texture_index.is_some());
        assert_eq!(reloaded.collect_rosters(), rosters);

        // Only the added part may use the texture that came with it: taken out, it leaves with it.
        let own = |editor: &EffEditor, part: usize| {
            let em = &editor.ptcl.as_ref().unwrap().emitter_sets[set_idx].emitters[part];
            editor.pristine[set_idx][em.source_idx].clone()
        };
        assert!(!editor.texture_belongs_to_added_part(line03, Some(&own(&editor, at))));
        assert!(editor.texture_belongs_to_added_part(line03, Some(&own(&editor, 0))));
        assert!(!editor.texture_belongs_to_added_part(0, Some(&own(&editor, 0))));

        // Another file and back: the part, its texture and the undo step that added it.
        editor
            .history
            .undo
            .push(editor.history_capture_set(set_idx).unwrap());
        editor.history.undo[0].emitters.remove(at);
        let queued = editor.collect_rosters();
        editor.load_eff(&root.join("effect/fighter/luigi/ef_luigi.eff"));
        assert!(has_line03(&editor).is_none() && !editor.can_undo());
        editor.load_eff(&root.join(SRC));
        editor.apply_structure_edits(&queued, &[]);
        assert!(editor.can_undo(), "{:?}", editor.history.cleared);
        assert_eq!(has_line03(&editor), Some(line03));
        assert_eq!(editor.collect_rosters(), rosters);
        assert_eq!(editor.carried_textures.len(), 1, "brought in once");
        editor.undo();
        assert!(editor.collect_rosters().is_empty());
        editor.redo();
        assert_eq!(editor.collect_rosters(), rosters);

        // What the editor shows is what ships: the exporter's file passes the game-file rules
        // and holds the part with that texture.
        let eff = crate::mod_project::EffMod {
            source_rel: SRC.into(),
            rosters,
            ..Default::default()
        };
        let bytes = crate::eff_export::rebuild_eff_bytes(
            &std::fs::read(root.join(SRC)).unwrap(),
            &eff,
            Some(&root),
        )
        .expect("export");
        assert!(crate::eff_validate::problems(&bytes).is_empty());
        let dir = std::env::temp_dir().join(format!("visionary-add-part-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("ef_mario.eff");
        std::fs::write(&path, &bytes).unwrap();
        let written = load_effect(&path).expect("written file loads");
        std::fs::remove_dir_all(&dir).ok();
        let set = &written.ptcl.emitter_sets[set_idx];
        let part = set
            .emitters
            .iter()
            .find(|e| e.name == "sparks")
            .expect("sparks shipped");
        let texture = part
            .texture_index
            .and_then(|t| written.ptcl.bntx_textures.get(t as usize))
            .expect("its texture resolves in the written file");
        assert_eq!(texture.tex_name, "ef_cmn_line03");
    }
}
