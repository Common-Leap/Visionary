//! Which file on disk an effect file's edits are made on.
//!
//! A game path (`effect/fighter/mario/ef_mario.eff`) can lead to several files: the game's, a
//! mod's, the project's own copy, one in a folder picked for the effects, or one the user opened
//! from anywhere else. Everything that opens, shows, saves, sends or exports effect edits asks
//! here, so they all mean the same one.

use std::path::{Path, PathBuf};

use crate::mod_project::{AuthoredEdit, EffMod, EmitterRoster, EntryEdit, RosterMod};
use crate::project_parts::{Part, ProjectParts, OTHER_EFFECTS};
use crate::roster::RosterKey;

/// Where effect files are read from.
#[derive(Debug, Clone, Default)]
pub struct EffectFiles {
    /// The file each edited effect file's edits are made on, by game path, where the project
    /// records one, or it is a made character's own.
    pub own: Vec<(String, PathBuf)>,
    /// The open project's folder and the folders picked in it.
    pub project: Option<(PathBuf, ProjectParts)>,
    /// Then the linked moveset's files, the game's and the installed mods', in order.
    pub roots: Vec<PathBuf>,
}

impl EffectFiles {
    /// The file the project says `rel`'s edits are made on, there or not.
    fn own(&self, rel: &str) -> Option<&Path> {
        self.own
            .iter()
            .find(|(of, _)| of.eq_ignore_ascii_case(rel))
            .map(|(_, path)| path.as_path())
    }

    /// The project's own copy of `rel`: in a folder picked for the effects, else in its `romfs`.
    fn project_copy(&self, rel: &str) -> Option<PathBuf> {
        let (workspace, parts) = self.project.as_ref()?;
        let rel = rel.replace('\\', "/");
        let picked = match rel
            .strip_prefix("effect/fighter/")
            .and_then(|rest| rest.split_once('/'))
        {
            Some((fighter, rest)) => parts
                .link(fighter, Part::Effects, 0)
                .map(|dir| dir.join(rest)),
            None => rel
                .strip_prefix("effect/")
                .and_then(|rest| Some(parts.link(OTHER_EFFECTS, Part::Effects, 0)?.join(rest))),
        };
        let own = crate::project_hub::workspace_romfs(workspace).join(&rel);
        picked.into_iter().chain([own]).find(|path| path.is_file())
    }

    fn by_game_path(&self, rel: &str) -> Option<PathBuf> {
        self.project_copy(rel)
            .or_else(|| find_effect_file(&self.roots, rel))
    }

    /// The file `rel`'s edits are made on: the recorded one even when it is missing, since
    /// nothing else holds what they change; else what its game path leads to.
    pub fn source(
        &self,
        rel: &str,
        roster: &RosterMod,
        made_root: &dyn Fn(&RosterKey) -> Option<PathBuf>,
    ) -> Option<PathBuf> {
        match self.own(rel) {
            Some(path) => Some(path.to_path_buf()),
            None => self.unrecorded(rel, roster, made_root),
        }
    }

    /// The file the game path `rel` leads to, whatever its edits were made on: the project's own
    /// copy, then the linked moveset's, the game's or a mod's.
    ///
    /// A made character's own costume file comes from its own folder, else from the project's
    /// files, never from a mod that happens to ship the same costume's file; with none, its
    /// costumes load the donor's file.
    pub fn unrecorded(
        &self,
        rel: &str,
        roster: &RosterMod,
        made_root: &dyn Fn(&RosterKey) -> Option<PathBuf>,
    ) -> Option<PathBuf> {
        match made_effect_lookup(rel, roster, made_root) {
            Some(Ok(own)) => Some(own),
            Some(Err(donor_rel)) => self
                .project_copy(rel)
                .or_else(|| self.source(&donor_rel, roster, made_root)),
            None => self.by_game_path(rel),
        }
    }

    /// The file a borrowed effect or an added part is copied from.
    pub fn find(&self, rel: &str) -> Option<PathBuf> {
        match self.own(rel) {
            Some(path) => Some(path.to_path_buf()),
            None => self.by_game_path(rel),
        }
    }

