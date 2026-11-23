//! The parts of a character a project can change, and where each part's files live.
//!
//! By default a part's files live in the project's `romfs/` copy of the game's folders. Any
//! part can instead point at a folder of the author's choosing, for files made with other
//! tools: the editor then reads, previews, adds to and exports that folder in place of the
//! project's own. The choices are saved beside the project in `parts.json`, so the project
//! file stays about edits and this file about where things are.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use anyhow::Context as _;
use serde::{Deserialize, Serialize};

/// The file beside `modproject.json` holding which characters the project shows and where
/// each part's folder is.
pub const PARTS_FILE: &str = "parts.json";

/// The key the project-wide effect folder (common, item and stage effects) is stored under.
pub const OTHER_EFFECTS: &str = "";

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Part {
    Moves,
    Effects,
    Sounds,
    Gimmicks,
    Values,
    Model,
    Animations,
    SelectScreen,
}

impl Part {
    pub const ALL: [Self; 8] = [
        Self::Moves,
        Self::Gimmicks,
        Self::Effects,
        Self::Sounds,
        Self::Model,
        Self::Animations,
        Self::Values,
        Self::SelectScreen,
    ];

    pub fn title(self) -> &'static str {
        match self {
            Self::Moves => "Moves",
            Self::Effects => "Effects",
            Self::Sounds => "Sounds",
            Self::Gimmicks => "Gimmicks",
            Self::Values => "Fighter values",
            Self::Model => "Model",
            Self::Animations => "Animations",
            Self::SelectScreen => "Select screen",
        }
    }

    /// Whether this part's files can live in a folder the author picks.
    pub fn has_folder(self) -> bool {
        !matches!(self, Self::Gimmicks | Self::Values | Self::SelectScreen)
    }

    /// Whether the folder belongs to one costume rather than the whole fighter.
    pub fn per_costume(self) -> bool {
        matches!(self, Self::Model | Self::Animations)
    }

    /// The game folder this part's files go to, for `fighter` (and `slot` when per costume).
    /// Moves have none: their folder is move code, which is built, not copied.
    pub fn game_dir(self, fighter: &str, slot: u8) -> Option<PathBuf> {
        let costume = format!("c{slot:02}");
        Some(match self {
            Self::Model => ["fighter", fighter, "model", "body", &costume]
                .iter()
                .collect(),
            Self::Animations => ["fighter", fighter, "motion", "body", &costume]
                .iter()
                .collect(),
            Self::Effects if fighter == OTHER_EFFECTS => PathBuf::from("effect"),
            Self::Effects => ["effect", "fighter", fighter].iter().collect(),
            Self::Sounds => ["sound", "bank"].iter().collect(),
            Self::Moves | Self::Gimmicks | Self::Values | Self::SelectScreen => return None,
        })
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PartLink {
    pub fighter: String,
    pub part: Part,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub slot: Option<u8>,
    pub folder: PathBuf,
}

/// A character made in the project: its own folder holds every file of its costumes, so its
/// model, animation, effect and sound folders are inside it, never the base fighter's.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct MadeFolder {
    /// Its select screen key, `"mario#c08"`.
    pub key: String,
    /// The fighter it plays as.
    pub fighter: String,
    pub slots: Vec<u8>,
    /// Its folder, relative to the project when inside it.
    pub root: PathBuf,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct ProjectParts {
    /// Characters shown on the project page, with the costume each one shows. Keyed by the
    /// card's select screen key: `"mario"` for a fighter, `"mario#c08"` for a made character.
    #[serde(default)]
    pub characters: BTreeMap<String, u8>,
    #[serde(default)]
    pub links: Vec<PartLink>,
    /// The project's made characters and their folders, written from the project whenever it
    /// changes so code that only has the project folder resolves their files too.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub made: Vec<MadeFolder>,
}

/// `path` as stored in the project: relative when it is inside `workspace`.
pub fn project_relative(workspace: &Path, path: &Path) -> PathBuf {
    path.strip_prefix(workspace)
        .map_or_else(|_| path.to_path_buf(), Path::to_path_buf)
}

/// A path stored in the project, on disk. Older projects stored absolute paths, which stay.
pub fn project_absolute(workspace: &Path, path: &Path) -> PathBuf {
    if path.is_absolute() {
        path.to_path_buf()
    } else {
        workspace.join(path)
    }
}

impl ProjectParts {
    pub fn load(workspace: &Path) -> Self {
        std::fs::read_to_string(workspace.join(PARTS_FILE))
            .ok()
            .and_then(|text| serde_json::from_str(&text).ok())
            .unwrap_or_default()
    }

    pub fn save(&self, workspace: &Path) -> anyhow::Result<()> {
        let path = workspace.join(PARTS_FILE);
        // One that cannot be read was loaded as empty; saving would put that in its place.
        match std::fs::read(&path) {
            Ok(bytes) => {
                serde_json::from_slice::<Self>(&bytes)
                    .with_context(|| format!("{} could not be read", path.display()))?;
            }
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
            Err(error) => return Err(error).with_context(|| format!("reading {}", path.display())),
        }
        if *self == Self::default() {
            if path.exists() {
                std::fs::remove_file(path)?;
            }
            return Ok(());
        }
        crate::user_files::write(&path, serde_json::to_string_pretty(self)?)
    }

    fn key(part: Part, slot: u8) -> Option<u8> {
        part.per_costume().then_some(slot)
    }

    /// The made character owning costume `slot` of `fighter`, if any.
    pub fn made_for(&self, fighter: &str, slot: u8) -> Option<&MadeFolder> {
        self.made
            .iter()
            .find(|made| made.fighter.eq_ignore_ascii_case(fighter) && made.slots.contains(&slot))
    }

    /// Replace the made characters' folders; true when that changed anything.
    pub fn set_made(&mut self, made: Vec<MadeFolder>) -> bool {
        if self.made == made {
            return false;
        }
        self.made = made;
        true
    }

    /// The folder the author picked for this part, if any. Move code is one folder for the
    /// whole project, so it is stored once, under no fighter. A made character's effects and
    /// sounds are always its own: a folder picked for the base fighter's is not theirs.
    pub fn link(&self, fighter: &str, part: Part, slot: u8) -> Option<&Path> {
        if part != Part::Moves && !part.per_costume() && self.made_for(fighter, slot).is_some() {
            return None;
        }
        let fighter = if part == Part::Moves { "" } else { fighter };
        self.links
            .iter()
            .find(|link| {
                link.part == part
                    && link.fighter.eq_ignore_ascii_case(fighter)
                    && link.slot == Self::key(part, slot)
            })
            .map(|link| link.folder.as_path())
    }

    pub fn set_link(&mut self, fighter: &str, part: Part, slot: u8, folder: Option<PathBuf>) {
        let fighter = if part == Part::Moves {
            String::new()
        } else {
            fighter.to_ascii_lowercase()
        };
        let slot = Self::key(part, slot);
        self.links
            .retain(|link| !(link.part == part && link.fighter == fighter && link.slot == slot));
        if let Some(folder) = folder {
            self.links.push(PartLink {
                fighter,
                part,
                slot,
                folder,
            });
        }
    }

    /// Where this part's files are read from and added to: the picked folder, else a made
    /// character's own folder, else the project's own copy of the game folder.
    pub fn dir(&self, workspace: &Path, fighter: &str, part: Part, slot: u8) -> Option<PathBuf> {
        if let Some(folder) = self.link(fighter, part, slot) {
            return Some(folder.to_path_buf());
        }
        let game = part.game_dir(&fighter.to_ascii_lowercase(), slot)?;
        Some(match self.made_for(fighter, slot) {
            Some(made) => project_absolute(workspace, &made.root).join(game),
            None => crate::project_hub::workspace_romfs(workspace).join(game),
        })
    }

    /// Each picked folder and the game folder its files ship to.
    pub fn shipped_folders(&self) -> Vec<(PathBuf, PathBuf)> {
        self.links
            .iter()
            .filter_map(|link| {
                let target = link.part.game_dir(&link.fighter, link.slot.unwrap_or(0))?;
                Some((link.folder.clone(), target))
            })
            .collect()
    }
}

/// Where a sound file ships: voices to the voice bank, everything else to the fighter bank.
pub fn sound_target(file_name: &str) -> &'static str {
    if file_name.to_ascii_lowercase().starts_with("vc_") {
        "fighter_voice"
    } else {
        "fighter"
    }
}

