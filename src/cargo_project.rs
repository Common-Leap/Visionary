//! The Cargo layout of a linked moveset, as `cargo metadata` reports it.
//!
//! Moveset repositories are laid out every way there is: the crate at the root, under
//! `plugin/`, one crate per fighter in a workspace, a `[lib] path` pointing somewhere unusual.
//! Guessing from folder names got each of those wrong somewhere. Cargo already knows the answer,
//! so ask it: which packages the workspace builds, and which file is each crate's root.
//!
//! Everything here is a refinement. When cargo is missing or the folder is not inside a
//! workspace, [`CargoProject::discover`] returns `None` and callers keep their folder heuristics.

use crate::console::NoConsoleWindow;
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::Mutex;
use std::time::SystemTime;

use serde::Deserialize;

/// One workspace, as cargo sees it.
#[derive(Debug, Clone, PartialEq)]
pub struct CargoProject {
    workspace_root: PathBuf,
    packages: Vec<CargoPackage>,
}

/// One workspace member.
#[derive(Debug, Clone, PartialEq)]
struct CargoPackage {
    manifest: PathBuf,
    /// Root source file of each library and binary target (`src/lib.rs`, a custom `[lib] path`).
    crate_roots: Vec<PathBuf>,
}

impl CargoProject {
    /// The workspace `dir` is in, or `None` when cargo cannot say.
    ///
    /// Cached per folder and revalidated against the manifests' modification times, because the
    /// source index is rebuilt on every move selection and a cargo process per click would show.
    pub fn discover(dir: &Path) -> Option<Self> {
        #[cfg(test)]
        if !tests::enabled() {
            return None;
        }
        static CACHE: Mutex<
            Option<HashMap<PathBuf, (Vec<(PathBuf, Option<SystemTime>)>, CargoProject)>>,
        > = Mutex::new(None);
        let key = dir.to_path_buf();
        if let Some((stamps, project)) = CACHE
            .lock()
            .ok()?
            .get_or_insert_with(HashMap::new)
            .get(&key)
        {
            if stamps.iter().all(|(path, stamp)| modified(path) == *stamp) {
                return Some(project.clone());
            }
        }
        let output = std::process::Command::new("cargo")
            .no_console_window()
            .args(["metadata", "--no-deps", "--format-version", "1"])
            .current_dir(dir)
            .output()
            .ok()?;
        if !output.status.success() {
            return None;
        }
        let project = Self::from_metadata(&output.stdout)?;
        let stamps = std::iter::once(project.workspace_root.join("Cargo.toml"))
            .chain(
                project
                    .packages
                    .iter()
                    .map(|package| package.manifest.clone()),
            )
            .map(|path| {
                let stamp = modified(&path);
                (path, stamp)
            })
            .collect();
        if let Ok(mut cache) = CACHE.lock() {
            cache
                .get_or_insert_with(HashMap::new)
                .insert(key, (stamps, project.clone()));
        }
        Some(project)
    }

    /// Parse `cargo metadata --format-version 1` output.
    pub fn from_metadata(json: &[u8]) -> Option<Self> {
        #[derive(Deserialize)]
        struct Metadata {
            workspace_root: PathBuf,
            #[serde(default)]
            packages: Vec<Package>,
        }
        #[derive(Deserialize)]
        struct Package {
            manifest_path: PathBuf,
            #[serde(default)]
            targets: Vec<Target>,
        }
        #[derive(Deserialize)]
        struct Target {
            #[serde(default)]
            kind: Vec<String>,
            src_path: PathBuf,
        }
        let meta: Metadata = serde_json::from_slice(json).ok()?;
        let packages = meta
            .packages
            .into_iter()
            .map(|package| {
                let crate_roots = package
                    .targets
                    .iter()
                    .filter(|target| {
                        target.kind.iter().any(|kind| {
                            matches!(
                                kind.as_str(),
                                "lib" | "cdylib" | "rlib" | "dylib" | "staticlib" | "bin"
                            )
                        })
                    })
                    .map(|target| target.src_path.clone())
                    .collect();
                CargoPackage {
                    manifest: package.manifest_path,
                    crate_roots,
                }
            })
            .collect();
        Some(Self {
            workspace_root: meta.workspace_root,
            packages,
        })
    }