    /// The effect file costume `slot` of `fighter` plays, as ARCropolis loads `ef_pacman_c40.eff`
    /// for Pac-Man's c40, else the fighter's own: its game path and where it is, when it is
    /// anywhere. A slot character's effects are only in its slot's file.
    pub fn of_costume(
        &self,
        fighter: &str,
        slot: u8,
        roster: &RosterMod,
    ) -> Option<(String, PathBuf)> {
        let costume = crate::roster::scaffold::effect_file(fighter, slot);
        let own = format!("effect/fighter/{fighter}/ef_{fighter}.eff");
        // A costume a made character owns plays that character's file, not the fighter's.
        let costume =
            Some(costume).filter(|rel| made_effect_lookup(rel, roster, &|_| None).is_none());
        costume.into_iter().chain([own]).find_map(|rel| {
            let path = self.source(&rel, roster, &|_| None)?;
            Some((rel, path))
        })
    }
}

/// The effect file `rel` (a game path) under the first of `roots` that has it.
fn find_effect_file(roots: &[PathBuf], rel: &str) -> Option<PathBuf> {
    roots
        .iter()
        .map(|root| root.join(rel))
        .find(|path| path.is_file())
}

/// Where the effect file `rel` is read from when it is a made character's own costume file:
/// `Ok` with the file in the character's folder, or, when it has none there, `Err` with the
/// donor's own file to look up instead (what its costumes load without one). `None` for any
/// other file.
fn made_effect_lookup(
    rel: &str,
    roster: &RosterMod,
    made_root: &dyn Fn(&RosterKey) -> Option<PathBuf>,
) -> Option<Result<PathBuf, String>> {
    let rel = rel.replace('\\', "/");
    let owner =
        crate::character::CharacterKey::parse(&crate::character::effect_owner(&rel, roster));
    owner.authored(roster)?;
    let own = made_root(owner.roster_key())
        .map(|root| root.join(&rel))
        .filter(|path| path.is_file());
    let fighter = owner.engine_fighter();
    Some(own.ok_or_else(|| format!("effect/fighter/{fighter}/ef_{fighter}.eff")))
}

/// Say which file `entry`'s first edit is made on, as it is about to be made or was just read
/// from the editor. `open` is the file open for it and whether the user opened it themselves;
/// `game` is the one its game path leads to. Never called for an entry that has edits: those
/// stay with the file they were made on.
pub fn record_first_edit(entry: &mut EffMod, open: Option<(&Path, bool)>, game: Option<&Path>) {
    entry.source_path = open
        // Edits that were all undone leave their file on record: a redo is made on it too.
        .filter(|(path, picked)| *picked || entry.source_path.as_deref() == Some(path))
        .map(|(path, _)| path)
        .filter(|path| Some(*path) != game)
        .map(Path::to_path_buf);
}

/// Put the open file's edits in `entry`, whose game path it is. Which file they are on is settled
/// by the first one: nothing later moves them. `game` is only asked for then.
pub fn fold(
    entry: &mut EffMod,
    rel: String,
    edits: (Vec<AuthoredEdit>, Vec<EmitterRoster>, Vec<EntryEdit>),
    open: Option<(&Path, bool)>,
    game: Option<&Path>,
) {
    let first = entry.is_empty();
    // Transplant records are preserved as they are.
    (entry.authored, entry.rosters, entry.entry_edits) = edits;
    entry.source_rel = rel;
    if first && !entry.is_empty() {
        record_first_edit(entry, open, game);
    }
}

/// What opening an effect file the user picked means for the edits of its game path.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Picked {
    /// It is the file the edits are made on.
    Same,
    /// Another file, and there are no edits: the first one made on it goes to it.
    Fresh,
    /// Another file, and the one the edits were made on is missing.
    StandIn,
    /// Another file, and the edits are on one that is there: they stay on it.
    Refused,
}

pub fn picked(has_edits: bool, own: &Path, path: &Path) -> Picked {
    let real = crate::user_files::real_path;
    if own == path || real(own) == real(path) {
        Picked::Same
    } else if !has_edits {
        Picked::Fresh
    } else if !own.is_file() {
        Picked::StandIn
    } else {
        Picked::Refused
    }
}