/// Copy a picked folder into an exported mod at `target` (a game folder). Sound files are
/// sorted into their bank; every other part keeps its subfolders as they are. Files the
/// export already wrote win, and are returned so the export can say so, unless they are the
/// same file.
pub fn ship_folder(
    folder: &Path,
    target: &Path,
    dest: &Path,
) -> anyhow::Result<(usize, Vec<String>)> {
    let sounds = target == Path::new("sound/bank");
    let mut copied = 0;
    let mut kept = Vec::new();
    let mut stack = vec![folder.to_path_buf()];
    while let Some(dir) = stack.pop() {
        for entry in std::fs::read_dir(&dir)?.flatten() {
            let path = entry.path();
            if path.is_dir() {
                if !sounds {
                    stack.push(path);
                }
                continue;
            }
            let relative = path.strip_prefix(folder)?;
            let out = if sounds {
                let name = relative.to_string_lossy();
                dest.join(target).join(sound_target(&name)).join(relative)
            } else {
                dest.join(target).join(relative)
            };
            if out.exists() {
                if !crate::project_hub::same_file_contents(&path, &out) {
                    kept.push(
                        out.strip_prefix(dest)
                            .unwrap_or(&out)
                            .to_string_lossy()
                            .replace('\\', "/"),
                    );
                }
                continue;
            }
            if let Some(parent) = out.parent() {
                std::fs::create_dir_all(parent)?;
            }
            std::fs::copy(&path, &out)?;
            copied += 1;
        }
    }
    Ok((copied, kept))
}

