//! Which fighters a project is about.
//!
//! The left fighter list follows the open project: a fighter is in it when the project added
//! it, has files for it in `romfs/`, or links a moveset with scripts for it. This module is the
//! pure logic for that: scanning the project's `romfs/`, pointing a fighter's paths into it, and
//! naming the reasons a row belongs.
//!
//! A moveset's scripts and a fighter's edits count for the fighter's own row, except when the
//! moveset installs that agent only on chosen costumes (`.set_costume(…)`): then they belong to
//! the project's slot character on those costumes, as Empoleon's do on Falco's c80–c87.

use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};

use crate::acmd_src::SourceIndex;
use crate::data::FighterEntry;
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
///
/// `effect_dir` is left alone too: it stays the dump's or a reference mod's folder
/// (`fighter_effect_names` already scans every root). Opening a fighter asks the romfs for
/// its eff first all the same (`fighter_eff_path` in `app.rs`), since a slot character's
/// effects are only in its slot's file there. The editor records that file by its game path
/// (`mod_project::eff_game_rel`), and export writes it under the folder the user picks.
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
    /// shared by its slot characters, so they count for the rows [`script_owners`] names.
    pub has_edits: &'a BTreeSet<String>,
    /// Fighters the linked moveset installs only on chosen costume slots
    /// ([`SourceIndex::costume_gated`]).
    pub costume_gated: &'a BTreeSet<String>,
}

