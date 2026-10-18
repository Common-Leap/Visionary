//! Which fighters a project is about.
//!
//! The left fighter list follows the open project: a fighter is in it when the project added
//! it, has files for it in `romfs/`, or links a moveset with scripts for it. This module is the
//! pure logic for that: scanning the project's `romfs/`, pointing a fighter's paths into it, and
//! naming the reasons a row belongs.

use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};

use crate::data::FighterEntry;
use crate::acmd_src::SourceIndex;
use crate::mod_project::{ModProjectFile, ProjectFighter};

/// How a project records its linked moveset crate: relative to the project folder (with `/`
/// separators) when the crate lives inside it, so the pair can move together; otherwise the
/// absolute path as given.
pub fn store_moveset_path(crate_root: &Path, project_dir: &Path) -> String {
    match crate_root.strip_prefix(project_dir) {
        Ok(rel) => rel
            .components()
            .map(|part| part.as_os_str().to_string_lossy())
            .collect::<Vec<_>>()
            .join("/"),
        Err(_) => crate_root.display().to_string(),
    }
}

/// The inverse of [`store_moveset_path`]: an absolute stored path is used as is, a relative
/// one is joined onto the project folder.
pub fn resolve_moveset_path(stored: &str, project_dir: &Path) -> PathBuf {
    let path = Path::new(stored);
    if path.is_absolute() {
        path.to_path_buf()
    } else {
        project_dir.join(path)
    }
}

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
/// A first edit also records the fighter as `Added`; `HasEdits` keeps it listed after the user
/// removes that entry, until the edits are gone.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MembershipReason {
    Added,
    RomfsFiles,
    MovesetScripts,
    HasEdits,
}

/// What the project knows, for [`membership`].
pub struct MembershipInputs<'a> {
    pub added: &'a BTreeSet<ProjectFighter>,
    pub romfs_slots: &'a BTreeMap<String, BTreeSet<u8>>,
    pub moveset_fighters: &'a BTreeSet<String>,
    /// Fighters with edits right now, by name (any case). Edits are kept per fighter and
    /// shared by its slot characters, so they count for the fighter's own row only.
    pub has_edits: &'a BTreeSet<String>,
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
    // The romfs scan lowercases names; the indexed row keeps its folder's spelling.
    if let Some(slots) = inputs.romfs_slots.get(&entry.name.to_lowercase()) {
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
    if own_row
        && inputs
            .has_edits
            .iter()
            .any(|name| name.eq_ignore_ascii_case(&entry.name))
    {
        reasons.push(MembershipReason::HasEdits);
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
        MembershipReason::HasEdits => "has edits",
    }
}

/// Where linking or unlinking a moveset is recorded.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LinkTarget {
    /// In the open project's `modproject.json`; the global link is left alone.
    Project,
    /// In the app config, as before projects carried their own link ("Browse without project").
    Global,
}

pub fn link_target(project_open: bool) -> LinkTarget {
    if project_open {
        LinkTarget::Project
    } else {
        LinkTarget::Global
    }
}

/// Add each fighter that `project` edits now but did not in `previously_edited` to `set`, and
/// return the fighters it edits now (pass them back in on the next build).
///
/// Only the first edit adds: a fighter the user removed from the project while it still has
/// edits stays removed (it stays listed through [`MembershipReason::HasEdits`]). Never removes
/// either, so undoing the last edit leaves the fighter in the project.
pub fn absorb_edited_fighters(
    set: &mut BTreeSet<ProjectFighter>,
    previously_edited: &BTreeSet<String>,
    project: &ModProjectFile,
) -> BTreeSet<String> {
    let edited = edited_fighters(project);
    for name in edited.difference(previously_edited) {
        set.insert(ProjectFighter::fighter(name));
    }
    edited
}

/// The fighters `project` holds edits for.
pub fn edited_fighters(project: &ModProjectFile) -> BTreeSet<String> {
    project
        .fighters
        .iter()
        .filter(|(_, fighter)| fighter.has_edits())
        .map(|(name, _)| name.clone())
        .collect()
}