/// What shipping a project's own files into an exported mod did.
#[derive(Debug, Default)]
pub struct ShippedFiles {
    pub report: Vec<String>,
    /// Files of the project the mod does not use, because it already has a different file
    /// there. The rest of the mod is unaffected, so these never stop the export.
    pub warnings: Vec<String>,
    /// Folders that could not be read or written.
    pub errors: Vec<String>,
}

/// Ship the project's own files into `dest`, after the export wrote the files it makes: each
/// folder picked for a part first, then the project's `romfs/` folder. A picked folder is
/// where the editor shows that part's files from, so it wins over an older copy in `romfs/`.
pub fn ship_project_files(workspace: &Path, dest: &Path) -> ShippedFiles {
    let mut out = ShippedFiles::default();
    for (folder, target) in ProjectParts::load(workspace).shipped_folders() {
        match ship_folder(&folder, &target, dest) {
            Ok((copied, kept)) => {
                if copied > 0 {
                    out.report.push(format!(
                        "{copied} file(s) from {} to {}",
                        folder.display(),
                        target.display()
                    ));
                }
                for path in kept {
                    out.warnings.push(format!(
                        "{path} from {} is not in the mod: the export made its own",
                        folder.display()
                    ));
                }
            }
            Err(e) => out.errors.push(format!("{}: {e:#}", folder.display())),
        }
    }
    let overlay = crate::project_hub::workspace_romfs(workspace);
    if overlay.is_dir() {
        match crate::project_hub::merge_romfs_overlay(&overlay, dest) {
            Ok((copied, skipped)) => {
                if copied > 0 {
                    out.report.push(format!(
                        "Workspace overlay — {copied} manual file{} (models/animations/…) from romfs/",
                        if copied == 1 { "" } else { "s" }
                    ));
                }
                for path in skipped {
                    out.warnings.push(format!(
                        "romfs/{path} is not in the mod: it already has a different {path}"
                    ));
                }
            }
            Err(e) => out.errors.push(format!("workspace overlay failed: {e:#}")),
        }
    }
    out
}