/// The first effect, part or texture `eff`'s edits change that `file` does not have.
pub fn missing_from(eff: &EffMod, file: &crate::effects::LoadedEffect) -> Option<String> {
    let sets = &file.ptcl.emitter_sets;
    let set = |name: &str| sets.iter().find(|set| set.name == name);
    let entry = |name: &str| {
        file.entries
            .iter()
            .any(|entry| entry.name.eq_ignore_ascii_case(name))
    };
    // A borrowed effect is put in the file before its edits are applied.
    let borrowed = |name: &str| {
        eff.transplants
            .iter()
            .any(|op| op.new_entry_name.eq_ignore_ascii_case(name))
    };
    for roster in eff.rosters.iter().filter(|r| !borrowed(&r.entry_name)) {
        let Some(set) = set(&roster.set_name) else {
            return Some(roster.entry_name.clone());
        };
        let gone = roster.slots.iter().find(|slot| {
            !slot.is_foreign() && !set.emitters.iter().any(|em| em.name == slot.source_name)
        });
        if let Some(slot) = gone {
            return Some(format!("{} in {}", slot.source_name, roster.entry_name));
        }
    }
    for edit in eff.authored.iter().filter(|e| !borrowed(&e.entry_name)) {
        let Some(set) = set(&edit.set_name) else {
            return Some(edit.entry_name.clone());
        };
        let added = eff.rosters.iter().any(|roster| {
            roster.set_name == edit.set_name
                && roster
                    .slots
                    .iter()
                    .any(|slot| slot.name == edit.emitter_name)
        });
        if !added && !set.emitters.iter().any(|em| em.name == edit.emitter_name) {
            return Some(format!("{} in {}", edit.emitter_name, edit.entry_name));
        }
    }
    let texture = |name: &str| {
        let added = eff.textures_added.iter().map(|t| &t.texture_name);
        let pool = file.ptcl.bntx_textures.iter().map(|t| &t.tex_name);
        added.chain(pool).any(|has| has == name)
    };
    let textures = eff.textures.iter().map(|t| &t.texture_name);
    let textures = textures.chain(&eff.textures_removed);
    let copied = eff.transplants.iter().filter(|op| {
        op.src_file_rel.is_empty() || op.src_file_rel.eq_ignore_ascii_case(&eff.source_rel)
    });
    let edits = eff.entry_edits.iter().map(|edit| &edit.entry_name);
    edits
        .filter(|name| !entry(name) && !borrowed(name))
        .chain(
            copied
                .map(|op| &op.src_set_name)
                .filter(|name| !entry(name)),
        )
        .chain(textures.filter(|name| !texture(name)))
        .next()
        .cloned()
}

#[cfg(test)]
mod tests {
    use super::*;

    const PICKEL: &str = "effect/fighter/pickel/ef_pickel.eff";
    const MARIO: &str = "effect/fighter/mario/ef_mario.eff";

    fn put(path: PathBuf) -> PathBuf {
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(&path, b"eff").unwrap();
        path
    }

    fn edit() -> AuthoredEdit {
        AuthoredEdit {
            set_name: "P_Dash".into(),
            entry_name: "dash".into(),
            set_idx: 0,
            emitter_name: "em0".into(),
            emitter_idx: 0,
            fields: Default::default(),
        }
    }

    fn edited(rel: &str, source_path: Option<PathBuf>) -> EffMod {
        EffMod {
            source_rel: rel.into(),
            source_path,
            authored: vec![edit()],
            ..Default::default()
        }
    }

    fn source(files: &EffectFiles, rel: &str) -> Option<PathBuf> {
        files.source(rel, &RosterMod::default(), &|_| None)
    }

