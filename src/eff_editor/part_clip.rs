//! Copying a part, with the parts made from it, and pasting it: in the same effect, another one,
//! another file or another character.
//!
//! A copy is what the project saves of a part (`EmitterSlot`: which part of which file it is)
//! and of its edits, so a pasted part is read, shown, saved, exported and sent as one added from
//! that file is, whatever becomes of the effect it was copied from. It is put on the system
//! clipboard as text too: Ctrl+V finds it there after a restart, or in another window.

use super::part_tree::PartTree;
use super::*;
use crate::mod_project::{EmitterFieldEdits, EmitterSlot, SlotTexture};

/// A copied part and the parts made from it, the part first.
#[derive(Clone, Debug, serde::Serialize, serde::Deserialize)]
pub(super) struct PartClip {
    /// What marks the clipboard's text as ours.
    visionary_part: u32,
    /// The effect it was copied from: its name as the list shows it, its set, and its file.
    effect: String,
    set: String,
    file: String,
    /// Whether it was made from another part.
    child: bool,
    parts: Vec<ClipPart>,
}

const CLIP_VERSION: u32 = 1;

#[derive(Clone, Debug, serde::Serialize, serde::Deserialize)]
struct ClipPart {
    /// The part of a file it is, under the name it had.
    slot: EmitterSlot,
    /// Its place and name in the set it was copied from, when it is that set's own.
    own: Option<(usize, String)>,
    /// What differs from that part.
    fields: EmitterFieldEdits,
    /// The textures it was given in place of that part's, which another file may lack.
    textures: Vec<SlotTexture>,
    /// Why it cannot go to another file.
    stays: Option<String>,
}

/// The settings that name a thing of their own file by its place there, or a model by its id,
/// with what they name.
const OWN_FILE_ATTRS: &[(&str, &str)] = &[
    ("shader_references.shader_index", "shader"),
    ("shader_references.compute_shader_index", "shader"),
    ("shader_references.user_shader_index1", "shader"),
    ("shader_references.user_shader_index2", "shader"),
    ("particle_data.primitive_id", "model"),
    ("particle_data.primitive_ex_id", "model"),
    ("shape_info.primitive_index", "model"),
];

impl PartClip {
    /// The copied parts' names: "Clock 1 and its child part Trace 1".
    fn names(&self) -> String {
        let mut names = self.parts.iter().map(|p| part_words(&p.slot.name));
        let first = names.next().unwrap_or_default();
        let children: Vec<String> = names.collect();
        match children.len() {
            0 => first,
            1 => format!("{first} and its child part {}", children[0]),
            _ => format!("{first} and its child parts {}", children.join(", ")),
        }
    }

    /// What Paste's hover says it holds.
    fn words(&self) -> String {
        format!(
            "{} from {} in {}",
            self.names(),
            self.effect,
            file_name(&self.file)
        )
    }

    /// A copy as the clipboard has it, when its text is one.
    fn read(text: &str) -> Option<Self> {
        if !text.trim_start().starts_with("{\"visionary_part\"") {
            return None;
        }
        let clip: Self = serde_json::from_str(text).ok()?;
        // One part and what is made from it, out of files named as the game folder names them.
        let files = clip.parts.iter().flat_map(|p| {
            let textures = p.slot.textures.iter().chain(&p.textures);
            std::iter::once(&p.slot.source_file).chain(textures.map(|t| &t.file))
        });
        let plain = |file: &String| {
            Path::new(file)
                .components()
                .all(|c| matches!(c, std::path::Component::Normal(_)))
        };
        // Named as a file can hold it.
        let named = |p: &ClipPart| {
            let name = &p.slot.name;
            name.len() <= 63 && name.chars().all(|c| c.is_ascii_graphic() || c == ' ')
        };
        let fits = clip.visionary_part == CLIP_VERSION
            && clip.parts.iter().all(named)
            && clip.parts.first().is_some_and(|p| p.slot.depth == 0)
            && clip.parts.iter().skip(1).all(|p| p.slot.depth == 1)
            && clip.parts.len() <= crate::eff_validate::MAX_CHILDREN + 1
            && files.into_iter().all(plain);
        fits.then_some(clip)
    }
}

fn file_name(rel: &str) -> String {
    Path::new(rel)
        .file_name()
        .map_or_else(|| rel.to_owned(), |n| n.to_string_lossy().into_owned())
}

