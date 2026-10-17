//! Which fighters a project is about.
//!
//! The left fighter list follows the open project: a fighter is in it when the project added
//! it, has files for it in `romfs/`, or links a moveset with scripts for it. This module is the
//! pure logic for that: scanning the project's `romfs/`, pointing a fighter's paths into it, and
//! naming the reasons a row belongs.

use std::collections::{BTreeMap, BTreeSet};
use std::path::Path;

use crate::data::FighterEntry;
use crate::mod_project::ProjectFighter;

/// `cNN` to `NN`: `c` and two or more ASCII digits that fit a `u8` (`c07`, `c120`; not `c7`).
fn parse_slot(folder: &str) -> Option<u8> {
    let digits = folder.strip_prefix('c')?;
    if digits.len() < 2 || !digits.bytes().all(|b| b.is_ascii_digit()) {
        return None;
    }
    digits.parse().ok()
}

/// True when `dir` holds a regular file anywhere below it. Symlinks are skipped, as
/// `merge_romfs_overlay` does: the romfs is an export boundary.
fn holds_file(dir: &Path) -> bool {
    let mut stack = vec![dir.to_path_buf()];
    while let Some(dir) = stack.pop() {
        let Ok(entries) = std::fs::read_dir(&dir) else {
            continue;
        };
        for entry in entries.flatten() {
            let path = entry.path();
            let Ok(meta) = std::fs::symlink_metadata(&path) else {
                continue;
            };
            if meta.file_type().is_symlink() {
                continue;
            }
            if meta.is_file() {
                return true;
            }
            if meta.is_dir() {
                stack.push(path);
            }
        }
    }
    false
}

fn real_dir(path: &Path) -> bool {
    std::fs::symlink_metadata(path).is_ok_and(|m| m.is_dir() && !m.file_type().is_symlink())
}

/// Every costume slot the project's `romfs/` carries files for, by lowercased fighter name.
///
/// A slot counts when `fighter/<name>/{model,motion}/<part>/cNN` holds at least one file.
/// A missing `romfs` gives an empty map.
pub fn romfs_fighter_slots(romfs: &Path) -> BTreeMap<String, BTreeSet<u8>> {
    let mut out: BTreeMap<String, BTreeSet<u8>> = BTreeMap::new();
    let Ok(fighters) = std::fs::read_dir(romfs.join("fighter")) else {
        return out;
    };
    for fighter in fighters.flatten() {
        let fighter_path = fighter.path();
        if !real_dir(&fighter_path) {
            continue;
        }
        let name = fighter.file_name().to_string_lossy().to_lowercase();
        for kind in ["model", "motion"] {
            let Ok(parts) = std::fs::read_dir(fighter_path.join(kind)) else {
                continue;
            };
            for part in parts.flatten() {
                if !real_dir(&part.path()) {
                    continue;
                }
                let Ok(slots) = std::fs::read_dir(part.path()) else {
                    continue;
                };
                for slot in slots.flatten() {
                    let Some(n) = parse_slot(&slot.file_name().to_string_lossy()) else {
                        continue;
                    };
                    let path = slot.path();
                    if real_dir(&path) && holds_file(&path) {
                        out.entry(name.clone()).or_default().insert(n);
                    }
                }
            }
        }
    }
    out
}

/// Point `entry`'s paths at the project's `romfs/` wherever it has the file, so a file in the
/// project wins over the dump. A part the romfs lacks keeps its dump path. `fighter_dir`,
/// `slots`, `source` and the name fields are never touched.
pub fn apply_romfs_overlay(entry: &mut FighterEntry, romfs: &Path) {
    let fighter = romfs.join("fighter").join(&entry.name);
    let slot = format!("c{:02}", entry.base_slot());
    let model = fighter.join("model").join("body").join(&slot);
    if model.is_dir() {
        entry.model_dir = model;
    }
    let motion = fighter.join("motion").join("body").join(&slot);
    if motion.is_dir() {
        entry.motion_dir = motion;
    }
    let param = fighter.join("param");
    if let Some(path) = ["vl.prc", "fighter_param.prc"]
        .iter()
        .map(|f| param.join(f))
        .find(|p| p.exists())
    {
        entry.param_path = path;
    }
    let effect = romfs.join("effect").join("fighter").join(&entry.name);
    if effect.is_dir() {
        entry.effect_dir = Some(effect);
    }
}