    /// The order: what the project records, the project's own copy (a picked folder's before
    /// its romfs'), the linked moveset's, then the game's. Each is there in turn.
    #[test]
    fn an_effect_files_edits_are_made_on_the_first_file_there_is() {
        let tmp = tempfile::tempdir().unwrap();
        let at = |dir: &str| tmp.path().join(dir);
        let mut files = EffectFiles {
            own: Vec::new(),
            project: Some((at("project"), ProjectParts::default())),
            roots: vec![at("moveset/romfs"), at("dump"), at("mod")],
        };
        assert_eq!(source(&files, PICKEL), None);
        for root in ["mod", "dump", "moveset/romfs", "project/romfs"] {
            let file = put(at(root).join(PICKEL));
            assert_eq!(source(&files, PICKEL), Some(file.clone()));
            assert_eq!(files.find(PICKEL), Some(file));
        }
        let parts = &mut files.project.as_mut().unwrap().1;
        parts.set_link("pickel", Part::Effects, 0, Some(at("picked")));
        // A folder picked that lacks the file changes nothing.
        assert_eq!(
            source(&files, PICKEL),
            Some(at("project/romfs").join(PICKEL))
        );
        let picked = put(at("picked/ef_pickel.eff"));
        assert_eq!(source(&files, PICKEL), Some(picked.clone()));
        let recorded = put(at("downloads/ef_pickel.eff"));
        files.own = vec![(PICKEL.to_uppercase(), recorded.clone())];
        assert_eq!(source(&files, PICKEL), Some(recorded.clone()));
        assert_eq!(files.find(PICKEL), Some(recorded.clone()));
        // What the game path leads to is still known, and other files are not the recorded one.
        let game = files.unrecorded(PICKEL, &RosterMod::default(), &|_| None);
        assert_eq!(game, Some(picked));
        assert_eq!(source(&files, MARIO), None);
        // Missing, it is still where the edits were made: no other file stands for it.
        std::fs::remove_file(&recorded).unwrap();
        assert_eq!(source(&files, PICKEL), Some(recorded.clone()));
        assert_eq!(files.find(PICKEL), Some(recorded));
    }

    /// The shared effects, from the folder picked for the effects of no fighter.
    #[test]
    fn other_effects_come_from_the_folder_picked_for_them() {
        const COMMON: &str = "effect/system/common/ef_common.eff";
        let tmp = tempfile::tempdir().unwrap();
        let picked = put(tmp.path().join("fx/system/common/ef_common.eff"));
        put(tmp.path().join("dump").join(COMMON));
        let mut parts = ProjectParts::default();
        parts.set_link(OTHER_EFFECTS, Part::Effects, 0, Some(tmp.path().join("fx")));
        let files = EffectFiles {
            project: Some((tmp.path().join("project"), parts)),
            roots: vec![tmp.path().join("dump")],
            ..Default::default()
        };
        assert_eq!(source(&files, COMMON), Some(picked));
    }

    /// Review finding 10: Vision's effect file is read from Vision's folder, never from another
    /// mod's copy of the c08 file; without one there, from Mario's own file. Mario's files and
    /// a base costume's file take the ordinary lookup. What the project records is first for
    /// Vision too.
    #[test]
    fn a_made_characters_effect_file_comes_from_its_own_folder() {
        let dir = tempfile::tempdir().unwrap();
        let mut roster = RosterMod::default();
        roster.authored.push(crate::mod_project::AuthoredEntry {
            key: RosterKey::slot("mario", 8),
            donor: "mario".into(),
            slot: 8,
            slots: (9..=15).collect(),
            display_name: "Vision".into(),
            name_id: "vision".into(),
            moveset_scaffolded: false,
            files_root: None,
        });
        let rel = "effect/fighter/mario/ef_mario_c08.eff";
        let root = dir.path().join("characters/vision");
        let folder = |_: &RosterKey| Some(root.clone());
        let game = put(dir.path().join("dump").join(MARIO));
        // Another mod's copy of the costume's file.
        put(dir.path().join("mod").join(rel));
        let mut files = EffectFiles {
            roots: vec![dir.path().join("dump"), dir.path().join("mod")],
            ..Default::default()
        };
        assert_eq!(files.source(rel, &roster, &folder), Some(game.clone()));
        let own = put(root.join(rel));
        assert_eq!(files.source(rel, &roster, &folder), Some(own));
        for (other, found) in [
            (MARIO, Some(game.clone())),
            ("effect/fighter/mario/ef_mario_c03.eff", None),
        ] {
            assert_eq!(files.source(other, &roster, &folder), found, "{other}");
        }
        let recorded = put(dir.path().join("downloads/ef_mario_c08.eff"));
        files.own = vec![(rel.into(), recorded.clone())];
        assert_eq!(files.source(rel, &roster, &folder), Some(recorded));
        // Mario's own row plays no file of Vision's.
        assert_eq!(
            files.of_costume("mario", 8, &roster),
            Some((MARIO.to_owned(), game))
        );
    }