    /// Every crate root the workspace compiles.
    pub fn crate_roots(&self) -> impl Iterator<Item = &PathBuf> {
        self.packages
            .iter()
            .flat_map(|package| package.crate_roots.iter())
    }
}

/// A comparison key for a path: separators unified and, on Windows, case folded, since cargo
/// and a folder picker can spell the same file differently.
pub fn path_key(path: &Path) -> String {
    let text = path.to_string_lossy();
    if cfg!(windows) {
        text.replace('/', "\\").to_lowercase()
    } else {
        text.into_owned()
    }
}

fn modified(path: &Path) -> Option<SystemTime> {
    std::fs::metadata(path).ok()?.modified().ok()
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;
    use std::cell::Cell;

    thread_local! {
        static ENABLED: Cell<bool> = const { Cell::new(false) };
    }

    /// Unit tests build hundreds of temporary indexes; a cargo process for each would dominate
    /// the run. Discovery is therefore off in tests unless a test turns it on for its thread.
    pub(crate) fn enabled() -> bool {
        ENABLED.with(Cell::get)
    }

    pub(crate) fn enable() {
        ENABLED.with(|enabled| enabled.set(true));
    }

    const WORKSPACE: &str = r#"{
        "workspace_root": "/ws",
        "target_directory": "/ws/target",
        "packages": [
            {"name": "my-mod", "manifest_path": "/ws/plugin/Cargo.toml", "targets": [
                {"name": "my_mod", "kind": ["cdylib"], "crate_types": ["cdylib"], "src_path": "/ws/plugin/src/main_lib.rs"}
            ]},
            {"name": "helper", "manifest_path": "/ws/crates/helper/Cargo.toml", "targets": [
                {"name": "helper", "kind": ["lib"], "crate_types": ["lib"], "src_path": "/ws/crates/helper/src/lib.rs"},
                {"name": "build-script-build", "kind": ["custom-build"], "crate_types": ["bin"], "src_path": "/ws/crates/helper/build.rs"}
            ]}
        ]
    }"#;

    #[test]
    fn metadata_gives_crate_roots() {
        let project = CargoProject::from_metadata(WORKSPACE.as_bytes()).unwrap();
        let roots: Vec<_> = project.crate_roots().cloned().collect();
        assert_eq!(
            roots,
            [
                PathBuf::from("/ws/plugin/src/main_lib.rs"),
                PathBuf::from("/ws/crates/helper/src/lib.rs")
            ]
        );
    }

    #[test]
    fn discovers_a_real_crate_in_a_subfolder() {
        enable();
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path();
        std::fs::write(
            root.join("Cargo.toml"),
            "[workspace]\nmembers = [\"code/mod\"]\nresolver = \"2\"\n",
        )
        .unwrap();
        std::fs::create_dir_all(root.join("code/mod/src")).unwrap();
        std::fs::write(
            root.join("code/mod/Cargo.toml"),
            "[package]\nname = \"deep-mod\"\nversion = \"0.1.0\"\nedition = \"2021\"\n[lib]\ncrate-type = [\"cdylib\"]\npath = \"src/entry.rs\"\n",
        )
        .unwrap();
        std::fs::write(root.join("code/mod/src/entry.rs"), "").unwrap();
        let project = CargoProject::discover(&root.join("code/mod/src")).expect("cargo metadata");
        let roots: Vec<_> = project.crate_roots().collect();
        assert!(
            matches!(roots[..], [root] if root.ends_with("code/mod/src/entry.rs")),
            "{roots:?}"
        );
    }
}
