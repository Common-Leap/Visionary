//! The parts of a character a project can change, and where each part's files live.
//!
//! By default a part's files live in the project's `romfs/` copy of the game's folders. Any
//! part can instead point at a folder of the author's choosing, for files made with other
//! tools: the editor then reads, previews, adds to and exports that folder in place of the
//! project's own. The choices are saved beside the project in `parts.json`, so the project
//! file stays about edits and this file about where things are.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

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
            Self::Model => ["fighter", fighter, "model", "body", &costume].iter().collect(),
            Self::Animations => ["fighter", fighter, "motion", "body", &costume].iter().collect(),
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

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct ProjectParts {
    /// Characters shown on the project page, with the costume each one shows.
    #[serde(default)]
    pub characters: BTreeMap<String, u8>,
    #[serde(default)]
    pub links: Vec<PartLink>,
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
        if *self == Self::default() {
            if path.exists() {
                std::fs::remove_file(path)?;
            }
            return Ok(());
        }
        std::fs::write(path, serde_json::to_string_pretty(self)?)?;
        Ok(())
    }

    fn key(part: Part, slot: u8) -> Option<u8> {
        part.per_costume().then_some(slot)
    }

    /// The folder the author picked for this part, if any. Move code is one folder for the
    /// whole project, so it is stored once, under no fighter.
    pub fn link(&self, fighter: &str, part: Part, slot: u8) -> Option<&Path> {
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
        let fighter = if part == Part::Moves { String::new() } else { fighter.to_ascii_lowercase() };
        let slot = Self::key(part, slot);
        self.links
            .retain(|link| !(link.part == part && link.fighter == fighter && link.slot == slot));
        if let Some(folder) = folder {
            self.links.push(PartLink { fighter, part, slot, folder });
        }
    }

    /// Where this part's files are read from and added to: the picked folder, else the
    /// project's own copy of the game folder.
    pub fn dir(&self, workspace: &Path, fighter: &str, part: Part, slot: u8) -> Option<PathBuf> {
        if let Some(folder) = self.link(fighter, part, slot) {
            return Some(folder.to_path_buf());
        }
        Some(crate::project_hub::workspace_romfs(workspace).join(part.game_dir(fighter, slot)?))
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
/// export already wrote win, and are returned so the export can say so.
pub fn ship_folder(folder: &Path, target: &Path, dest: &Path) -> anyhow::Result<(usize, Vec<String>)> {
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
                kept.push(out.strip_prefix(dest).unwrap_or(&out).display().to_string());
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
        let dir = std::env::temp_dir().join(format!("visionary_parts_{name}_{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    #[test]
    fn a_picked_folder_replaces_the_project_copy_for_its_costume_only() {
        let workspace = scratch("link");
        let mut parts = ProjectParts::default();
        parts.set_link("Pickel", Part::Model, 7, Some(PathBuf::from("/mods/cape")));
        assert_eq!(parts.dir(&workspace, "pickel", Part::Model, 7), Some(PathBuf::from("/mods/cape")));
        assert_eq!(
            parts.dir(&workspace, "pickel", Part::Model, 0),
            Some(workspace.join("romfs/fighter/pickel/model/body/c00"))
        );
        // Effects are one folder per fighter, whatever costume is shown.
        parts.set_link("pickel", Part::Effects, 3, Some(PathBuf::from("/fx")));
        assert_eq!(parts.link("pickel", Part::Effects, 5), Some(Path::new("/fx")));
        // Move code is one folder for the project.
        parts.set_link("mario", Part::Moves, 0, Some(PathBuf::from("/src")));
        assert_eq!(parts.link("pickel", Part::Moves, 0), Some(Path::new("/src")));
        parts.save(&workspace).unwrap();
        assert_eq!(ProjectParts::load(&workspace), parts);
        parts.set_link("pickel", Part::Model, 7, None);
        assert_eq!(parts.link("pickel", Part::Model, 7), None);
        // An empty layout leaves no file behind.
        ProjectParts::default().save(&workspace).unwrap();
        assert!(!workspace.join(PARTS_FILE).exists());
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
        assert!(dest.join("sound/bank/fighter/se_pickel.nus3audio").is_file());
        assert!(dest.join("sound/bank/fighter_voice/vc_pickel.nus3audio").is_file());

        let (copied, kept) = ship_folder(&effects, Path::new("effect/fighter/pickel"), &dest).unwrap();
        assert_eq!(copied, 1);
        assert_eq!(kept, vec!["effect/fighter/pickel/ef_pickel.eff".to_owned()]);
        assert_eq!(std::fs::read(dest.join("effect/fighter/pickel/ef_pickel.eff")).unwrap(), b"built");
        assert!(dest.join("effect/fighter/pickel/trail/tex.nutexb").is_file());
    }
}