    /// Shy Guy plays on Pac-Man's c40: his effects exist only in the project's
    /// `ef_pacman_c40.eff`, while the dump has Pac-Man's own `ef_pacman.eff`.
    #[test]
    fn a_slot_character_plays_its_slot_file_before_the_fighters_own() {
        const OWN: &str = "effect/fighter/pacman/ef_pacman.eff";
        const SLOT: &str = "effect/fighter/pacman/ef_pacman_c40.eff";
        let tmp = tempfile::tempdir().unwrap();
        let (romfs, dump) = (tmp.path().join("romfs"), tmp.path().join("dump"));
        let slot = put(romfs.join(SLOT));
        let own = put(dump.join(OWN));
        let mut files = EffectFiles {
            roots: vec![romfs.clone(), dump.clone()],
            ..Default::default()
        };
        let roster = RosterMod::default();
        let plays = |files: &EffectFiles, at: u8| files.of_costume("pacman", at, &roster);
        assert_eq!(plays(&files, 40), Some((SLOT.into(), slot.clone())));
        // Pac-Man himself has no slot file and keeps the dump's.
        assert_eq!(plays(&files, 0), Some((OWN.into(), own)));
        // A slot file anywhere beats the fighter's own file in an earlier folder.
        put(romfs.join(OWN));
        std::fs::remove_file(slot).unwrap();
        let slot = put(dump.join(SLOT));
        assert_eq!(plays(&files, 40), Some((SLOT.into(), slot)));
        assert_eq!(files.of_costume("mario", 0, &roster), None);
        // Its recorded file is its file, there or not.
        let gone = tmp.path().join("gone/ef_pacman_c40.eff");
        files.own = vec![(SLOT.into(), gone.clone())];
        assert_eq!(plays(&files, 40), Some((SLOT.into(), gone)));
    }

    /// Linking, unlinking, rescanning and syncing a moveset ask again for the open fighter's
    /// file: a folder picked for its effects and a recorded file stand, whatever the moveset has.
    #[test]
    fn a_moveset_refresh_opens_the_file_the_edits_are_made_on() {
        let tmp = tempfile::tempdir().unwrap();
        let at = |dir: &str| tmp.path().join(dir);
        put(at("moveset/romfs").join(PICKEL));
        put(at("dump").join(PICKEL));
        let picked = put(at("picked/ef_pickel.eff"));
        let mut parts = ProjectParts::default();
        parts.set_link("pickel", Part::Effects, 0, Some(at("picked")));
        let mut files = EffectFiles {
            own: Vec::new(),
            project: Some((at("project"), parts)),
            roots: vec![at("moveset/romfs"), at("dump")],
        };
        let roster = RosterMod::default();
        let plays = |files: &EffectFiles| files.of_costume("pickel", 0, &roster).unwrap().1;
        assert_eq!(plays(&files), picked);
        let outside = at("downloads/ef_pickel.eff");
        files.own = vec![(PICKEL.into(), outside.clone())];
        assert_eq!(plays(&files), outside);
    }

    /// A sync puts the open file's edits in the project and never says they are on another file:
    /// not while the game's copy is open, nor a picked one, nor when its file is missing.
    #[test]
    fn a_sync_never_moves_edits_to_another_file() {
        let outside = PathBuf::from("/downloads/ef_mario.eff");
        let game = PathBuf::from("/dump").join(MARIO);
        for record in [None, Some(outside.clone())] {
            for (open, picked) in [(&game, false), (&outside, false), (&outside, true)] {
                let mut entry = edited(MARIO, record.clone());
                let edits = (vec![edit(), edit()], Vec::new(), Vec::new());
                fold(&mut entry, MARIO.into(), edits, Some((open, picked)), None);
                assert_eq!(entry.source_path, record, "{open:?} {picked}");
                assert_eq!(entry.authored.len(), 2);
            }
        }
    }