/// The newest change time under `dir` (one level of subfolders deep), or `None` when empty.
pub fn newest_change(dir: &Path) -> Option<std::time::SystemTime> {
    let mut newest = None;
    let mut stack = vec![(dir.to_path_buf(), 0)];
    while let Some((dir, depth)) = stack.pop() {
        for entry in std::fs::read_dir(&dir).into_iter().flatten().flatten() {
            let Ok(meta) = entry.metadata() else { continue };
            if meta.is_dir() {
                if depth < 1 {
                    stack.push((entry.path(), depth + 1));
                }
            } else if let Ok(at) = meta.modified() {
                newest = newest.max(Some(at));
            }
        }
    }
    newest
}

#[cfg(test)]
mod tests {
    use super::*;

    fn scratch(name: &str) -> PathBuf {
        let dir =
            std::env::temp_dir().join(format!("visionary_parts_{name}_{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    #[test]
    fn a_picked_folder_replaces_the_project_copy_for_its_costume_only() {
        let workspace = scratch("link");
        let mut parts = ProjectParts::default();
        parts.set_link("Pickel", Part::Model, 7, Some(PathBuf::from("/mods/cape")));
        assert_eq!(
            parts.dir(&workspace, "pickel", Part::Model, 7),
            Some(PathBuf::from("/mods/cape"))
        );
        assert_eq!(
            parts.dir(&workspace, "pickel", Part::Model, 0),
            Some(workspace.join("romfs/fighter/pickel/model/body/c00"))
        );
        // Effects are one folder per fighter, whatever costume is shown.
        parts.set_link("pickel", Part::Effects, 3, Some(PathBuf::from("/fx")));
        assert_eq!(
            parts.link("pickel", Part::Effects, 5),
            Some(Path::new("/fx"))
        );
        // Move code is one folder for the project.
        parts.set_link("mario", Part::Moves, 0, Some(PathBuf::from("/src")));
        assert_eq!(
            parts.link("pickel", Part::Moves, 0),
            Some(Path::new("/src"))
        );
        parts.save(&workspace).unwrap();
        assert_eq!(ProjectParts::load(&workspace), parts);
        parts.set_link("pickel", Part::Model, 7, None);
        assert_eq!(parts.link("pickel", Part::Model, 7), None);
        // An empty layout leaves no file behind.
        ProjectParts::default().save(&workspace).unwrap();
        assert!(!workspace.join(PARTS_FILE).exists());
        // One that cannot be read is not replaced with what was loaded in its place: nothing.
        std::fs::write(workspace.join(PARTS_FILE), "{ \"links\": [").unwrap();
        let read = ProjectParts::load(&workspace);
        assert!(read.save(&workspace).is_err());
        assert!(parts.save(&workspace).is_err());
        assert!(workspace.join(PARTS_FILE).exists());
    }

    #[test]
    fn a_made_characters_parts_live_in_its_own_folder_and_the_base_fighters_do_not() {
        let workspace = scratch("made");
        let mut parts = ProjectParts::default();
        parts.set_link("mario", Part::Effects, 0, Some(PathBuf::from("/fx")));
        assert!(parts.set_made(vec![MadeFolder {
            key: "mario#c08".into(),
            fighter: "mario".into(),
            slots: (8..=15).collect(),
            root: PathBuf::from("characters/vision"),
        }]));
        let own = workspace.join("characters/vision");
        assert_eq!(
            parts.dir(&workspace, "mario", Part::Model, 9),
            Some(own.join("fighter/mario/model/body/c09"))
        );
        assert_eq!(
            parts.dir(&workspace, "mario", Part::Animations, 15),
            Some(own.join("fighter/mario/motion/body/c15"))
        );
        assert_eq!(
            parts.dir(&workspace, "mario", Part::Sounds, 8),
            Some(own.join("sound/bank"))
        );
        // The base fighter's picked effect folder is the base fighter's alone.
        assert_eq!(parts.link("mario", Part::Effects, 8), None);
        assert_eq!(
            parts.dir(&workspace, "mario", Part::Effects, 8),
            Some(own.join("effect/fighter/mario"))
        );
        assert_eq!(
            parts.link("mario", Part::Effects, 0),
            Some(Path::new("/fx"))
        );
        assert_eq!(
            parts.dir(&workspace, "mario", Part::Model, 0),
            Some(workspace.join("romfs/fighter/mario/model/body/c00"))
        );
        // A costume folder picked for the made character still wins.
        parts.set_link(
            "mario",
            Part::Model,
            8,
            Some(PathBuf::from("/skins/vision")),
        );
        assert_eq!(
            parts.dir(&workspace, "mario", Part::Model, 8),
            Some(PathBuf::from("/skins/vision"))
        );
        // Unchanged folders are not a change.
        assert!(!parts.set_made(parts.made.clone()));
        parts.save(&workspace).unwrap();
        assert_eq!(ProjectParts::load(&workspace), parts);
    }

    #[test]
    fn project_paths_are_stored_relative_and_old_absolute_ones_still_resolve() {
        let workspace = Path::new("/projects/mine");
        let inside = Path::new("/projects/mine/characters/vision");
        assert_eq!(
            project_relative(workspace, inside),
            PathBuf::from("characters/vision")
        );
        assert_eq!(
            project_absolute(workspace, Path::new("characters/vision")),
            inside
        );
        assert_eq!(project_absolute(workspace, inside), inside);
        let outside = Path::new("/elsewhere/vision");
        assert_eq!(project_relative(workspace, outside), outside);
    }

    #[test]
    fn shipping_sorts_sounds_into_their_bank_and_keeps_files_the_export_wrote() {
        let root = scratch("ship");
        let sounds = root.join("sounds");
        std::fs::create_dir_all(&sounds).unwrap();
        std::fs::write(sounds.join("se_pickel.nus3audio"), b"se").unwrap();
        std::fs::write(sounds.join("vc_pickel.nus3audio"), b"vc").unwrap();
        let effects = root.join("fx");
        std::fs::create_dir_all(effects.join("trail")).unwrap();
        std::fs::write(effects.join("ef_pickel.eff"), b"eff").unwrap();
        std::fs::write(effects.join("trail/tex.nutexb"), b"t").unwrap();
        let dest = root.join("out");
        std::fs::create_dir_all(dest.join("effect/fighter/pickel")).unwrap();
        std::fs::write(dest.join("effect/fighter/pickel/ef_pickel.eff"), b"built").unwrap();

        let (copied, kept) = ship_folder(&sounds, Path::new("sound/bank"), &dest).unwrap();
        assert_eq!((copied, kept.len()), (2, 0));
        assert!(dest
            .join("sound/bank/fighter/se_pickel.nus3audio")
            .is_file());
        assert!(dest
            .join("sound/bank/fighter_voice/vc_pickel.nus3audio")
            .is_file());

        let (copied, kept) =
            ship_folder(&effects, Path::new("effect/fighter/pickel"), &dest).unwrap();
        assert_eq!(copied, 1);
        assert_eq!(kept, vec!["effect/fighter/pickel/ef_pickel.eff".to_owned()]);
        assert_eq!(
            std::fs::read(dest.join("effect/fighter/pickel/ef_pickel.eff")).unwrap(),
            b"built"
        );
        assert!(dest
            .join("effect/fighter/pickel/trail/tex.nutexb")
            .is_file());
    }

    /// A folder linked to Steve's c07 model while the project still has its earlier copy of
    /// the same files in `romfs/`, as in the e2e project.
    fn steve_with_linked_model(name: &str, romfs_model: &[u8]) -> (PathBuf, PathBuf, PathBuf) {
        let root = scratch(name);
        let workspace = root.join("project");
        let linked = root.join("cape/model");
        std::fs::create_dir_all(&linked).unwrap();
        std::fs::write(linked.join("model.numdlb"), b"mesh").unwrap();
        std::fs::write(linked.join("cape.nutexb"), b"cape").unwrap();
        let copy =
            crate::project_hub::workspace_romfs(&workspace).join("fighter/pickel/model/body/c07");
        std::fs::create_dir_all(&copy).unwrap();
        std::fs::write(copy.join("model.numdlb"), romfs_model).unwrap();
        std::fs::write(copy.join("cape.nutexb"), b"cape").unwrap();
        let mut parts = ProjectParts::default();
        parts.set_link("pickel", Part::Model, 7, Some(linked.clone()));
        parts.save(&workspace).unwrap();
        let dest = root.join("mod");
        std::fs::create_dir_all(&dest).unwrap();
        (workspace, linked, dest)
    }

    /// The same files in the linked folder and in `romfs/` are no conflict: nothing to warn
    /// about and nothing that stops the export.
    #[test]
    fn the_same_file_in_a_linked_folder_and_romfs_ships_once_without_a_message() {
        let (workspace, _, dest) = steve_with_linked_model("same", b"mesh");
        let shipped = ship_project_files(&workspace, &dest);
        assert!(shipped.warnings.is_empty(), "{:?}", shipped.warnings);
        assert!(shipped.errors.is_empty(), "{:?}", shipped.errors);
        assert_eq!(
            std::fs::read(dest.join("fighter/pickel/model/body/c07/model.numdlb")).unwrap(),
            b"mesh"
        );
    }

    /// Paired with the above: a different older copy in `romfs/` is a warning, never an
    /// error, and the linked folder (where the editor shows the part from) is what ships.
    #[test]
    fn a_different_romfs_copy_of_a_linked_part_is_a_warning_and_the_link_ships() {
        let (workspace, _, dest) = steve_with_linked_model("differ", b"old mesh");
        let shipped = ship_project_files(&workspace, &dest);
        assert!(shipped.errors.is_empty(), "{:?}", shipped.errors);
        assert_eq!(shipped.warnings.len(), 1, "{:?}", shipped.warnings);
        assert!(shipped.warnings[0].contains("romfs/fighter/pickel/model/body/c07/model.numdlb"));
        assert_eq!(
            std::fs::read(dest.join("fighter/pickel/model/body/c07/model.numdlb")).unwrap(),
            b"mesh"
        );
    }

    /// A folder that cannot be read is still an error.
    #[test]
    fn a_missing_linked_folder_is_an_error() {
        let (workspace, linked, dest) = steve_with_linked_model("missing", b"mesh");
        std::fs::remove_dir_all(&linked).unwrap();
        let shipped = ship_project_files(&workspace, &dest);
        assert_eq!(shipped.errors.len(), 1, "{:?}", shipped.errors);
    }

    /// A file the export made itself wins over a different one in a linked folder, and an
    /// identical one is not reported.
    #[test]
    fn a_linked_file_the_export_also_made_is_reported_only_when_it_differs() {
        let root = scratch("ship_same");
        let effects = root.join("fx");
        std::fs::create_dir_all(&effects).unwrap();
        std::fs::write(effects.join("ef_pickel.eff"), b"built").unwrap();
        let dest = root.join("out");
        std::fs::create_dir_all(dest.join("effect/fighter/pickel")).unwrap();
        std::fs::write(dest.join("effect/fighter/pickel/ef_pickel.eff"), b"built").unwrap();
        let target = Path::new("effect/fighter/pickel");
        assert_eq!(ship_folder(&effects, target, &dest).unwrap(), (0, vec![]));
        std::fs::write(effects.join("ef_pickel.eff"), b"mine").unwrap();
        assert_eq!(
            ship_folder(&effects, target, &dest).unwrap(),
            (0, vec!["effect/fighter/pickel/ef_pickel.eff".to_owned()])
        );
    }
}
