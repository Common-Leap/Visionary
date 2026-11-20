//! Creating a new character as a costume of a donor fighter.
//!
//! What this produces is a costume slot with its own model, animations, effects, name, and
//! **moveset**, and an entry of its own on the character select screen (`css::add_character`).
//!
//! ## The base fighter's files are copied in
//!
//! A character made here starts as a full copy of the fighter it plays as: every model part,
//! animation, effect, sound bank and portrait of the matching base costume, under the new
//! character's own costume numbers and name. It works in the game at once, and every file that
//! makes it up is in its folder to be replaced.
//!
//! ## The slot guard, and the trap inside it
//!
//! A Smashline script **replaces** the fighter's script for every costume, not just ours. So a
//! guard that returns early on other costumes does not "fall through to vanilla" — it silences
//! the move for every other costume of the donor, which is a far worse bug than the one it was
//! meant to avoid and shows up only when someone plays the donor normally.
//!
//! The guard therefore has two arms, and the vanilla arm is mandatory: our costume runs the
//! authored body, every other costume runs the fighter's original body, which Visionary
//! already has because it stores the source of every move it opens. [`wrap_with_slot_guard`]
//! will not produce a function without it.

use std::path::{Path, PathBuf};

use anyhow::{bail, Result};

/// The directories a new costume slot needs, relative to a mod root.
///
/// `model/body` and `motion/body` are where the user's own files go. The effect file is
/// per-slot too, so a character can have its own particles without touching the donor's.
pub fn directories(donor: &str, slot: u8) -> Vec<String> {
    let donor = donor.to_ascii_lowercase();
    vec![
        format!("fighter/{donor}/model/body/c{slot:02}"),
        format!("fighter/{donor}/motion/body/c{slot:02}"),
        format!("effect/fighter/{donor}"),
    ]
}

/// The per-slot effect file's path, relative to a mod root.
pub fn effect_file(donor: &str, slot: u8) -> String {
    let donor = donor.to_ascii_lowercase();
    format!("effect/fighter/{donor}/ef_{donor}_c{slot:02}.eff")
}

/// The folders made for a new character.
#[derive(Debug, Clone)]
pub struct Scaffold {
    /// Every costume it takes, ascending.
    pub slots: Vec<u8>,
}

/// Make the folders for every costume in `slots` under one mod folder.
pub fn create_many(mod_root: &Path, donor: &str, slots: &[u8]) -> Result<Scaffold> {
    if donor.trim().is_empty() {
        bail!("a new character needs a fighter to play as");
    }
    if slots.is_empty() {
        bail!("a new character needs at least one costume");
    }
    let mut sorted: Vec<u8> = slots.to_vec();
    sorted.sort_unstable();
    sorted.dedup();
    for slot in &sorted {
        for relative in directories(donor, *slot) {
            std::fs::create_dir_all(mod_root.join(&relative))?;
        }
    }
    Ok(Scaffold { slots: sorted })
}

/// Directories for every slot in `slots`, de-duplicated (the shared
/// `effect/fighter/<donor>` dir appears once).
pub fn directories_for_slots(donor: &str, slots: &[u8]) -> Vec<String> {
    let mut out = Vec::new();
    for slot in slots {
        for relative in directories(donor, *slot) {
            if !out.contains(&relative) {
                out.push(relative);
            }
        }
    }
    out
}

/// Per-slot effect files for every slot in `slots`.
pub fn effect_files(donor: &str, slots: &[u8]) -> Vec<String> {
    slots.iter().map(|slot| effect_file(donor, *slot)).collect()
}

/// What [`copy_base`] brought over.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct Copied {
    pub files: usize,
    pub bytes: u64,
}

