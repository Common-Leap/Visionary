//! The files ARCropolis actually loads for one costume folder.
//!
//! A mod's `fighter/<name>/model/body/c03` rarely holds every file the game reads from that
//! path. ARCropolis takes each file from, in order:
//!
//! 1. the mod's own folder;
//! 2. a `config.json` share (`share-to-vanilla` or `share-to-added`), which points the slot's
//!    file at another game path, usually vanilla `c00` and sometimes another fighter entirely
//!    (Shy Guy's `pacman/motion/body/c40` borrows 36 of Villager's animations);
//! 3. the game's own file at that path, which exists only for vanilla slots.
//!
//! Visionary used to read only the first, so a texture-only recolour rendered with no mesh and a
//! slot's shared animations were missing from its move list. [`CostumeFolder`] applies all three,
//! reading shares from the mod root's `config.json` and the game's files from the vanilla dump.

use std::collections::{BTreeMap, HashMap};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, OnceLock};
use std::time::SystemTime;

/// Where a costume file was found.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum FileSource {
    /// In the folder itself.
    Own,
    /// A `config.json` share of this game path (`fighter/pacman/model/body/c00/model.xmb`).
    Shared(String),
    /// The game's own file at this path, read from the vanilla dump.
    Vanilla,
}

#[derive(Debug, Clone)]
pub struct ResolvedFile {
    /// The name the game loads it under in this folder.
    pub name: String,
    /// The file to read.
    pub path: PathBuf,
    pub source: FileSource,
}

/// One costume folder's files after shares and the vanilla fallback are applied. The folder
/// itself need not exist: a vanilla slot a mod doesn't touch is made entirely of game files.
#[derive(Debug, Clone, Default)]
pub struct CostumeFolder {
    /// Keyed by lowercase name. ARCropolis matches paths by hash, which ignores case.
    files: BTreeMap<String, ResolvedFile>,
}

impl CostumeFolder {
    /// Resolve `dir` against the vanilla dump the user configured, if any.
    pub fn resolve(dir: &Path) -> Self {
        Self::resolve_with(dir, configured_vanilla_root().as_deref())
    }

    pub fn resolve_with(dir: &Path, vanilla_root: Option<&Path>) -> Self {
        let mut files = BTreeMap::new();
        for (name, path) in list_files(dir) {
            files.entry(name.to_lowercase()).or_insert(ResolvedFile {
                name,
                path,
                source: FileSource::Own,
            });
        }

        let Some((root, game_dir)) = split_root(dir) else {
            return Self { files };
        };
        // A vanilla dump opened as the data root is its own fallback; nothing to add.
        let vanilla_root = vanilla_root.filter(|vanilla| !same_dir(vanilla, &root));

        if let Some(index) = share_index(&root) {
            if let Some(shares) = index.by_dir.get(&game_dir.to_lowercase()) {
                for (name, source) in shares {
                    let key = name.to_lowercase();
                    if files.contains_key(&key) {
                        continue;
                    }
                    if let Some(path) = resolve_source(&root, vanilla_root, &index, source, 0) {
                        files.insert(
                            key,
                            ResolvedFile {
                                name: name.clone(),
                                path,
                                source: FileSource::Shared(source.clone()),
                            },
                        );
                    }
                }
            }
        }

        if let Some(vanilla_root) = vanilla_root {
            for (name, path) in list_files(&game_path_to_fs(vanilla_root, &game_dir)) {
                files.entry(name.to_lowercase()).or_insert(ResolvedFile {
                    name,
                    path,
                    source: FileSource::Vanilla,
                });
            }
        }

        Self { files }
    }

    /// The file loaded under `name`, ignoring case.
    pub fn get(&self, name: &str) -> Option<&Path> {
        self.files
            .get(&name.to_lowercase())
            .map(|file| file.path.as_path())
    }

    #[cfg(test)]
    pub fn files(&self) -> impl Iterator<Item = &ResolvedFile> {
        self.files.values()
    }