/// Where the copy goes in an effect with part `selected` open: its place in the list, and
/// whether it is made from another part there.
///
/// Right after the open part, and made from the same part when both are made from one; else on
/// its own after the open part's family. `as_child` makes it from the open part.
fn paste_place(
    clip: &PartClip,
    tree: &PartTree,
    selected: usize,
    as_child: bool,
) -> Result<(usize, u8), String> {
    if tree.len() == 0 {
        return match as_child {
            true => Err("This effect has no part to make it from".into()),
            false => Ok((0, 0)),
        };
    }
    let selected = selected.min(tree.len() - 1);
    let root = tree.root(selected);
    if as_child && tree.depth(selected) > 0 {
        return Err(format!(
            "{} is itself made from {}, and the game plays one level of child parts",
            tree.name(selected),
            tree.name(root)
        ));
    }
    if !as_child && !(clip.child && tree.depth(selected) > 0) {
        return Ok((tree.family(root).end, 0));
    }
    if clip.parts.len() > 1 {
        return Err(format!(
            "{} has child parts of its own, and the game plays one level of them",
            part_words(&clip.parts[0].slot.name)
        ));
    }
    if tree.children(root).len() >= crate::eff_validate::MAX_CHILDREN {
        return Err(format!(
            "{} has {} child parts, the most the game plays",
            tree.name(root),
            crate::eff_validate::MAX_CHILDREN
        ));
    }
    Ok((tree.family(selected).end, 1))
}

/// What pasting at part `selected` would do, or why it cannot be done.
pub(super) fn paste_state(
    clip: Option<&PartClip>,
    blocked: Option<&str>,
    tree: &PartTree,
    selected: usize,
    as_child: bool,
) -> Result<String, String> {
    let Some(clip) = clip else {
        return Err("No part is copied. Copy one from its menu or with Ctrl+C.".into());
    };
    let words = clip.words();
    if let Some(blocked) = blocked {
        return Err(format!("{words}\n{blocked}"));
    }
    match paste_place(clip, tree, selected, as_child) {
        Err(why) => Err(format!("{words}\n{why}")),
        Ok(_) if tree.len() == 0 => Ok(format!("{words} (Ctrl+V)")),
        Ok((_, 1)) => {
            let root = tree.name(tree.root(selected.min(tree.len() - 1)));
            let key = if as_child { "" } else { " (Ctrl+V)" };
            Ok(format!("{words}\nGoes under {root}, made from it{key}"))
        }
        Ok((at, _)) => Ok(format!(
            "{words}\nGoes after {} (Ctrl+V)",
            tree.path(at.saturating_sub(1))
        )),
    }
}

impl EffEditor {
    /// The file the open one is, as the game folder names it.
    fn open_rel(&self) -> String {
        self.loaded_rel().unwrap_or_default()
    }

    /// Copy part `i` of set `set_idx`, with the parts made from it, as they are now.
    pub(super) fn copy_part(&mut self, set_idx: usize, i: usize) {
        self.paste_error = None;
        if let Some(clip) = self.clip_of(set_idx, i) {
            self.clip_publish = serde_json::to_string(&clip).ok();
            self.set_clip(clip);
        }
    }

    fn set_clip(&mut self, clip: PartClip) {
        self.clip = Some(Arc::new(clip));
        self.clip_check = None;
    }

    fn clip_of(&mut self, set_idx: usize, i: usize) -> Option<PartClip> {
        let tree = self.part_tree(set_idx);
        let set = self.ptcl.as_ref()?.emitter_sets.get(set_idx)?;
        let set_name = set.name.clone();
        let emitters = set.emitters.get(tree.family(i))?.to_vec();
        let file = self.open_rel();
        let mut parts = Vec::new();
        for (k, em) in emitters.iter().enumerate() {
            let snapshot = self.pristine.get(set_idx)?.get(em.source_idx)?.clone();
            let fields = field_edits(em, &snapshot, &self.texture_names());
            let origin = self.origin_of(set_idx, em)?;
            let own = !self.part_origins.contains_key(&(set_idx, em.source_idx));
            let (textures, stays) = self.given_textures(em, &snapshot);
            let of_its_file = OWN_FILE_ATTRS
                .iter()
                .find(|(id, _)| fields.attrs.contains_key(*id))
                .map(|(_, what)| format!("Its {what} was changed to one of {}", file_name(&file)));
            parts.push(ClipPart {
                slot: EmitterSlot {
                    name: em.name.clone(),
                    depth: u8::from(k > 0),
                    ..origin.slot()
                },
                own: own.then(|| (em.source_idx, snapshot.name.clone())),
                fields,
                textures,
                stays: stays.or(of_its_file),
            });
        }
        Some(PartClip {
            visionary_part: CLIP_VERSION,
            effect: self.open_effect_name(),
            set: set_name,
            file,
            child: tree.depth(i) > 0,
            parts,
        })
    }