    /// The first edit made on a file the user opened themselves records it; one made on the file
    /// its game path leads to records nothing; and looking at a file records nothing.
    #[test]
    fn the_first_edit_records_a_file_the_user_opened() {
        let outside = PathBuf::from("/downloads/ef_mario.eff");
        let game = PathBuf::from("/dump").join(MARIO);
        let edits = || (vec![edit()], Vec::new(), Vec::new());
        let none = || (Vec::new(), Vec::new(), Vec::new());
        let sync = |entry: &mut EffMod, edits, open: &Path, picked| {
            fold(
                entry,
                MARIO.into(),
                edits,
                Some((open, picked)),
                Some(&game),
            );
        };
        let mut entry = EffMod::default();
        sync(&mut entry, none(), &outside, true);
        assert_eq!(entry.source_path, None, "only looked at");
        sync(&mut entry, edits(), &outside, true);
        assert_eq!(entry.source_path.as_ref(), Some(&outside));
        // Undone and done again, it is still that file's, however it was opened meanwhile.
        sync(&mut entry, none(), &outside, true);
        sync(&mut entry, edits(), &outside, false);
        assert_eq!(entry.source_path.as_ref(), Some(&outside));
        // With none left, the first one made on the game's file is on the game's.
        sync(&mut entry, none(), &outside, false);
        sync(&mut entry, edits(), &game, true);
        assert_eq!(entry.source_path, None);
        // A file the editor opened by itself records nothing.
        let mut entry = EffMod::default();
        sync(&mut entry, edits(), &outside, false);
        assert_eq!(entry.source_path, None);
        // A borrowed effect or a replaced texture is an edit like any other.
        let mut entry = EffMod::default();
        record_first_edit(&mut entry, Some((&outside, true)), Some(&game));
        assert_eq!(entry.source_path, Some(outside));
    }

    /// Opening another copy never takes edits from the file they are on; the same file however
    /// it is spelled is that file; and one opened in place of a missing file leaves the record.
    #[test]
    fn opening_another_copy_leaves_edits_where_they_are() {
        let tmp = tempfile::tempdir().unwrap();
        let own = put(tmp.path().join("a/ef_mario.eff"));
        let other = put(tmp.path().join("b/ef_mario.eff"));
        let spelled = tmp.path().join("b/../a/ef_mario.eff");
        for has_edits in [false, true] {
            assert_eq!(picked(has_edits, &own, &own), Picked::Same);
            assert_eq!(picked(has_edits, &own, &spelled), Picked::Same);
        }
        assert_eq!(picked(false, &own, &other), Picked::Fresh);
        assert_eq!(picked(true, &own, &other), Picked::Refused);
        std::fs::remove_file(&own).unwrap();
        assert_eq!(picked(true, &own, &other), Picked::StandIn);
    }

    /// A file opened in place of a missing one has every effect and part the edits change.
    #[test]
    fn a_stand_in_has_what_the_edits_change() {
        let Some(root) = std::env::var_os("VISIONARY_EFF_ROOT").map(PathBuf::from) else {
            eprintln!("skipped: set VISIONARY_EFF_ROOT to the extracted effect/ tree");
            return;
        };
        let file = crate::effects::load_effect(&root.join(MARIO)).unwrap();
        let set = &file.ptcl.emitter_sets[0];
        let part = set.emitters[0].name.clone();
        let on = |set_name: &str, emitter_name: &str| AuthoredEdit {
            set_name: set_name.into(),
            emitter_name: emitter_name.into(),
            ..edit()
        };
        let mut eff = EffMod {
            source_rel: MARIO.into(),
            authored: vec![on(&set.name, &part)],
            ..Default::default()
        };
        assert_eq!(missing_from(&eff, &file), None);
        eff.authored.push(on(&set.name, "ring"));
        assert_eq!(missing_from(&eff, &file).as_deref(), Some("ring in dash"));
        // A part the edits add is in no file.
        eff.rosters.push(EmitterRoster {
            set_name: set.name.clone(),
            entry_name: "dash".into(),
            set_idx: 0,
            slots: vec![crate::mod_project::EmitterSlot {
                source_name: part,
                name: "ring".into(),
                ..Default::default()
            }],
        });
        assert_eq!(missing_from(&eff, &file), None);
        eff.textures_removed.push("not_here".into());
        assert_eq!(missing_from(&eff, &file).as_deref(), Some("not_here"));
        eff.authored.push(on("P_NotHere", "em0"));
        assert_eq!(missing_from(&eff, &file).as_deref(), Some("dash"));
    }
}