    /// `(name, path)` of every file whose name ends in `.extension`, ignoring case.
    pub fn with_extension<'a>(
        &'a self,
        extension: &'a str,
    ) -> impl Iterator<Item = (&'a str, &'a Path)> + 'a {
        self.files.values().filter_map(move |file| {
            Path::new(&file.name)
                .extension()
                .and_then(|e| e.to_str())
                .is_some_and(|e| e.eq_ignore_ascii_case(extension))
                .then_some((file.name.as_str(), file.path.as_path()))
        })
    }

    /// The files that come from somewhere other than the folder itself.
    pub fn borrowed(&self) -> impl Iterator<Item = &ResolvedFile> {
        self.files
            .values()
            .filter(|file| file.source != FileSource::Own)
    }

    /// One line on where the borrowed files come from, or `None` when there are none:
    /// `9 files shared from pacman/model/body/c00 · 4 from the game`.
    pub fn summary(&self) -> Option<String> {
        let mut shared: BTreeMap<String, usize> = BTreeMap::new();
        let mut vanilla = 0;
        for file in self.borrowed() {
            match &file.source {
                FileSource::Shared(source) => {
                    *shared.entry(short_game_dir(source)).or_default() += 1;
                }
                FileSource::Vanilla => vanilla += 1,
                FileSource::Own => {}
            }
        }
        let mut parts: Vec<String> = Vec::new();
        let shared_total: usize = shared.values().sum();
        if shared_total > 0 {
            let from = if shared.len() == 1 {
                format!(" from {}", shared.keys().next().unwrap())
            } else {
                format!(" from {} folders", shared.len())
            };
            parts.push(format!("{} shared{from}", files_word(shared_total)));
        }
        if vanilla > 0 {
            parts.push(format!("{} from the game", files_word(vanilla)));
        }
        (!parts.is_empty()).then(|| parts.join(" · "))
    }

    /// Every borrowed file and where it comes from, one per line, for a hover.
    pub fn detail(&self) -> String {
        self.borrowed()
            .map(|file| match &file.source {
                FileSource::Shared(source) => format!("{} ← {source} (config.json)", file.name),
                FileSource::Vanilla => format!("{} ← the game's own file", file.name),
                FileSource::Own => unreachable!(),
            })
            .collect::<Vec<_>>()
            .join("\n")
    }

    /// `(name, path)` pairs for [`ssbh_wgpu::ModelFolder::load_files`].
    pub fn model_files(&self) -> Vec<(String, PathBuf)> {
        self.files
            .values()
            .map(|file| (file.name.clone(), file.path.clone()))
            .collect()
    }
}

/// The vanilla dump set in the effect editor. Tests never see it, so they can't depend on the
/// machine they run on.
fn configured_vanilla_root() -> Option<PathBuf> {
    if cfg!(test) {
        return None;
    }
    crate::app::load_config_path(crate::app::VANILLA_ROOT_CONFIG_KEY)
}

/// The part folders under `parent` (`fighter/<name>/model` → `body`, `sword`, ...), including
/// parts only the vanilla dump has, as paths under `parent` for [`CostumeFolder::resolve`].
/// Sorted by name.
pub fn sibling_dirs(parent: &Path) -> Vec<PathBuf> {
    sibling_dirs_with(parent, configured_vanilla_root().as_deref())
}

fn sibling_dirs_with(parent: &Path, vanilla_root: Option<&Path>) -> Vec<PathBuf> {
    let subdirs = |dir: &Path| -> Vec<String> {
        std::fs::read_dir(dir)
            .into_iter()
            .flatten()
            .flatten()
            .filter(|entry| entry.path().is_dir())
            .filter_map(|entry| entry.file_name().to_str().map(str::to_string))
            .collect()
    };
    let mut names: BTreeMap<String, String> = subdirs(parent)
        .into_iter()
        .map(|name| (name.to_lowercase(), name))
        .collect();
    if let (Some((root, game_dir)), Some(vanilla)) = (split_root(parent), vanilla_root) {
        if !same_dir(vanilla, &root) {
            for name in subdirs(&game_path_to_fs(vanilla, &game_dir)) {
                names.entry(name.to_lowercase()).or_insert(name);
            }
        }
    }
    names.into_values().map(|name| parent.join(name)).collect()
}