/// Copy the base fighter's files into a new character's folders under `mod_root`.
///
/// Colour `k` of the new character (costume `slots[k]`) starts as base costume `k`: each part
/// under `fighter/<donor>/model` and `/motion`, the fighter's effect file as the costume's own,
/// its sound banks, and its portraits renamed to `name_id` and colour `k`. `ui_name` is the
/// name the base fighter's portraits are filed under. Later `roots` win, as in the game.
pub fn copy_base(
    mod_root: &Path,
    roots: &[PathBuf],
    donor: &str,
    ui_name: &str,
    name_id: &str,
    slots: &[u8],
) -> Result<Copied> {
    let donor = donor.to_ascii_lowercase();
    let mut copied = Copied::default();
    let mut copy = |from: &Path, to: &Path| -> Result<()> {
        if let Some(parent) = to.parent() {
            std::fs::create_dir_all(parent)?;
        }
        copied.bytes += std::fs::copy(from, to)?;
        copied.files += 1;
        Ok(())
    };
    fn files(dir: &Path) -> Vec<PathBuf> {
        let mut found: Vec<PathBuf> = std::fs::read_dir(dir)
            .into_iter()
            .flatten()
            .flatten()
            .map(|entry| entry.path())
            .collect();
        found.sort();
        found
    }
    fn tree(
        from: &Path,
        to: &Path,
        copy: &mut dyn FnMut(&Path, &Path) -> Result<()>,
    ) -> Result<()> {
        for path in files(from) {
            let Some(name) = path.file_name() else {
                continue;
            };
            if path.is_dir() {
                tree(&path, &to.join(name), copy)?;
            } else {
                copy(&path, &to.join(name))?;
            }
        }
        Ok(())
    }
    for (colour, slot) in slots.iter().enumerate() {
        let (base, own) = (format!("c{:02}", colour % 8), format!("c{slot:02}"));
        for root in roots {
            for kind in ["model", "motion"] {
                let kind_dir = root.join("fighter").join(&donor).join(kind);
                for part in files(&kind_dir) {
                    let Some(name) = part.file_name() else {
                        continue;
                    };
                    let to = mod_root
                        .join("fighter")
                        .join(&donor)
                        .join(kind)
                        .join(name)
                        .join(&own);
                    tree(&part.join(&base), &to, &mut copy)?;
                }
            }
            let effect = root.join(format!("effect/fighter/{donor}/ef_{donor}.eff"));
            if effect.is_file() {
                copy(&effect, &mod_root.join(effect_file(&donor, *slot)))?;
            }
            for bank in ["fighter", "fighter_voice"] {
                let dir = root.join("sound/bank").join(bank);
                for path in files(&dir) {
                    let Some(name) = path.file_name().and_then(|name| name.to_str()) else {
                        continue;
                    };
                    let marker = format!("_{donor}_{base}.");
                    if let Some((stem, ext)) = name.split_once(&marker) {
                        copy(
                            &path,
                            &mod_root
                                .join("sound/bank")
                                .join(bank)
                                .join(format!("{stem}_{donor}_{own}.{ext}")),
                        )?;
                    }
                }
            }
            for area in ["ui/replace/chara", "ui/replace_patch/chara"] {
                for set in files(&root.join(area)) {
                    let Some(kind) = set.file_name().and_then(|name| name.to_str()) else {
                        continue;
                    };
                    let from = set.join(format!("{kind}_{ui_name}_{:02}.bntx", colour % 8));
                    if from.is_file() {
                        copy(
                            &from,
                            &mod_root
                                .join(area)
                                .join(kind)
                                .join(format!("{kind}_{name_id}_{colour:02}.bntx")),
                        )?;
                    }
                }
            }
        }
    }
    Ok(copied)
}

/// What one costume slot holds, per file kind, plus where to open it.
///
/// `measure` answers "how ready is this authored slot"; this answers "what
/// is in each skin and where do I put more" for any fighter's slot —
/// vanilla's eight, extended slots, or a new character's. Counts union
/// across roots like everything else; directory answers prefer later roots
/// (mods) over the data root, so "open" lands somewhere writable.
#[derive(Debug, Clone, Default)]
pub struct SlotInventory {
    /// Model mesh files (`.numdlb/.numshb/.numatb/.nusktb/.numshexb`).
    pub meshes: usize,
    /// Textures (`.nutexb`) in the model folder.
    pub textures: usize,
    /// Other files in the model folder (materials aside, usually guides).
    pub other_model: usize,
    /// Animations (`.nuanmb`) in the motion folder.
    pub anims: usize,
    pub has_motion_list: bool,
    pub has_effect: bool,
    pub has_portrait: bool,
    /// Existing slot folders, mods preferred — the reveal targets.
    pub model_dir: Option<PathBuf>,
    pub motion_dir: Option<PathBuf>,
}

/// Model mesh extensions, lowercased.
const MESH_EXTS: &[&str] = &["numdlb", "numshb", "numatb", "nusktb", "numshexb"];