/// Why a fighter row is in the project's list, in the order the list shows them.
/// "Has edits" isn't a reason here: an edited fighter is recorded as `Added`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MembershipReason {
    Added,
    RomfsFiles,
    MovesetScripts,
}

/// What the project knows, for [`membership`].
pub struct MembershipInputs<'a> {
    pub added: &'a BTreeSet<ProjectFighter>,
    pub romfs_slots: &'a BTreeMap<String, BTreeSet<u8>>,
    pub moveset_fighters: &'a BTreeSet<String>,
}

/// The reasons `entry` belongs in the project's list, in [`MembershipReason`] order; empty
/// when it does not. `all_rows` lets a fighter's own row claim romfs slots no character row owns.
pub fn membership(
    entry: &FighterEntry,
    all_rows: &[FighterEntry],
    inputs: &MembershipInputs,
) -> Vec<MembershipReason> {
    let mut reasons = Vec::new();
    let own_row = entry.custom_name.is_none();
    let key = ProjectFighter {
        name: entry.name.clone(),
        character: entry.custom_name.clone(),
    };
    if inputs.added.contains(&key) {
        reasons.push(MembershipReason::Added);
    }
    if let Some(slots) = inputs.romfs_slots.get(&entry.name) {
        let owned = slots.iter().any(|s| entry.slots.contains(s));
        let unowned = own_row
            && slots.iter().any(|s| {
                !all_rows
                    .iter()
                    .any(|row| row.name == entry.name && row.slots.contains(s))
            });
        if owned || unowned {
            reasons.push(MembershipReason::RomfsFiles);
        }
    }
    if own_row && inputs.moveset_fighters.contains(&entry.name) {
        reasons.push(MembershipReason::MovesetScripts);
    }
    reasons
}

/// The fighters a linked moveset has scripts for. The unlabelled key `""` counts for nothing.
pub fn moveset_fighters(index: &crate::acmd_src::SourceIndex) -> BTreeSet<String> {
    index
        .fighters
        .keys()
        .filter(|name| !name.is_empty())
        .cloned()
        .collect()
}