fn files_word(count: usize) -> String {
    if count == 1 {
        "1 file".to_string()
    } else {
        format!("{count} files")
    }
}

/// `fighter/pacman/model/body/c00/model.xmb` → `pacman/model/body/c00`.
fn short_game_dir(game_path: &str) -> String {
    let dir = game_path.rsplit_once('/').map_or(game_path, |(dir, _)| dir);
    dir.strip_prefix("fighter/").unwrap_or(dir).to_string()
}

/// The files directly in `dir`, by name.
pub fn list_files(dir: &Path) -> Vec<(String, PathBuf)> {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return Vec::new();
    };
    entries
        .flatten()
        .map(|entry| entry.path())
        .filter(|path| path.is_file())
        .filter_map(|path| Some((path.file_name()?.to_str()?.to_string(), path)))
        .collect()
}

/// Split `<root>/fighter/...` into the root and the game path `fighter/...`.
fn split_root(dir: &Path) -> Option<(PathBuf, String)> {
    let parts: Vec<_> = dir.components().collect();
    let at = parts.iter().rposition(|part| {
        part.as_os_str()
            .to_str()
            .is_some_and(|name| name.eq_ignore_ascii_case("fighter"))
    })?;
    let root: PathBuf = parts[..at].iter().collect();
    let game: Vec<&str> = parts[at..]
        .iter()
        .map(|part| part.as_os_str().to_str())
        .collect::<Option<_>>()?;
    Some((root, game.join("/").to_lowercase()))
}

fn game_path_to_fs(root: &Path, game_path: &str) -> PathBuf {
    game_path
        .split('/')
        .filter(|part| !part.is_empty())
        .fold(root.to_path_buf(), |path, part| path.join(part))
}

fn same_dir(a: &Path, b: &Path) -> bool {
    match (std::fs::canonicalize(a), std::fs::canonicalize(b)) {
        (Ok(a), Ok(b)) => a == b,
        _ => a == b,
    }
}

/// A share's source: the mod's own copy, else another share it is itself the target of, else
/// the game's file.
fn resolve_source(
    root: &Path,
    vanilla_root: Option<&Path>,
    index: &ShareIndex,
    source: &str,
    depth: u8,
) -> Option<PathBuf> {
    let own = game_path_to_fs(root, source);
    if own.is_file() {
        return Some(own);
    }
    if depth < 4 {
        if let Some(next) = index.by_target.get(&source.to_lowercase()) {
            if let Some(path) = resolve_source(root, vanilla_root, index, next, depth + 1) {
                return Some(path);
            }
        }
    }
    vanilla_root
        .map(|vanilla| game_path_to_fs(vanilla, source))
        .filter(|path| path.is_file())
}

/// A `config.json`'s shares, reversed: for each target, the game path it reads.
#[derive(Debug, Default)]
struct ShareIndex {
    /// Lowercase target folder → `(file name, source game path)`.
    by_dir: HashMap<String, Vec<(String, String)>>,
    /// Lowercase target game path → source game path.
    by_target: HashMap<String, String>,
}

impl ShareIndex {
    fn parse(text: &str) -> Option<Self> {
        let config: serde_json::Value = serde_json::from_str(text).ok()?;
        let mut index = Self::default();
        for key in ["share-to-vanilla", "share-to-added"] {
            let Some(shares) = config.get(key).and_then(|v| v.as_object()) else {
                continue;
            };
            for (source, targets) in shares {
                let source = normalize_game_path(source);
                // A target is a single path or a list of them.
                let targets: Vec<&str> = match targets {
                    serde_json::Value::String(target) => vec![target.as_str()],
                    serde_json::Value::Array(list) => {
                        list.iter().filter_map(|t| t.as_str()).collect()
                    }
                    _ => Vec::new(),
                };
                for target in targets {
                    let target = normalize_game_path(target);
                    let Some((dir, name)) = target.rsplit_once('/') else {
                        continue;
                    };
                    index
                        .by_dir
                        .entry(dir.to_lowercase())
                        .or_default()
                        .push((name.to_string(), source.clone()));
                    index
                        .by_target
                        .insert(target.to_lowercase(), source.clone());
                }
            }
        }
        Some(index)
    }
}