/// The rows a fighter's scripts and edits belong to: its own row, unless the linked moveset
/// installs that fighter's agent only on chosen costumes (`Agent::new("falco")
/// .set_costume(…)`), in which case they are the slot characters this project is about. Those
/// are the characters it added or has `romfs/` files for: Empoleon on Falco's c80–c87, never
/// another mod's character on the same fighter. With none, the fighter's own row holds the
/// slots, so it keeps them.
pub fn script_owners(
    name: &str,
    all_rows: &[FighterEntry],
    inputs: &MembershipInputs,
) -> Vec<ProjectFighter> {
    let gated = inputs
        .costume_gated
        .iter()
        .any(|fighter| fighter.eq_ignore_ascii_case(name));
    if gated {
        let romfs = inputs.romfs_slots.get(&name.to_lowercase());
        let characters: Vec<ProjectFighter> = all_rows
            .iter()
            .filter(|row| row.name.eq_ignore_ascii_case(name))
            .filter_map(|row| {
                let character = row.custom_name.as_ref()?;
                let key = ProjectFighter::character(&row.name, character);
                let in_project = inputs.added.contains(&key)
                    || romfs.is_some_and(|slots| row.slots.iter().any(|s| slots.contains(s)));
                in_project.then_some(key)
            })
            .collect();
        if !characters.is_empty() {
            return characters;
        }
    }
    vec![ProjectFighter::fighter(
        all_rows
            .iter()
            .find(|row| row.custom_name.is_none() && row.name.eq_ignore_ascii_case(name))
            .map_or(name, |row| row.name.as_str()),
    )]
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
    let scripted = inputs.moveset_fighters.contains(&entry.name);
    let edited = inputs
        .has_edits
        .iter()
        .any(|name| name.eq_ignore_ascii_case(&entry.name));
    let owns = (scripted || edited) && script_owners(&entry.name, all_rows, inputs).contains(&key);
    if owns && scripted {
        reasons.push(MembershipReason::MovesetScripts);
    }
    if owns && edited {
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

/// A reason as a verb phrase, to follow "because it" in the list's hover text.
pub fn reason_label(reason: MembershipReason) -> &'static str {
    match reason {
        MembershipReason::Added => "was added to the project",
        MembershipReason::RomfsFiles => "has files in romfs/",
        MembershipReason::MovesetScripts => "has scripts in the moveset",
        MembershipReason::HasEdits => "has edits",
    }
}

/// `reasons` as one phrase for "... because it {phrase}": "a", "a and b", "a, b and c".
pub fn join_reasons(reasons: &[MembershipReason]) -> String {
    let labels: Vec<&str> = reasons.iter().map(|r| reason_label(*r)).collect();
    match labels.split_last() {
        None => String::new(),
        Some((last, [])) => last.to_string(),
        Some((last, rest)) => format!("{} and {last}", rest.join(", ")),
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

/// Add the rows of each fighter that `project` edits now but did not in `previously_edited` to
/// `set`, and return the fighters it edits now (pass them back in on the next build). `owners`
/// names a fighter's rows; see [`script_owners`].
///
/// Only the first edit adds: a fighter the user removed from the project while it still has
/// edits stays removed (it stays listed through [`MembershipReason::HasEdits`]). Never removes
/// either, so undoing the last edit leaves the fighter in the project.
pub fn absorb_edited_fighters(
    set: &mut BTreeSet<ProjectFighter>,
    previously_edited: &BTreeSet<String>,
    project: &ModProjectFile,
    owners: impl Fn(&str) -> Vec<ProjectFighter>,
) -> BTreeSet<String> {
    let edited = edited_fighters(project);
    for name in edited.difference(previously_edited) {
        set.extend(owners(name));
    }
    edited
}

/// Point the fighter rows a v1–v3 project was seeded with ([`ModProjectFile::migrate`]) at the
/// rows that own their edits now that the fighter list and moveset are known: Empoleon's edits
/// were seeded as Falco's own row. Character rows are left as they are.
pub fn reassign_seeded_fighters(
    set: &mut BTreeSet<ProjectFighter>,
    owners: impl Fn(&str) -> Vec<ProjectFighter>,
) {
    let seeded: Vec<ProjectFighter> = set
        .iter()
        .filter(|row| row.character.is_none())
        .cloned()
        .collect();
    for row in seeded {
        let rows = owners(&row.name);
        if !rows.contains(&row) {
            set.remove(&row);
            set.extend(rows);
        }
    }
}

/// The slot characters `roster` authors, as project rows.
pub fn authored_characters(roster: &crate::mod_project::RosterMod) -> BTreeSet<ProjectFighter> {
    roster
        .authored
        .iter()
        .map(|entry| ProjectFighter::character(&entry.donor, &entry.display_name))
        .collect()
}

/// Add each character `roster` authors now but did not in `previously_authored` to `set`, and
/// return the characters it authors now. Like [`absorb_edited_fighters`]: a character made in
/// the Roster window joins the project once, and removing it from the project sticks.
pub fn absorb_authored_characters(
    set: &mut BTreeSet<ProjectFighter>,
    previously_authored: &BTreeSet<ProjectFighter>,
    roster: &crate::mod_project::RosterMod,
) -> BTreeSet<ProjectFighter> {
    let authored = authored_characters(roster);
    for row in authored.difference(previously_authored) {
        set.insert(row.clone());
    }
    authored
}

/// The fighters `project` holds edits for.
pub fn edited_fighters(project: &ModProjectFile) -> BTreeSet<String> {
    // A costume's own effect file is edited under the costume's key, and belongs to its row.
    let costume_files = project.characters.iter().filter(|(key, edits)| {
        let character = crate::character::CharacterKey::made((*key).clone());
        edits.has_edits() && character.costume_file(&project.roster).is_some()
    });
    project
        .fighters
        .iter()
        .filter(|(_, fighter)| fighter.has_edits())
        .map(|(name, _)| name.clone())
        .chain(costume_files.map(|(key, _)| key.as_str().to_owned()))
        .collect()
}

/// The moveset crate a project folder is itself, when it is one: a `Cargo.toml` whose crate has
/// ACMD scripts. Used for a project that records no link, which before projects recorded one
/// was every project kept inside its moveset.
pub fn workspace_moveset(dir: &Path) -> Option<PathBuf> {
    if !dir.join("Cargo.toml").is_file() {
        return None;
    }
    SourceIndex::build(dir)
        .ok()
        .filter(SourceIndex::has_scripts)
        .map(|_| dir.to_path_buf())
}

/// The moveset a just-read project uses: the one it records (`stored`, resolved), else the one
/// its page kept in `parts.json` before projects recorded one, else the crate the project is kept
/// in, else, for a project from before projects kept a link (`keeps_scope`), this machine's: its
/// scripts must not turn vanilla because the link now belongs to the project.
pub fn moveset_of_project(
    stored: Option<PathBuf>,
    dir: Option<&Path>,
    version: u32,
    keeps_scope: bool,
    global: impl FnOnce() -> Option<PathBuf>,
) -> Option<PathBuf> {
    stored
        .or_else(|| {
            let dir = dir.filter(|_| version < 10)?;
            crate::project_parts::ProjectParts::load(dir)
                .link("", crate::project_parts::Part::Moves, 0)
                .map(|folder| crate::project_parts::project_absolute(dir, folder))
        })
        .or_else(|| dir.and_then(workspace_moveset))
        .or_else(|| (!keeps_scope).then(global).flatten())
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

    /// Where a project's moves come from when it records no link of its own: the one its page
    /// kept, the crate it is in, and this machine's link only when it never kept one.
    #[test]
    fn a_project_from_before_links_uses_this_machines() {
        let tmp = tempfile::tempdir().unwrap();
        let dir = tmp.path();
        let global = || Some(PathBuf::from("/machine"));
        let of = |stored: Option<&str>, version: u32, keeps_scope: bool| {
            let stored = stored.map(PathBuf::from);
            moveset_of_project(stored, Some(dir), version, keeps_scope, global)
        };
        // A project from before links, and one the user left without.
        assert_eq!(of(None, 7, false), global());
        assert_eq!(of(None, 8, false), global());
        assert_eq!(of(None, 8, true), None);
        assert_eq!(of(None, 10, true), None);
        assert_eq!(of(Some("/mine"), 10, true), Some(PathBuf::from("/mine")));
        // One whose page named its move code; the project's own link takes over from 10 on.
        let mut parts = crate::project_parts::ProjectParts::default();
        parts.set_link(
            "",
            crate::project_parts::Part::Moves,
            0,
            Some(PathBuf::from("/page")),
        );
        parts.save(dir).unwrap();
        assert_eq!(of(None, 8, false), Some(PathBuf::from("/page")));
        assert_eq!(of(None, 10, true), None);
        assert_eq!(of(Some("/mine"), 8, true), Some(PathBuf::from("/mine")));
    }
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
        for crate_root in [
            proj.path().join("a").join("moveset"),
            other.path().join("moveset"),
        ] {
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
        // A romfs effect folder never redirects: effects stay on the dump.
        touch(t.path(), "effect/fighter/mario/ef_mario.eff");
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
        run_gated(added, romfs, moveset, edits, &[])
    }

    fn run_gated(
        added: &[ProjectFighter],
        romfs: &[(&str, &[u8])],
        moveset: &[&str],
        edits: &[&str],
        gated: &[&str],
    ) -> Vec<Vec<MembershipReason>> {
        let added: BTreeSet<_> = added.iter().cloned().collect();
        let romfs_slots: BTreeMap<_, _> = romfs
            .iter()
            .map(|(n, s)| (n.to_string(), s.iter().copied().collect()))
            .collect();
        let moveset_fighters: BTreeSet<_> = moveset.iter().map(|s| s.to_string()).collect();
        let has_edits: BTreeSet<_> = edits.iter().map(|s| s.to_string()).collect();
        let costume_gated: BTreeSet<_> = gated.iter().map(|s| s.to_string()).collect();
        let inputs = MembershipInputs {
            added: &added,
            romfs_slots: &romfs_slots,
            moveset_fighters: &moveset_fighters,
            has_edits: &has_edits,
            costume_gated: &costume_gated,
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

    /// Editing the effects of a mod's character on a costume adds its row, not its fighter's.
    #[test]
    fn a_costume_files_edits_belong_to_its_own_row() {
        let mut project = ModProjectFile::default();
        let key = crate::roster::RosterKey::slot("pacman", 40);
        project.characters.entry(key).or_default().eff = Some(crate::mod_project::EffMod {
            source_rel: "effect/fighter/pacman/ef_pacman_c40.eff".into(),
            textures_removed: vec!["x".into()],
            ..Default::default()
        });
        let shy_guy = ProjectFighter::character("pacman", "Shy Guy");
        let mut set = BTreeSet::new();
        let edited = absorb_edited_fighters(&mut set, &BTreeSet::new(), &project, |owner| {
            assert_eq!(owner, "pacman#c40");
            vec![shy_guy.clone()]
        });
        assert_eq!(set, BTreeSet::from([shy_guy]));
        assert_eq!(edited, BTreeSet::from(["pacman#c40".to_owned()]));
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
    fn reasons_join_into_one_phrase() {
        use MembershipReason::*;
        assert_eq!(join_reasons(&[]), "");
        assert_eq!(
            join_reasons(&[MovesetScripts]),
            "has scripts in the moveset"
        );
        assert_eq!(
            join_reasons(&[RomfsFiles, HasEdits]),
            "has files in romfs/ and has edits"
        );
        assert_eq!(
            join_reasons(&[Added, RomfsFiles, MovesetScripts]),
            "was added to the project, has files in romfs/ and has scripts in the moveset"
        );
    }

    #[test]
    fn unlabelled_moveset_counts_for_nothing() {
        let mut index = crate::acmd_src::SourceIndex::default();
        index.fighters.insert(String::new(), Default::default());
        assert!(moveset_fighters(&index).is_empty());
        index.fighters.insert("mario".into(), Default::default());
        assert_eq!(
            moveset_fighters(&index),
            BTreeSet::from(["mario".to_string()])
        );
    }

    #[test]
    fn membership_matches_romfs_case_insensitively() {
        let added = BTreeSet::new();
        let romfs_slots = BTreeMap::from([("mario".to_string(), BTreeSet::from([0u8]))]);
        let moveset_fighters = BTreeSet::new();
        let has_edits = BTreeSet::new();
        let costume_gated = BTreeSet::new();
        let inputs = MembershipInputs {
            added: &added,
            romfs_slots: &romfs_slots,
            moveset_fighters: &moveset_fighters,
            has_edits: &has_edits,
            costume_gated: &costume_gated,
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

    fn own_row(name: &str) -> Vec<ProjectFighter> {
        vec![ProjectFighter::fighter(name)]
    }

    /// Empoleon: `Agent::new("pacman").set_costume(…)` stands in for `Agent::new("falco")
    /// .set_costume(…)`. Scripts and edits on a costume-gated agent are the slot character's;
    /// the fighter's own row is not in the project for them.
    #[test]
    fn costume_gated_moveset_belongs_to_the_slot_character() {
        let got = run_gated(
            &[],
            &[("pacman", &[40, 41])],
            &["pacman"],
            &["pacman"],
            &["pacman"],
        );
        assert!(got[1].is_empty(), "own row: {:?}", got[1]);
        assert_eq!(
            got[2],
            vec![
                MembershipReason::RomfsFiles,
                MembershipReason::MovesetScripts,
                MembershipReason::HasEdits,
            ]
        );
        // An added character counts as the project's without any romfs files.
        let got = run_gated(
            &[ProjectFighter::character("pacman", "Shy Guy")],
            &[],
            &["pacman"],
            &[],
            &["pacman"],
        );
        assert!(got[1].is_empty());
        assert_eq!(
            got[2],
            vec![MembershipReason::Added, MembershipReason::MovesetScripts]
        );
    }

    /// Gated, but the project names no character on that fighter (its slots are not split off,
    /// or the character's mod is not this project's): the own row holds the slots and keeps them.
    #[test]
    fn costume_gated_without_a_project_character_stays_on_the_own_row() {
        let got = run_gated(&[], &[], &["pacman"], &["pacman"], &["pacman"]);
        assert_eq!(
            got[1],
            vec![MembershipReason::MovesetScripts, MembershipReason::HasEdits]
        );
        assert!(got[2].is_empty());
        // romfs files on the fighter's own slots only do not make Shy Guy the project's.
        let got = run_gated(&[], &[("pacman", &[3])], &["pacman"], &[], &["pacman"]);
        assert_eq!(
            got[1],
            vec![
                MembershipReason::RomfsFiles,
                MembershipReason::MovesetScripts
            ]
        );
        assert!(got[2].is_empty());
    }

    #[test]
    fn first_edit_on_a_gated_fighter_adds_the_character() {
        let rows = rows();
        let added = BTreeSet::new();
        let romfs_slots = BTreeMap::from([("pacman".to_string(), BTreeSet::from([40u8]))]);
        let empty = BTreeSet::new();
        let gated = BTreeSet::from(["pacman".to_string()]);
        let inputs = MembershipInputs {
            added: &added,
            romfs_slots: &romfs_slots,
            moveset_fighters: &empty,
            has_edits: &empty,
            costume_gated: &gated,
        };
        let mut set = BTreeSet::new();
        let mut project = crate::mod_project::ModProjectFile::default();
        edited(&mut project, "pacman");
        absorb_edited_fighters(&mut set, &BTreeSet::new(), &project, |name| {
            script_owners(name, &rows, &inputs)
        });
        assert_eq!(
            set,
            BTreeSet::from([ProjectFighter::character("pacman", "Shy Guy")])
        );

        // A v3 file seeded the fighter's own row from the same edits.
        let mut seeded = BTreeSet::from([
            ProjectFighter::fighter("pacman"),
            ProjectFighter::fighter("mario"),
        ]);
        reassign_seeded_fighters(&mut seeded, |name| script_owners(name, &rows, &inputs));
        assert_eq!(
            seeded,
            BTreeSet::from([
                ProjectFighter::fighter("mario"),
                ProjectFighter::character("pacman", "Shy Guy"),
            ])
        );
    }

    #[test]
    fn editing_adds_fighter_and_survives_undo() {
        let mut set = BTreeSet::new();
        let mut project = crate::mod_project::ModProjectFile::default();
        edited(&mut project, "fox");
        // Built but unedited: not a member.
        project.fighters.entry("mario".into()).or_default();
        let edited_now = absorb_edited_fighters(&mut set, &BTreeSet::new(), &project, own_row);
        assert_eq!(set, BTreeSet::from([ProjectFighter::fighter("fox")]));
        assert_eq!(edited_now, BTreeSet::from(["fox".to_string()]));
        // The edit is undone; the next build still keeps fox.
        project.fighters.clear();
        let edited_now = absorb_edited_fighters(&mut set, &edited_now, &project, own_row);
        assert_eq!(set, BTreeSet::from([ProjectFighter::fighter("fox")]));
        assert!(edited_now.is_empty());
    }

    #[test]
    fn authored_character_joins_once() {
        let mut set = BTreeSet::new();
        let mut project = crate::mod_project::ModProjectFile::default();
        project
            .roster
            .authored
            .push(crate::mod_project::AuthoredEntry {
                key: crate::roster::RosterKey::slot("pacman", 8),
                donor: "pacman".into(),
                slot: 8,
                slots: Vec::new(),
                display_name: "Shy Guy".into(),
                name_id: "shy_guy".into(),
                moveset_scaffolded: false,
                files_root: None,
            });
        let shy_guy = ProjectFighter::character("pacman", "Shy Guy");
        let authored = absorb_authored_characters(&mut set, &BTreeSet::new(), &project.roster);
        assert_eq!(set, BTreeSet::from([shy_guy.clone()]));
        set.remove(&shy_guy);
        absorb_authored_characters(&mut set, &authored, &project.roster);
        assert!(set.is_empty());
    }

    #[test]
    fn removed_edited_fighter_stays_removed() {
        let mut set = BTreeSet::new();
        let mut project = crate::mod_project::ModProjectFile::default();
        edited(&mut project, "fox");
        let edited_now = absorb_edited_fighters(&mut set, &BTreeSet::new(), &project, own_row);
        // "Remove from project" while fox still has edits.
        set.remove(&ProjectFighter::fighter("fox"));
        let edited_now = absorb_edited_fighters(&mut set, &edited_now, &project, own_row);
        assert!(set.is_empty());
        assert_eq!(edited_now, BTreeSet::from(["fox".to_string()]));
        // Undone and edited again: that is a new edit, so fox joins again.
        project.fighters.clear();
        let edited_now = absorb_edited_fighters(&mut set, &edited_now, &project, own_row);
        edited(&mut project, "fox");
        absorb_edited_fighters(&mut set, &edited_now, &project, own_row);
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

    /// Empoleon keeps `modproject.json` in the moveset crate itself and never recorded a link.
    #[test]
    fn a_project_inside_its_moveset_crate_finds_it() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(
            dir.path().join("Cargo.toml"),
            "[package]\nname = \"empoleon\"\n\n[lib]\ncrate-type = [\"cdylib\"]\n",
        )
        .unwrap();
        std::fs::create_dir_all(dir.path().join("src/emperte")).unwrap();
        std::fs::write(dir.path().join("src/lib.rs"), "mod emperte;\npub fn main() { let a = &mut Agent::new(\"falco\"); emperte::install(a); }\n").unwrap();
        assert_eq!(
            workspace_moveset(dir.path()),
            None,
            "a crate with no scripts is not a moveset"
        );
        std::fs::write(
            dir.path().join("src/emperte/mod.rs"),
            "unsafe extern \"C\" fn game_attackhi3(agent: &mut L2CAgentBase) {}\npub fn install(agent: &mut Agent) { agent.acmd(\"game_attackhi3\", game_attackhi3, Priority::Low); }\n",
        )
        .unwrap();
        assert_eq!(
            workspace_moveset(dir.path()),
            Some(dir.path().to_path_buf())
        );
        assert_eq!(
            workspace_moveset(&dir.path().join("src")),
            None,
            "no Cargo.toml there"
        );
    }

    #[test]
    fn stale_moveset_path_is_kept() {
        let gone = PathBuf::from("Z:/no/such/moveset");
        let (kept, index, problem) =
            moveset_on_load(Some(gone.clone()), |_| Err(anyhow::anyhow!("not found")));
        assert_eq!(kept, Some(gone.clone()));
        assert!(index.is_none());
        assert_eq!(
            problem.as_deref(),
            Some(format!("could not re-open {}: not found", gone.display()).as_str())
        );

        let (kept, index, problem) = moveset_on_load(Some(gone.clone()), |_| {
            Ok(crate::acmd_src::SourceIndex::default())
        });
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