/// Inventory one costume slot: file counts by kind plus reveal targets.
///
/// `name_id` is the roster row's id for the portrait lookup, when the slot
/// has a row (a slot without one simply reports no portrait).
pub fn inventory(roots: &[PathBuf], donor: &str, slot: u8, name_id: Option<&str>) -> SlotInventory {
    let donor = donor.to_ascii_lowercase();
    let mut out = SlotInventory::default();
    // Later roots win, so walk reversed: the first hit is the mod the user
    // would edit, not the dumped game data underneath it.
    let mut rev: Vec<&PathBuf> = roots.iter().collect();
    rev.reverse();
    let model_rel = format!("fighter/{donor}/model/body/c{slot:02}");
    let motion_rel = format!("fighter/{donor}/motion/body/c{slot:02}");
    for root in &rev {
        let dir = root.join(&model_rel);
        if out.model_dir.is_none() && dir.is_dir() {
            out.model_dir = Some(dir.clone());
        }
        if let Ok(entries) = std::fs::read_dir(&dir) {
            for entry in entries.flatten() {
                if !entry.path().is_file() {
                    continue;
                }
                let name = entry.file_name().to_string_lossy().to_ascii_lowercase();
                if name == "put_your_model_here.txt" {
                    continue;
                }
                let ext = name.rsplit('.').next().unwrap_or("");
                if MESH_EXTS.contains(&ext) {
                    out.meshes += 1;
                } else if ext == "nutexb" {
                    out.textures += 1;
                } else {
                    out.other_model += 1;
                }
            }
        }
        let dir = root.join(&motion_rel);
        if out.motion_dir.is_none() && dir.is_dir() {
            out.motion_dir = Some(dir.clone());
        }
        if let Ok(entries) = std::fs::read_dir(&dir) {
            for entry in entries.flatten() {
                if !entry.path().is_file() {
                    continue;
                }
                let name = entry.file_name().to_string_lossy().to_ascii_lowercase();
                if name == "motion_list.bin" {
                    out.has_motion_list = true;
                } else if name.ends_with(".nuanmb") {
                    out.anims += 1;
                }
            }
        }
    }
    out.has_effect = roots
        .iter()
        .any(|root| root.join(effect_file(&donor, slot)).is_file());
    out.has_portrait = name_id
        .map(|nid| super::icons::find_portrait(roots, nid, slot).is_some())
        .unwrap_or(false);
    out
}

/// Emit an authored function, the fighter's own function, and a dispatcher between them.
///
/// The dispatcher keeps the **registered** name, so the `agent.acmd` line the exporter already
/// writes needs no change. The authored body moves to `<name>_costume` and the fighter's own
/// script becomes `<name>_original`.
///
/// `authored_source` and `original_source` are whole `unsafe extern "C" fn …` definitions. Both
/// are renamed by replacing their declared name, which is exact rather than a substring
/// rewrite: the name appears once, in the declaration.
///
/// Emit an authored function, the fighter's own function, and a dispatcher between them,
/// gated to every slot in `slots`.
///
/// The dispatcher keeps the **registered** name, so the `agent.acmd` line the exporter already
/// writes needs no change. The authored body moves to `<name>_costume` and the fighter's own
/// script becomes `<name>_original`. A multi-skin character shares one moveset across all its
/// costumes, so the gate is an OR over its slots (`color == 8 || color == 9 …`).
///
/// `authored_source` and `original_source` are whole `unsafe extern "C" fn …` definitions. Both
/// are renamed by replacing their declared name, which is exact rather than a substring
/// rewrite: the name appears once, in the declaration.
///
/// The original arm is mandatory in the same way it is in [`wrap_with_slot_guard`]: without it
/// this move would be removed from every other costume of the donor.
pub fn costume_gated_source_multi(
    registered_name: &str,
    slots: &[u8],
    authored_source: &str,
    original_source: &str,
) -> String {
    let mut sorted: Vec<u8> = slots.to_vec();
    sorted.sort_unstable();
    sorted.dedup();
    debug_assert!(!sorted.is_empty(), "a costume gate needs at least one slot");
    let condition = sorted
        .iter()
        .map(|slot| format!("color == {slot}"))
        .collect::<Vec<_>>()
        .join(" || ");
    let authored_name = format!("{registered_name}_costume");
    let original_name = format!("{registered_name}_original");
    format!(
        "{authored}\n{original}\n\
         unsafe extern \"C\" fn {registered_name}(agent: &mut L2CAgentBase) {{\n\
         \x20   // Costume gate, generated by Visionary. This script replaces the move for EVERY\n\
         \x20   // costume of this fighter, so the else arm runs the fighter's own script —\n\
         \x20   // without it, every other costume would lose this move entirely.\n\
         \x20   let color = smash::app::lua_bind::WorkModule::get_int(\n\
         \x20       agent.module_accessor,\n\
         \x20       *smash::lib::lua_const::FIGHTER_INSTANCE_WORK_ID_INT_COLOR,\n\
         \x20   );\n\
         \x20   if {condition} {{\n\
         \x20       {authored_name}(agent);\n\
         \x20   }} else {{\n\
         \x20       {original_name}(agent);\n\
         \x20   }}\n\
         }}\n",
        authored = rename_function(authored_source, registered_name, &authored_name),
        original = rename_declared_function(original_source, &original_name),
        registered_name = registered_name,
        condition = condition,
        authored_name = authored_name,
        original_name = original_name,
    )
}