    /// The textures a part was given in place of its file part's, with the file each is in, and
    /// why they cannot go to another file when one is in none.
    fn given_textures(
        &mut self,
        em: &EmitterDef,
        snapshot: &EmitterSnapshot,
    ) -> (Vec<SlotTexture>, Option<String>) {
        let Some(pool) = self.ptcl.as_ref().map(|p| p.bntx_textures.clone()) else {
            return (Vec::new(), None);
        };
        let was = texture_slots(&snapshot.to_def(0), &pool);
        let mut given = Vec::new();
        for (slot, at) in texture_slots(em, &pool) {
            if was.contains(&(slot, at)) {
                continue;
            }
            let texture = &pool[at];
            let Some(file) = self.texture_file(texture) else {
                let why = format!(
                    "Its texture {} was added to {}",
                    texture.tex_name,
                    file_name(&self.open_rel())
                );
                return (given, Some(why));
            };
            given.push(SlotTexture {
                slot,
                name: texture.tex_name.clone(),
                file,
            });
        }
        (given, None)
    }

    /// The file on disk the open file's texture is in: its own, or the one it came from with a
    /// borrowed effect or an added part. None for a texture the author added here.
    fn texture_file(&mut self, texture: &TextureInfo) -> Option<String> {
        texture.id?;
        let own = self.open_rel();
        let came = self
            .carried_textures
            .iter()
            .any(|(t, _)| t.id == texture.id);
        if !came && !self.loaded_is_merged {
            return Some(own);
        }
        let borrowed = self
            .current_edit_source()
            .into_iter()
            .flat_map(|source| &source.transplants)
            .map(|item| item.donor_file.clone());
        let added = self.part_origins.values().flat_map(|origin| {
            let textures = origin.textures.iter().map(|t| t.file.clone());
            std::iter::once(origin.file.clone()).chain(textures)
        });
        let files: Vec<String> = std::iter::once(own).chain(borrowed).chain(added).collect();
        files.into_iter().find(|rel| {
            self.part_file(rel).is_some_and(|file| {
                let mut theirs = file.loaded.ptcl.bntx_textures.iter();
                theirs.any(|t| t.id == texture.id && t.tex_name == texture.tex_name)
            })
        })
    }

    /// Why the copy cannot go to the open file, whichever part is open. Worked out once for each
    /// file, as it looks for the copied part's file.
    pub(super) fn clip_blocked(&mut self) -> Option<String> {
        let clip = self.clip.clone()?;
        let key = self.loaded_path.clone();
        if let Some((_, blocked)) = self.clip_check.as_ref().filter(|(k, _)| *k == key) {
            return blocked.clone();
        }
        let other_file = !self.is_open_file(&clip.file);
        let blocked = clip.parts.iter().find_map(|part| {
            if other_file && part.stays.is_some() {
                return part.stays.clone();
            }
            let textures = part.slot.textures.iter().chain(&part.textures);
            let mut files =
                std::iter::once(&part.slot.source_file).chain(textures.map(|t| &t.file));
            files
                .find(|rel| !self.is_open_file(rel) && !self.file_of(rel).is_file())
                .map(|rel| format!("{} was not found", file_name(rel)))
        });
        self.clip_check = Some((key, blocked.clone()));
        blocked
    }

    /// Paste the copy at the open part, saying why when it does not go.
    pub(super) fn paste_part(&mut self, set_idx: usize, as_child: bool) {
        self.paste_error = self.try_paste(set_idx, as_child).err();
    }