pub fn reason_label(reason: MembershipReason) -> &'static str {
    match reason {
        MembershipReason::Added => "added to project",
        MembershipReason::RomfsFiles => "has files in romfs/",
        MembershipReason::MovesetScripts => "moveset has scripts",
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::data::FighterSource;
    use std::path::PathBuf;

    fn touch(root: &Path, rel: &str) {
        let path = root.join(rel);
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(path, b"x").unwrap();
    }

    fn entry(name: &str, custom: Option<&str>, slots: std::ops::Range<u8>) -> FighterEntry {
        let dir = PathBuf::from("dump/fighter").join(name);
        FighterEntry {
            name: name.to_string(),
            display_name: name.to_string(),
            param_path: dir.join("param/vl.prc"),
            motion_dir: dir.join("motion/body/c00"),
            model_dir: dir.join("model/body/c00"),
            effect_dir: Some(PathBuf::from("dump/effect/fighter").join(name)),
            slots: slots.collect(),
            fighter_dir: dir,
            source: FighterSource::DataRoot,
            custom_name: custom.map(str::to_string),
        }
    }

    #[test]
    fn romfs_scan_finds_model_and_motion_slots() {
        let t = tempfile::tempdir().unwrap();
        touch(t.path(), "fighter/mario/model/body/c00/model.numdlb");
        touch(t.path(), "fighter/mario/motion/body/c03/a.nuanmb");
        let got = romfs_fighter_slots(t.path());
        let want = BTreeMap::from([("mario".to_string(), BTreeSet::from([0u8, 3]))]);
        assert_eq!(got, want);
    }

    #[test]
    fn romfs_scan_ignores_junk() {
        let t = tempfile::tempdir().unwrap();
        assert!(romfs_fighter_slots(&t.path().join("nope")).is_empty());
        touch(t.path(), "fighter/readme.txt");
        touch(t.path(), "fighter/mario/model/body/notaslot/x");
        touch(t.path(), "fighter/mario/model/body/c7/x");
        touch(t.path(), "fighter/mario/model/body/c300/x");
        std::fs::create_dir_all(t.path().join("fighter/luigi/model/body/c00")).unwrap();
        assert!(romfs_fighter_slots(t.path()).is_empty());
    }

    #[test]
    fn overlay_redirects_existing_parts() {
        let t = tempfile::tempdir().unwrap();
        touch(t.path(), "fighter/mario/model/body/c00/x");
        touch(t.path(), "fighter/mario/param/vl.prc");
        let mut e = entry("mario", None, 0..8);
        let before = e.clone();
        apply_romfs_overlay(&mut e, t.path());
        assert_eq!(e.model_dir, t.path().join("fighter/mario/model/body/c00"));
        assert_eq!(e.param_path, t.path().join("fighter/mario/param/vl.prc"));
        assert_eq!(e.motion_dir, before.motion_dir);
        assert_eq!(e.fighter_dir, before.fighter_dir);
        assert_eq!(e.effect_dir, before.effect_dir);
    }

    #[test]
    fn overlay_keeps_dump_paths_for_missing_parts() {
        let t = tempfile::tempdir().unwrap();
        let mut e = entry("mario", None, 0..8);
        let before = e.clone();
        apply_romfs_overlay(&mut e, t.path());
        assert_eq!(e.param_path, before.param_path);
        assert_eq!(e.motion_dir, before.motion_dir);
        assert_eq!(e.model_dir, before.model_dir);
        assert_eq!(e.effect_dir, before.effect_dir);
        assert_eq!(e.fighter_dir, before.fighter_dir);
        assert_eq!(e.slots, before.slots);
    }

    fn rows() -> Vec<FighterEntry> {
        vec![
            entry("mario", None, 0..8),
            entry("pacman", None, 0..8),
            entry("pacman", Some("Shy Guy"), 40..48),
        ]
    }

    fn run(
        added: &[ProjectFighter],
        romfs: &[(&str, &[u8])],
        moveset: &[&str],
    ) -> Vec<Vec<MembershipReason>> {
        let added: BTreeSet<_> = added.iter().cloned().collect();
        let romfs_slots: BTreeMap<_, _> = romfs
            .iter()
            .map(|(n, s)| (n.to_string(), s.iter().copied().collect()))
            .collect();
        let moveset_fighters: BTreeSet<_> = moveset.iter().map(|s| s.to_string()).collect();
        let inputs = MembershipInputs {
            added: &added,
            romfs_slots: &romfs_slots,
            moveset_fighters: &moveset_fighters,
        };
        let rows = rows();
        rows.iter().map(|r| membership(r, &rows, &inputs)).collect()
    }

    #[test]
    fn membership_by_row() {
        let got = run(&[ProjectFighter::character("pacman", "Shy Guy")], &[], &[]);
        assert_eq!(got[2], vec![MembershipReason::Added]);
        assert!(got[1].is_empty());
        assert!(got[0].is_empty());
    }

    #[test]
    fn romfs_slot_matches_owning_row() {
        let got = run(&[], &[("pacman", &[40])], &[]);
        assert_eq!(got[2], vec![MembershipReason::RomfsFiles]);
        assert!(got[1].is_empty());
        let got = run(&[], &[("pacman", &[99])], &[]);
        assert_eq!(got[1], vec![MembershipReason::RomfsFiles]);
        assert!(got[2].is_empty());
    }

    #[test]
    fn moveset_marks_own_row_only() {
        let got = run(&[], &[], &["pacman"]);
        assert_eq!(got[1], vec![MembershipReason::MovesetScripts]);
        assert!(got[2].is_empty());
        assert!(got[0].is_empty());
    }

    #[test]
    fn membership_ignores_unknown_rows() {
        let got = run(&[ProjectFighter::fighter("ghost")], &[], &[]);
        assert!(got.iter().all(Vec::is_empty));
    }

    #[test]
    fn reasons_come_in_enum_order() {
        let got = run(&[ProjectFighter::fighter("mario")], &[("mario", &[2])], &["mario"]);
        assert_eq!(
            got[0],
            vec![
                MembershipReason::Added,
                MembershipReason::RomfsFiles,
                MembershipReason::MovesetScripts
            ]
        );
    }

    #[test]
    fn unlabelled_moveset_counts_for_nothing() {
        let mut index = crate::acmd_src::SourceIndex::default();
        index.fighters.insert(String::new(), Default::default());
        assert!(moveset_fighters(&index).is_empty());
        index.fighters.insert("mario".into(), Default::default());
        assert_eq!(moveset_fighters(&index), BTreeSet::from(["mario".to_string()]));
    }
}