/// The same function source as the costume arm of a gate names it.
///
/// Used by the export verifier so that "did this body reach disk" is still asked of a gated
/// move, rather than the check being skipped for it.
pub fn costume_arm_source(source: &str) -> String {
    let Some(name) = declared_name(source) else {
        return source.to_string();
    };
    rename_function(source, &name, &format!("{name}_costume"))
}

/// The name of the first function a source declares.
fn declared_name(source: &str) -> Option<String> {
    let start = source.find("fn ")? + 3;
    let open = source[start..].find('(')?;
    Some(source[start..start + open].trim().to_string())
}

/// Rename a function whose current name is known.
fn rename_function(source: &str, from: &str, to: &str) -> String {
    source.replacen(&format!("fn {from}("), &format!("fn {to}("), 1)
}

/// Rename whatever function a source defines first, to `to`.
///
/// The fighter's own script comes from the archive or from a live capture and is named after
/// the move, which is not always the name the exporter chose. Rewriting the declared name
/// whatever it is avoids depending on those agreeing.
fn rename_declared_function(source: &str, to: &str) -> String {
    match declared_name(source) {
        Some(name) => rename_function(source, &name, to),
        None => source.to_string(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_scaffold_creates_the_slots_own_directories() {
        let dir = tempfile::tempdir().unwrap();
        let scaffold = create_many(dir.path(), "Mario", &[8]).unwrap();
        assert_eq!(scaffold.slots, vec![8]);
        assert!(dir.path().join("fighter/mario/model/body/c08").is_dir());
        assert!(dir.path().join("fighter/mario/motion/body/c08").is_dir());
        assert!(dir.path().join("effect/fighter/mario").is_dir());
    }

    /// A new character starts as the base fighter: each colour gets the matching base costume
    /// under its own costume number, and its portraits under its own name.
    #[test]
    fn the_base_fighter_s_files_are_copied_under_the_new_character_s_names() {
        let (game, made) = (tempfile::tempdir().unwrap(), tempfile::tempdir().unwrap());
        let put = |path: &str, bytes: &[u8]| {
            let path = game.path().join(path);
            std::fs::create_dir_all(path.parent().unwrap()).unwrap();
            std::fs::write(path, bytes).unwrap();
        };
        put("fighter/mario/model/body/c00/model.numdlb", b"zero");
        put("fighter/mario/model/body/c01/model.numdlb", b"one");
        put("fighter/mario/model/fireball/c00/model.numdlb", b"ball");
        put("fighter/mario/motion/body/c00/motion_list.bin", b"list");
        put("fighter/mario/motion/body/c00/deep/a.nuanmb", b"anim");
        put("effect/fighter/mario/ef_mario.eff", b"eff");
        put("sound/bank/fighter/se_mario_c01.nus3audio", b"se");
        put("ui/replace/chara/chara_1/chara_1_mario_01.bntx", b"face");
        put("ui/replace/chara/chara_1/chara_1_luigi_01.bntx", b"other");
        let copied = copy_base(
            made.path(),
            &[game.path().to_path_buf()],
            "Mario",
            "mario",
            "vision",
            &[8, 9],
        )
        .unwrap();
        let read = |path: &str| std::fs::read(made.path().join(path)).ok();
        assert_eq!(
            read("fighter/mario/model/body/c08/model.numdlb").as_deref(),
            Some(&b"zero"[..])
        );
        assert_eq!(
            read("fighter/mario/model/body/c09/model.numdlb").as_deref(),
            Some(&b"one"[..])
        );
        assert_eq!(
            read("fighter/mario/model/fireball/c08/model.numdlb").as_deref(),
            Some(&b"ball"[..])
        );
        assert_eq!(
            read("fighter/mario/motion/body/c08/deep/a.nuanmb").as_deref(),
            Some(&b"anim"[..])
        );
        assert_eq!(
            read("effect/fighter/mario/ef_mario_c09.eff").as_deref(),
            Some(&b"eff"[..])
        );
        assert_eq!(
            read("sound/bank/fighter/se_mario_c09.nus3audio").as_deref(),
            Some(&b"se"[..])
        );
        assert_eq!(
            read("ui/replace/chara/chara_1/chara_1_vision_01.bntx").as_deref(),
            Some(&b"face"[..])
        );
        assert!(
            read("ui/replace/chara/chara_1/chara_1_vision_00.bntx").is_none(),
            "the base has no such portrait"
        );
        assert_eq!(copied.files, 9);
        assert_eq!(copied.bytes, 4 + 3 + 4 + 4 + 4 + 3 + 3 + 2 + 4);
    }

    /// The dossier splits the model folder into meshes vs textures so a slot
    /// with a model but no textures (untextured grey) reads differently from
    /// an empty one — and the guide file counts as neither.
    #[test]
    fn inventory_counts_meshes_textures_and_anims_separately() {
        let dir = tempfile::tempdir().unwrap();
        let model = dir.path().join("fighter/mario/model/body/c08");
        let motion = dir.path().join("fighter/mario/motion/body/c08");
        std::fs::create_dir_all(&model).unwrap();
        std::fs::create_dir_all(&motion).unwrap();
        std::fs::write(model.join("model.numdlb"), b"mesh").unwrap();
        std::fs::write(model.join("face.nutexb"), b"tex").unwrap();
        std::fs::write(motion.join("attack.nuanmb"), b"anim").unwrap();
        std::fs::write(motion.join("motion_list.bin"), b"list").unwrap();
        let inv = inventory(&[dir.path().to_path_buf()], "mario", 8, None);
        assert_eq!(inv.meshes, 1);
        assert_eq!(inv.textures, 1);
        assert_eq!(inv.other_model, 0);
        assert_eq!(inv.anims, 1);
        assert!(inv.has_motion_list);
        assert!(!inv.has_effect);
        assert!(!inv.has_portrait);
        assert_eq!(inv.model_dir, Some(model));
        assert_eq!(inv.motion_dir, Some(motion));
    }

    /// Reveal targets prefer mod roots: opening the dumped game data invites
    /// editing files the export will never ship.
    #[test]
    fn inventory_reveal_targets_prefer_mod_roots() {
        let base = tempfile::tempdir().unwrap();
        let modded = tempfile::tempdir().unwrap();
        for root in [base.path(), modded.path()] {
            std::fs::create_dir_all(root.join("fighter/mario/model/body/c00")).unwrap();
        }
        let inv = inventory(
            &[base.path().to_path_buf(), modded.path().to_path_buf()],
            "mario",
            0,
            None,
        );
        assert_eq!(
            inv.model_dir,
            Some(modded.path().join("fighter/mario/model/body/c00"))
        );
    }

    const AUTHORED: &str = "unsafe extern \"C\" fn game_attack11(agent: &mut L2CAgentBase) {\n    frame(agent.lua_state_agent, 4.0);\n}\n";
    /// The archive spells the fighter's own script after the move, which is not always the
    /// name the exporter picked, hence renaming whatever is declared rather than a known name.
    const ORIGINAL: &str = "unsafe extern \"C\" fn game_attack_11(agent: &mut L2CAgentBase) {\n    frame(agent.lua_state_agent, 2.0);\n}\n";

    #[test]
    fn a_costume_gate_keeps_the_registered_name_and_renames_both_arms() {
        let generated = costume_gated_source_multi("game_attack11", &[8], AUTHORED, ORIGINAL);
        assert!(generated.contains("fn game_attack11_costume(agent"));
        assert!(generated.contains("fn game_attack11_original(agent"));
        assert!(generated.contains("unsafe extern \"C\" fn game_attack11(agent"));
        assert!(generated.contains("if color == 8 {"));
        assert!(generated.contains("game_attack11_costume(agent);"));
        assert!(generated.contains("game_attack11_original(agent);"));
        // The original's own name must be gone, or the crate has two functions defining it.
        assert!(!generated.contains("fn game_attack_11("));
    }

    /// The dispatcher is what the exporter registers, so it must exist exactly once under the
    /// registered name. Two definitions, or none, both fail to build.
    #[test]
    fn exactly_one_function_carries_the_registered_name() {
        let generated = costume_gated_source_multi("game_attack11", &[8], AUTHORED, ORIGINAL);
        assert_eq!(generated.matches("fn game_attack11(").count(), 1);
    }

    #[test]
    fn a_costume_gated_function_set_parses_as_rust() {
        let generated = costume_gated_source_multi("game_attack11", &[8], AUTHORED, ORIGINAL);
        let source = format!("mod generated {{ use smash::lib::lua_const::*; {generated} }}");
        if let Err(error) = syn::parse_file(&source) {
            panic!("costume-gated source does not parse: {error}\n\n{generated}");
        }
    }
}