    fn try_paste(&mut self, set_idx: usize, as_child: bool) -> Result<(), String> {
        let clip = self.clip.clone().ok_or("No part is copied")?;
        if let Some(blocked) = self.clip_blocked() {
            return Err(blocked);
        }
        let tree = self.part_tree(set_idx);
        let (at, depth) = paste_place(&clip, &tree, self.selected_emitter, as_child)?;
        let set_name = self
            .ptcl
            .as_ref()
            .and_then(|p| p.emitter_sets.get(set_idx))
            .map(|set| set.name.clone())
            .ok_or("No effect is open")?;
        let same_set = self.is_open_file(&clip.file) && clip.set == set_name;
        let mut parts = Vec::new();
        for part in &clip.parts {
            let mut fields = part.fields.clone();
            let mut slot = part.slot.clone();
            for texture in &part.textures {
                // A texture of this file is pointed at by the edit that says so; one of another
                // file comes with the part, as the part's own do.
                if self.is_open_file(&texture.file) {
                    continue;
                }
                fields.attrs.remove(TEXTURE_SLOT_IDS[texture.slot.min(2)]);
                if texture.slot == 0 {
                    fields.texture_name = None;
                }
                slot.textures.retain(|t| t.slot != texture.slot);
                slot.textures.push(texture.clone());
            }
            let count = self.file_part_count(set_idx);
            let own = part.own.as_ref().filter(|(at, name)| {
                let snapshot = self.pristine.get(set_idx).and_then(|list| list.get(*at));
                same_set && *at < count && snapshot.is_some_and(|s| s.name == *name)
            });
            let em = match own {
                Some((at, _)) => self.pristine[set_idx][*at].to_def(*at),
                None => self.foreign_slot_part(set_idx, &slot).ok_or_else(|| {
                    format!(
                        "{} of {} was not found in {}",
                        slot.source_name,
                        slot.source_set,
                        file_name(&slot.source_file)
                    )
                })?,
            };
            parts.push((em, fields, part));
        }
        // After the parts are read: they brought their textures.
        let textures = self.texture_names();
        let mut taken: Vec<String> = self
            .missing_parts
            .iter()
            .filter(|(set, ..)| *set == set_idx)
            .map(|(_, _, slot)| slot.name.clone())
            .collect();
        self.history_close_step();
        let label = format!("Paste {} to {}", clip.names(), self.open_effect_name());
        let set = self
            .ptcl
            .as_mut()
            .and_then(|p| p.emitter_sets.get_mut(set_idx))
            .ok_or("No effect is open")?;
        taken.extend(set.emitters.iter().map(|e| e.name.clone()));
        let mut pasted = Vec::new();
        for (mut em, fields, part) in parts {
            apply_field_edits(&mut em, &fields, &textures);
            let names: Vec<&str> = taken.iter().map(String::as_str).collect();
            em.name = add_part::unique_part_name(&part.slot.name, &names);
            em.depth = depth + part.slot.depth;
            taken.push(em.name.clone());
            pasted.push(em);
        }
        set.emitters.splice(at..at, pasted);
        self.selected_emitter = at;
        self.touch();
        self.history.next_label = Some(label);
        Ok(())
    }

    /// Ctrl+C and Ctrl+V on the Effects page. While a text box has focus, or text on the page is
    /// marked, the keys are for the text.
    pub(super) fn part_keys(&mut self, ctx: &egui::Context) {
        let Some(set_idx) = self
            .selected_entry
            .and_then(|i| self.entries.get(i))
            .map(|e| e.set_idx)
        else {
            return;
        };
        if ctx.text_edit_focused() {
            return;
        }
        let (copy, paste) = ctx.input(|i| {
            let mut paste = None;
            for event in &i.events {
                if let egui::Event::Paste(text) = event {
                    paste = Some(text.clone());
                }
            }
            (i.events.contains(&egui::Event::Copy), paste)
        });
        let marked = ctx
            .plugin_opt::<egui::text_selection::LabelSelectionState>()
            .is_some_and(|state| state.lock().has_selection());
        if copy && !marked {
            self.copy_part(set_idx, self.selected_emitter);
        }
        if let Some(text) = paste {
            // One copied in another window, or before a restart.
            if let Some(clip) = PartClip::read(&text) {
                self.set_clip(clip);
            }
            if self.clip.is_some() {
                self.paste_part(set_idx, false);
            }
        }
    }

    /// Put a part just copied on the system clipboard. Ctrl+V needs it there: with nothing on it
    /// no paste arrives.
    pub(super) fn publish_clip(&mut self, ctx: &egui::Context) {
        if let Some(text) = self.clip_publish.take() {
            ctx.copy_text(text);
        }
    }
}

#[cfg(test)]
mod tests;