fn normalize_game_path(path: &str) -> String {
    path.replace('\\', "/").trim_matches('/').to_string()
}

/// The share index of `root/config.json`, re-read when the file changes.
fn share_index(root: &Path) -> Option<Arc<ShareIndex>> {
    type Cache = Mutex<HashMap<PathBuf, (Option<SystemTime>, Arc<ShareIndex>)>>;
    static CACHE: OnceLock<Cache> = OnceLock::new();

    let config = root.join("config.json");
    let modified = std::fs::metadata(&config).ok()?.modified().ok();
    let cache = CACHE.get_or_init(Default::default);
    if let Some((cached_at, index)) = cache.lock().ok()?.get(&config) {
        if *cached_at == modified {
            return Some(index.clone());
        }
    }
    let index = Arc::new(ShareIndex::parse(&std::fs::read_to_string(&config).ok()?)?);
    cache.lock().ok()?.insert(config, (modified, index.clone()));
    Some(index)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn write(root: &Path, game_path: &str, body: &str) -> PathBuf {
        let path = game_path_to_fs(root, game_path);
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(&path, body).unwrap();
        path
    }

    fn read(folder: &CostumeFolder, name: &str) -> String {
        std::fs::read_to_string(folder.get(name).unwrap()).unwrap()
    }

    #[test]
    fn own_files_win_then_shares_then_the_game() {
        let vanilla = tempfile::tempdir().unwrap();
        let mod_root = tempfile::tempdir().unwrap();
        for name in ["model.numshb", "model.numatb", "model.xmb"] {
            write(
                vanilla.path(),
                &format!("fighter/mario/model/body/c03/{name}"),
                "game c03",
            );
        }
        write(
            vanilla.path(),
            "fighter/mario/model/body/c00/model.nusktb",
            "game c00",
        );
        write(
            mod_root.path(),
            "fighter/mario/model/body/c03/model.numatb",
            "mod",
        );
        std::fs::write(
            mod_root.path().join("config.json"),
            r#"{"share-to-vanilla": {
                "fighter/mario/model/body/c00/model.nusktb": ["fighter/mario/model/body/c03/model.nusktb"],
                "fighter/mario/model/body/c00/model.numatb": ["fighter/mario/model/body/c03/model.numatb"]
            }}"#,
        )
        .unwrap();

        let dir = game_path_to_fs(mod_root.path(), "fighter/mario/model/body/c03");
        let folder = CostumeFolder::resolve_with(&dir, Some(vanilla.path()));

        assert_eq!(read(&folder, "model.numatb"), "mod");
        assert_eq!(read(&folder, "MODEL.NUSKTB"), "game c00");
        assert_eq!(read(&folder, "model.numshb"), "game c03");
        assert_eq!(folder.files().count(), 4);
        assert_eq!(
            folder.summary().as_deref(),
            Some("1 file shared from mario/model/body/c00 · 2 files from the game")
        );
    }

    #[test]
    fn an_added_slot_borrows_from_another_fighter_and_the_mod_itself() {
        let vanilla = tempfile::tempdir().unwrap();
        let mod_root = tempfile::tempdir().unwrap();
        write(
            vanilla.path(),
            "fighter/murabito/motion/body/c00/d00specialn.nuanmb",
            "villager",
        );
        write(
            mod_root.path(),
            "fighter/pacman/motion/body/c40/c00attack11.nuanmb",
            "shy guy",
        );
        write(
            mod_root.path(),
            "fighter/pacman/motion/body/c41/ik.prc",
            "mod c41",
        );
        std::fs::write(
            mod_root.path().join("config.json"),
            r#"{"share-to-added": {
                "fighter/murabito/motion/body/c00/d00specialn.nuanmb": ["fighter/pacman/motion/body/c40/d00specialn.nuanmb"],
                "fighter/pacman/motion/body/c41/ik.prc": "fighter/pacman/motion/body/c40/ik.prc",
                "fighter/pacman/motion/body/c00/missing.nuanmb": ["fighter/pacman/motion/body/c40/missing.nuanmb"]
            }}"#,
        )
        .unwrap();

        let dir = game_path_to_fs(mod_root.path(), "fighter/pacman/motion/body/c40");
        let folder = CostumeFolder::resolve_with(&dir, Some(vanilla.path()));

        assert_eq!(read(&folder, "d00specialn.nuanmb"), "villager");
        assert_eq!(read(&folder, "ik.prc"), "mod c41");
        assert!(
            folder.get("missing.nuanmb").is_none(),
            "a share with no source is skipped"
        );
        assert_eq!(folder.with_extension("NUANMB").count(), 2);
        assert!(folder
            .detail()
            .contains("d00specialn.nuanmb ← fighter/murabito/motion/body/c00/d00specialn.nuanmb"));
    }

    #[test]
    fn a_share_of_a_shared_file_follows_the_chain() {
        let vanilla = tempfile::tempdir().unwrap();
        let mod_root = tempfile::tempdir().unwrap();
        write(
            vanilla.path(),
            "fighter/mario/model/body/c00/model.xmb",
            "game c00",
        );
        std::fs::write(
            mod_root.path().join("config.json"),
            r#"{"share-to-vanilla": {
                "fighter/mario/model/body/c00/model.xmb": ["fighter/mario/model/body/c08/model.xmb"]
            }, "share-to-added": {
                "fighter/mario/model/body/c08/model.xmb": ["fighter/mario/model/body/c09/model.xmb"]
            }}"#,
        )
        .unwrap();
        let dir = game_path_to_fs(mod_root.path(), "fighter/mario/model/body/c09");
        let folder = CostumeFolder::resolve_with(&dir, Some(vanilla.path()));
        assert_eq!(read(&folder, "model.xmb"), "game c00");
    }

    #[test]
    fn the_vanilla_dump_as_data_root_is_only_its_own_files() {
        let vanilla = tempfile::tempdir().unwrap();
        write(
            vanilla.path(),
            "fighter/mario/model/body/c00/model.numshb",
            "game",
        );
        let dir = game_path_to_fs(vanilla.path(), "fighter/mario/model/body/c00");
        let folder = CostumeFolder::resolve_with(&dir, Some(vanilla.path()));
        assert_eq!(folder.files().count(), 1);
        assert!(folder.summary().is_none());
    }

    #[test]
    fn sibling_parts_include_those_only_the_game_has() {
        let vanilla = tempfile::tempdir().unwrap();
        let mod_root = tempfile::tempdir().unwrap();
        write(
            vanilla.path(),
            "fighter/link/model/sword/c00/model.nusktb",
            "game",
        );
        write(
            vanilla.path(),
            "fighter/link/model/body/c00/model.nusktb",
            "game",
        );
        write(
            mod_root.path(),
            "fighter/link/model/Body/c00/model.numatb",
            "mod",
        );
        let parent = game_path_to_fs(mod_root.path(), "fighter/link/model");
        assert_eq!(
            sibling_dirs_with(&parent, Some(vanilla.path())),
            vec![parent.join("Body"), parent.join("sword")]
        );
        let sword = CostumeFolder::resolve_with(&parent.join("sword/c00"), Some(vanilla.path()));
        assert!(sword.get("model.nusktb").is_some());
    }

    #[test]
    fn a_folder_outside_any_fighter_tree_lists_itself() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("model.numshb"), "x").unwrap();
        let folder = CostumeFolder::resolve_with(dir.path(), None);
        assert_eq!(folder.files().count(), 1);
    }
}