/// Re-open a loaded project's moveset link with `build`. Returns the path to keep, the index
/// when it built, and the problem to show when it did not. The stored path is always kept: a
/// crate that is missing today (an unplugged drive, a renamed folder) must not be unlinked by
/// the next autosave.
pub fn moveset_on_load(
    stored: Option<PathBuf>,
    build: impl FnOnce(&Path) -> anyhow::Result<SourceIndex>,
) -> (Option<PathBuf>, Option<SourceIndex>, Option<String>) {
    let Some(path) = stored else {
        return (None, None, None);
    };
    match build(&path) {
        Ok(index) => (Some(path), Some(index), None),
        Err(error) => {
            let problem = format!("could not re-open {}: {error}", path.display());
            (Some(path), None, Some(problem))
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::data::FighterSource;
    use std::path::PathBuf;

    #[test]
    fn inside_crate_is_relative() {
        let dir = tempfile::tempdir().unwrap();
        let crate_root = dir.path().join("moveset");
        assert_eq!(store_moveset_path(&crate_root, dir.path()), "moveset");
    }

    #[test]
    fn outside_crate_is_absolute() {
        let proj = tempfile::tempdir().unwrap();
        let other = tempfile::tempdir().unwrap();
        let crate_root = other.path().join("moveset");
        assert_eq!(
            store_moveset_path(&crate_root, proj.path()),
            crate_root.display().to_string()
        );
    }

    #[test]
    fn resolve_round_trips() {
        let proj = tempfile::tempdir().unwrap();
        let other = tempfile::tempdir().unwrap();
        for crate_root in [proj.path().join("a").join("moveset"), other.path().join("moveset")] {
            let stored = store_moveset_path(&crate_root, proj.path());
            assert_eq!(resolve_moveset_path(&stored, proj.path()), crate_root);
        }
    }

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
        run_with_edits(added, romfs, moveset, &[])
    }

    fn run_with_edits(
        added: &[ProjectFighter],
        romfs: &[(&str, &[u8])],
        moveset: &[&str],
        edits: &[&str],
    ) -> Vec<Vec<MembershipReason>> {
        let added: BTreeSet<_> = added.iter().cloned().collect();
        let romfs_slots: BTreeMap<_, _> = romfs
            .iter()
            .map(|(n, s)| (n.to_string(), s.iter().copied().collect()))
            .collect();
        let moveset_fighters: BTreeSet<_> = moveset.iter().map(|s| s.to_string()).collect();
        let has_edits: BTreeSet<_> = edits.iter().map(|s| s.to_string()).collect();
        let inputs = MembershipInputs {
            added: &added,
            romfs_slots: &romfs_slots,
            moveset_fighters: &moveset_fighters,
            has_edits: &has_edits,
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
        let got = run_with_edits(
            &[ProjectFighter::fighter("mario")],
            &[("mario", &[2])],
            &["mario"],
            &["mario"],
        );
        assert_eq!(
            got[0],
            vec![
                MembershipReason::Added,
                MembershipReason::RomfsFiles,
                MembershipReason::MovesetScripts,
                MembershipReason::HasEdits,
            ]
        );
    }

    #[test]
    fn has_edits_reason_on_own_row_only() {
        // Edits are kept per fighter, shared by its slot characters: only the own row shows it,
        // and the name matches whatever its case.
        let got = run_with_edits(&[], &[], &[], &["PacMan"]);
        assert_eq!(got[1], vec![MembershipReason::HasEdits]);
        assert!(got[2].is_empty());
        assert!(got[0].is_empty());
    }

    #[test]
    fn unlabelled_moveset_counts_for_nothing() {
        let mut index = crate::acmd_src::SourceIndex::default();
        index.fighters.insert(String::new(), Default::default());
        assert!(moveset_fighters(&index).is_empty());
        index.fighters.insert("mario".into(), Default::default());
        assert_eq!(moveset_fighters(&index), BTreeSet::from(["mario".to_string()]));
    }

    #[test]
    fn membership_matches_romfs_case_insensitively() {
        let added = BTreeSet::new();
        let romfs_slots = BTreeMap::from([("mario".to_string(), BTreeSet::from([0u8]))]);
        let moveset_fighters = BTreeSet::new();
        let has_edits = BTreeSet::new();
        let inputs = MembershipInputs {
            added: &added,
            romfs_slots: &romfs_slots,
            moveset_fighters: &moveset_fighters,
            has_edits: &has_edits,
        };
        let rows = vec![entry("Mario", None, 0..8)];
        assert_eq!(
            membership(&rows[0], &rows, &inputs),
            vec![MembershipReason::RomfsFiles]
        );
    }

    fn edited(project: &mut crate::mod_project::ModProjectFile, name: &str) {
        project
            .fighters
            .entry(name.to_string())
            .or_default()
            .sound_scripts
            .insert("attack11".into(), Default::default());
    }

    #[test]
    fn editing_adds_fighter_and_survives_undo() {
        let mut set = BTreeSet::new();
        let mut project = crate::mod_project::ModProjectFile::default();
        edited(&mut project, "fox");
        // Built but unedited: not a member.
        project.fighters.entry("mario".into()).or_default();
        let edited_now = absorb_edited_fighters(&mut set, &BTreeSet::new(), &project);
        assert_eq!(set, BTreeSet::from([ProjectFighter::fighter("fox")]));
        assert_eq!(edited_now, BTreeSet::from(["fox".to_string()]));
        // The edit is undone; the next build still keeps fox.
        project.fighters.clear();
        let edited_now = absorb_edited_fighters(&mut set, &edited_now, &project);
        assert_eq!(set, BTreeSet::from([ProjectFighter::fighter("fox")]));
        assert!(edited_now.is_empty());
    }

    #[test]
    fn removed_edited_fighter_stays_removed() {
        let mut set = BTreeSet::new();
        let mut project = crate::mod_project::ModProjectFile::default();
        edited(&mut project, "fox");
        let edited_now = absorb_edited_fighters(&mut set, &BTreeSet::new(), &project);
        // "Remove from project" while fox still has edits.
        set.remove(&ProjectFighter::fighter("fox"));
        let edited_now = absorb_edited_fighters(&mut set, &edited_now, &project);
        assert!(set.is_empty());
        assert_eq!(edited_now, BTreeSet::from(["fox".to_string()]));
        // Undone and edited again: that is a new edit, so fox joins again.
        project.fighters.clear();
        let edited_now = absorb_edited_fighters(&mut set, &edited_now, &project);
        edited(&mut project, "fox");
        absorb_edited_fighters(&mut set, &edited_now, &project);
        assert_eq!(set, BTreeSet::from([ProjectFighter::fighter("fox")]));
    }

    #[test]
    fn project_fighters_round_trip_through_save() {
        let dir = tempfile::tempdir().unwrap();
        let project = crate::mod_project::ModProjectFile {
            project_fighters: BTreeSet::from([ProjectFighter::fighter("ghost")]),
            ..Default::default()
        };
        let path = dir.path().join("modproject.json");
        let portable =
            crate::mod_export::make_portable_project(&project, dir.path(), "assets").unwrap();
        let json = serde_json::to_string_pretty(&portable).unwrap();
        let mut loaded: crate::mod_project::ModProjectFile = serde_json::from_str(&json).unwrap();
        crate::mod_export::resolve_project_assets(&mut loaded, &path).unwrap();
        loaded.seed_project_fighters();
        assert_eq!(
            loaded.project_fighters,
            BTreeSet::from([ProjectFighter::fighter("ghost")])
        );
    }

    #[test]
    fn stale_moveset_path_is_kept() {
        let gone = PathBuf::from("Z:/no/such/moveset");
        let (kept, index, problem) = moveset_on_load(Some(gone.clone()), |_| {
            Err(anyhow::anyhow!("not found"))
        });
        assert_eq!(kept, Some(gone.clone()));
        assert!(index.is_none());
        assert_eq!(
            problem.as_deref(),
            Some(format!("could not re-open {}: not found", gone.display()).as_str())
        );

        let (kept, index, problem) =
            moveset_on_load(Some(gone.clone()), |_| Ok(crate::acmd_src::SourceIndex::default()));
        assert_eq!(kept, Some(gone));
        assert!(index.is_some());
        assert!(problem.is_none());

        let (kept, index, problem) = moveset_on_load(None, |_| unreachable!());
        assert!(kept.is_none() && index.is_none() && problem.is_none());
    }

    #[test]
    fn linking_in_project_mode_leaves_global_alone() {
        assert_eq!(link_target(true), LinkTarget::Project);
        assert_eq!(link_target(false), LinkTarget::Global);
    }
}
