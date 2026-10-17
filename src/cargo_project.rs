//! The Cargo layout of a linked moveset, as `cargo metadata` reports it.
//!
//! Moveset repositories are laid out every way there is: the crate at the root, under
//! `plugin/`, one crate per fighter in a workspace, a `[lib] path` pointing somewhere unusual, a
//! `CARGO_TARGET_DIR` or `.cargo/config.toml` moving the build output. Guessing from folder names
//! got each of those wrong somewhere. Cargo already knows the answer, so ask it: which packages
//! the workspace builds, which file is each crate's root, which package is the `cdylib` plugin,
//! and where its output lands.
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
    pub workspace_root: PathBuf,
    pub target_dir: PathBuf,
    pub packages: Vec<CargoPackage>,
}

/// One workspace member.
#[derive(Debug, Clone, PartialEq)]
pub struct CargoPackage {
    pub name: String,
    pub manifest: PathBuf,
    /// Root source file of each library and binary target (`src/lib.rs`, a custom `[lib] path`).
    pub crate_roots: Vec<PathBuf>,
    /// Crate name of the `cdylib` target, when the package builds one: the plugin is
    /// `lib<name>.nro`.
    pub cdylib: Option<String>,
}

impl CargoPackage {
    pub fn dir(&self) -> &Path {
        self.manifest.parent().unwrap_or(&self.manifest)
    }
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
        static CACHE: Mutex<Option<HashMap<PathBuf, (Vec<(PathBuf, Option<SystemTime>)>, CargoProject)>>> =
            Mutex::new(None);
        let key = dir.to_path_buf();
        if let Some((stamps, project)) = CACHE.lock().ok()?.get_or_insert_with(HashMap::new).get(&key) {
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
            .chain(project.packages.iter().map(|package| package.manifest.clone()))
            .map(|path| {
                let stamp = modified(&path);
                (path, stamp)
            })
            .collect();
        if let Ok(mut cache) = CACHE.lock() {
            cache.get_or_insert_with(HashMap::new).insert(key, (stamps, project.clone()));
        }
        Some(project)
    }

    /// Parse `cargo metadata --format-version 1` output.
    pub fn from_metadata(json: &[u8]) -> Option<Self> {
        #[derive(Deserialize)]
        struct Metadata {
            workspace_root: PathBuf,
            target_directory: PathBuf,
            #[serde(default)]
            packages: Vec<Package>,
        }
        #[derive(Deserialize)]
        struct Package {
            name: String,
            manifest_path: PathBuf,
            #[serde(default)]
            targets: Vec<Target>,
        }
        #[derive(Deserialize)]
        struct Target {
            name: String,
            #[serde(default)]
            kind: Vec<String>,
            #[serde(default)]
            crate_types: Vec<String>,
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
                        target
                            .kind
                            .iter()
                            .any(|kind| matches!(kind.as_str(), "lib" | "cdylib" | "rlib" | "dylib" | "staticlib" | "bin"))
                    })
                    .map(|target| target.src_path.clone())
                    .collect();
                let cdylib = package
                    .targets
                    .iter()
                    .find(|target| target.crate_types.iter().any(|kind| kind == "cdylib"))
                    .map(|target| target.name.replace('-', "_"));
                CargoPackage {
                    name: package.name,
                    manifest: package.manifest_path,
                    crate_roots,
                    cdylib,
                }
            })
            .collect();
        Some(Self {
            workspace_root: meta.workspace_root,
            target_dir: meta.target_directory,
            packages,
        })
    }

    /// Every crate root the workspace compiles.
    pub fn crate_roots(&self) -> impl Iterator<Item = &PathBuf> {
        self.packages.iter().flat_map(|package| package.crate_roots.iter())
    }

    /// The plugin package: the workspace's only `cdylib`, or, when there are several, the one
    /// most of `files` belong to. `None` when no package builds a `cdylib`, or the tie between
    /// several cannot be broken.
    pub fn plugin_for(&self, files: &[PathBuf]) -> Option<&CargoPackage> {
        let plugins: Vec<&CargoPackage> = self.packages.iter().filter(|p| p.cdylib.is_some()).collect();
        if plugins.len() <= 1 {
            return plugins.first().copied();
        }
        let mut counts: Vec<(usize, &CargoPackage)> = plugins
            .iter()
            .map(|package| (files.iter().filter(|file| file.starts_with(package.dir())).count(), *package))
            .collect();
        counts.sort_by(|a, b| b.0.cmp(&a.0));
        (counts[0].0 > 0 && counts[0].0 > counts[1].0).then_some(counts[0].1)
    }

    /// Where `cargo skyline build` with `profile` writes `package`'s plugin.
    pub fn built_plugin(&self, package: &CargoPackage, profile: &str) -> Option<PathBuf> {
        let folder = if profile == "dev" { "debug" } else { profile };
        Some(
            self.target_dir
                .join("aarch64-skyline-switch")
                .join(folder)
                .join(format!("lib{}.nro", package.cdylib.as_ref()?)),
        )
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
    fn metadata_gives_crate_roots_and_the_plugin() {
        let project = CargoProject::from_metadata(WORKSPACE.as_bytes()).unwrap();
        let roots: Vec<_> = project.crate_roots().cloned().collect();
        assert_eq!(roots, [PathBuf::from("/ws/plugin/src/main_lib.rs"), PathBuf::from("/ws/crates/helper/src/lib.rs")]);
        let plugin = project.plugin_for(&[]).unwrap();
        assert_eq!(plugin.name, "my-mod");
        assert_eq!(plugin.dir(), Path::new("/ws/plugin"));
        assert_eq!(
            project.built_plugin(plugin, "release").unwrap(),
            Path::new("/ws/target/aarch64-skyline-switch/release/libmy_mod.nro")
        );
    }

    #[test]
    fn several_plugins_are_told_apart_by_the_linked_files() {
        let mut project = CargoProject::from_metadata(WORKSPACE.as_bytes()).unwrap();
        let mut second = project.packages[0].clone();
        second.name = "other".into();
        second.manifest = PathBuf::from("/ws/other/Cargo.toml");
        project.packages.push(second);
        assert!(project.plugin_for(&[]).is_none(), "no evidence, no guess");
        let files = [PathBuf::from("/ws/other/src/lib.rs"), PathBuf::from("/ws/other/src/acmd.rs")];
        assert_eq!(project.plugin_for(&files).unwrap().name, "other");
    }

    #[test]
    fn discovers_a_real_crate_in_a_subfolder() {
        enable();
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path();
        std::fs::write(root.join("Cargo.toml"), "[workspace]\nmembers = [\"code/mod\"]\nresolver = \"2\"\n").unwrap();
        std::fs::create_dir_all(root.join("code/mod/src")).unwrap();
        std::fs::write(
            root.join("code/mod/Cargo.toml"),
            "[package]\nname = \"deep-mod\"\nversion = \"0.1.0\"\nedition = \"2021\"\n[lib]\ncrate-type = [\"cdylib\"]\npath = \"src/entry.rs\"\n",
        )
        .unwrap();
        std::fs::write(root.join("code/mod/src/entry.rs"), "").unwrap();
        let Some(project) = CargoProject::discover(&root.join("code/mod/src")) else {
            eprintln!("cargo unavailable; skipping");
            return;
        };
        let plugin = project.plugin_for(&[]).unwrap();
        assert_eq!(plugin.cdylib.as_deref(), Some("deep_mod"));
        assert!(plugin.crate_roots[0].ends_with("entry.rs"), "{:?}", plugin.crate_roots);
    }
}
