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
use crate::user_files::real_path;
use std::collections::HashMap;
use std::io::Read;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::Mutex;
use std::time::{Duration, Instant, SystemTime};

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

/// How long cargo may take to answer. It only reads manifests, so one that takes longer is stuck.
const TIMEOUT: Duration = Duration::from_secs(10);

/// What an answer depends on, as it was when cargo was asked: files that must still have these
/// modification times, or still be absent.
type Stamps = Vec<(PathBuf, Option<SystemTime>)>;

enum Answer {
    /// A worker is asking cargo; what it said last time, if it did.
    Asked(Option<CargoProject>),
    /// `None` when cargo could not say. Not asked again until the folder changes, or is to be
    /// looked at again (no stamps).
    Known(Option<Stamps>, Option<CargoProject>),
}

static ANSWERS: Mutex<Option<HashMap<PathBuf, Answer>>> = Mutex::new(None);
/// Workers asking cargo now.
static ASKING: AtomicUsize = AtomicUsize::new(0);
/// An answer came in since [`take_finished`] last looked.
static FINISHED: AtomicBool = AtomicBool::new(false);

fn with_answers<T>(use_answers: impl FnOnce(&mut HashMap<PathBuf, Answer>) -> T) -> Option<T> {
    let mut answers = ANSWERS.lock().ok()?;
    Some(use_answers(answers.get_or_insert_with(HashMap::new)))
}

/// Whether a worker is still asking cargo about the folder `dir`.
pub fn is_pending(dir: &Path) -> bool {
    is_asking()
        && with_answers(|answers| matches!(answers.get(&real_path(dir)), Some(Answer::Asked(_))))
            .unwrap_or(false)
}

/// Whether any worker is asking cargo. Cheap enough to look at every frame.
pub fn is_asking() -> bool {
    ASKING.load(Ordering::Relaxed) > 0
}

/// Whether an answer came in since this was last called, so an index built without it is stale.
pub fn take_finished() -> bool {
    FINISHED.swap(false, Ordering::Relaxed)
}

/// Have cargo asked about `dir` again, whatever it said before.
pub fn ask_again(dir: &Path) {
    with_answers(|answers| {
        if let Some(Answer::Known(stamps, _)) = answers.get_mut(&real_path(dir)) {
            *stamps = None;
        }
    });
}

impl CargoProject {
    /// The workspace `dir` is in, or `None` when cargo cannot say or has not yet. Never waits: a
    /// worker asks, [`is_pending`] says whether it still is, and [`take_finished`] when to look
    /// again. The answer is kept until a manifest changes, a failure until the folder does.
    pub fn discover(dir: &Path) -> Option<Self> {
        #[cfg(test)]
        if !tests::enabled() {
            return None;
        }
        let (project, worker) = Self::look_up(dir);
        // A test reads the answer straight away.
        if let Some(worker) = worker.filter(|_| cfg!(test)) {
            worker.join().ok()?;
            return Self::discover(dir);
        }
        project
    }

    /// What is known about `dir` now, and the worker started to ask when that is nothing or old.
    fn look_up(dir: &Path) -> (Option<Self>, Option<std::thread::JoinHandle<()>>) {
        let dir = real_path(dir);
        let known = with_answers(|answers| {
            let (project, valid) = match answers.get(&dir) {
                Some(Answer::Asked(last)) => (last.clone(), true),
                Some(Answer::Known(stamps, project)) => {
                    let same = |(path, at): &(PathBuf, _)| modified(path) == *at;
                    let valid = stamps.as_ref().is_some_and(|all| all.iter().all(same));
                    (project.clone(), valid)
                }
                None => (None, false),
            };
            if !valid {
                answers.insert(dir.clone(), Answer::Asked(project.clone()));
                ASKING.fetch_add(1, Ordering::Relaxed);
            }
            (project, valid)
        });
        let Some((project, false)) = known else {
            return (known.and_then(|(project, _)| project), None);
        };
        let worker = std::thread::spawn(move || {
            let project = Self::ask(&dir);
            let stamps = Some(stamps(&dir, project.as_ref()));
            with_answers(|answers| answers.insert(dir, Answer::Known(stamps, project)));
            FINISHED.store(true, Ordering::Relaxed);
            ASKING.fetch_sub(1, Ordering::Relaxed);
        });
        (project, Some(worker))
    }

    /// Ask cargo, for at most [`TIMEOUT`] a try.
    ///
    /// First with the manifest named from outside its folder, and never fetching: a linked
    /// moveset's `rust-toolchain.toml` would have rustup install that toolchain first, and its
    /// `.cargo/config.toml` can name programs to run. Cargo run that way is the machine's default
    /// one, which may be none at all, or too old for a manifest its own toolchain reads. So when
    /// that fails cargo is asked inside the folder, as a build of the moveset runs it.
    fn ask(dir: &Path) -> Option<Self> {
        #[cfg(test)]
        tests::ASKED.lock().unwrap().push(dir.to_path_buf());
        Self::ask_from(dir, false).or_else(|| Self::ask_from(dir, true))
    }

    /// Ask cargo inside the moveset's folder and wait for it: what a build there will do, with
    /// its own toolchain and its `.cargo/config.toml`, which can move the build's output. For
    /// a build the user asked for; `None` when cargo cannot say.
    pub fn ask_as_a_build_does(dir: &Path) -> Option<Self> {
        Self::ask_from(dir, true)
    }

    fn ask_from(dir: &Path, inside: bool) -> Option<Self> {
        let manifest = dir
            .ancestors()
            .map(|dir| dir.join("Cargo.toml"))
            .find(|manifest| manifest.is_file())?;
        let folder = if inside {
            manifest.parent()?
        } else {
            dir.ancestors().last()?
        };
        let mut cargo = std::process::Command::new("cargo");
        cargo
            .args([
                "metadata",
                "--no-deps",
                "--offline",
                "--format-version",
                "1",
            ])
            .arg("--manifest-path")
            .arg(&manifest)
            .current_dir(folder)
            .env("RUSTUP_AUTO_INSTALL", "0");
        Self::from_metadata(&output_within(cargo, TIMEOUT)?)
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
                        target.kind.iter().any(|kind| {
                            matches!(
                                kind.as_str(),
                                "lib" | "cdylib" | "rlib" | "dylib" | "staticlib" | "bin"
                            )
                        })
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
        self.packages
            .iter()
            .flat_map(|package| package.crate_roots.iter())
    }

    /// The plugin package: the workspace's only `cdylib`, or, when there are several, the one
    /// most of `files` belong to. `None` when no package builds a `cdylib`, or the tie between
    /// several cannot be broken.
    pub fn plugin_for(&self, files: &[PathBuf]) -> Option<&CargoPackage> {
        let plugins: Vec<&CargoPackage> = self
            .packages
            .iter()
            .filter(|p| p.cdylib.is_some())
            .collect();
        if plugins.len() <= 1 {
            return plugins.first().copied();
        }
        let mut counts: Vec<(usize, &CargoPackage)> = plugins
            .iter()
            .map(|package| {
                (
                    files
                        .iter()
                        .filter(|file| file.starts_with(package.dir()))
                        .count(),
                    *package,
                )
            })
            .collect();
        counts.sort_by_key(|count| std::cmp::Reverse(count.0));
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

/// What `command` prints, or `None` when it fails or takes longer than `limit`.
fn output_within(mut command: std::process::Command, limit: Duration) -> Option<Vec<u8>> {
    let mut child = command
        .no_console_window()
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::null())
        .spawn()
        .ok()?;
    let mut printed = child.stdout.take()?;
    let (send, receive) = std::sync::mpsc::channel();
    std::thread::spawn(move || {
        let mut out = Vec::new();
        if printed.read_to_end(&mut out).is_ok() {
            let _ = send.send(out);
        }
    });
    let deadline = Instant::now() + limit;
    let out = receive.recv_timeout(limit).ok();
    loop {
        match child.try_wait() {
            Ok(Some(status)) => return out.filter(|_| status.success()),
            Ok(None) if out.is_some() && Instant::now() < deadline => {
                std::thread::sleep(Duration::from_millis(5));
            }
            _ => {
                let _ = child.kill();
                let _ = child.wait();
                return None;
            }
        }
    }
}

/// What `project`, cargo's answer about `dir`, depends on: its manifests, or for no answer the
/// folder itself and every manifest it could be under.
fn stamps(dir: &Path, project: Option<&CargoProject>) -> Stamps {
    let paths: Vec<PathBuf> = match project {
        Some(project) => std::iter::once(project.workspace_root.join("Cargo.toml"))
            .chain(
                project
                    .packages
                    .iter()
                    .map(|package| package.manifest.clone()),
            )
            .collect(),
        None => std::iter::once(dir.to_path_buf())
            .chain(dir.ancestors().map(|dir| dir.join("Cargo.toml")))
            .collect(),
    };
    paths
        .into_iter()
        .map(|path| {
            let at = modified(&path);
            (path, at)
        })
        .collect()
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

    /// The folders cargo was asked about.
    pub(super) static ASKED: Mutex<Vec<PathBuf>> = Mutex::new(Vec::new());

    fn asked(dir: &Path) -> usize {
        let dir = real_path(dir);
        ASKED
            .lock()
            .unwrap()
            .iter()
            .filter(|at| **at == dir)
            .count()
    }

    const PACKAGE: &str = "[package]\nname = \"mine\"\nversion = \"0.1.0\"\nedition = \"2021\"\n";

    fn crate_in(root: &Path, manifest: &str) {
        std::fs::create_dir_all(root.join("src")).unwrap();
        std::fs::write(root.join("Cargo.toml"), manifest).unwrap();
        std::fs::write(root.join("src/lib.rs"), "").unwrap();
    }

    /// A worker asks; the caller has no answer yet and does not wait for one.
    #[test]
    fn asking_does_not_hold_up_the_caller() {
        let tmp = tempfile::tempdir().unwrap();
        crate_in(tmp.path(), PACKAGE);
        let (project, worker) = CargoProject::look_up(tmp.path());
        assert_eq!(project, None);
        // Asked once, however often the index is rebuilt meanwhile.
        assert!(CargoProject::look_up(tmp.path()).1.is_none());
        worker.expect("a worker asks").join().unwrap();
        assert!(!is_pending(tmp.path()));
        assert!(take_finished());
        let (project, worker) = CargoProject::look_up(tmp.path());
        assert!(project.is_some() && worker.is_none());
        assert_eq!(asked(tmp.path()), 1);
    }

    /// A folder cargo cannot read is not asked about on every index, only once it changes or the
    /// user looks at it again; the last answer stands meanwhile.
    #[test]
    fn a_failure_is_kept_until_the_folder_changes() {
        enable();
        let tmp = tempfile::tempdir().unwrap();
        crate_in(tmp.path(), "not a manifest");
        for _ in 0..3 {
            assert_eq!(CargoProject::discover(tmp.path()), None);
        }
        assert_eq!(asked(tmp.path()), 1);
        ask_again(tmp.path());
        assert_eq!(CargoProject::discover(tmp.path()), None);
        assert_eq!(asked(tmp.path()), 2);
        // A later time, for file systems that keep whole seconds.
        std::fs::write(tmp.path().join("Cargo.toml"), PACKAGE).unwrap();
        let later = SystemTime::now() + Duration::from_secs(5);
        std::fs::File::options()
            .write(true)
            .open(tmp.path().join("Cargo.toml"))
            .and_then(|file| file.set_modified(later))
            .unwrap();
        assert!(CargoProject::discover(tmp.path()).is_some());
        assert_eq!(asked(tmp.path()), 3);
    }

    /// A linked folder's toolchain file would have rustup install that toolchain, and its cargo
    /// config names the compiler to run: neither is used to read its manifest.
    #[test]
    fn a_linked_folders_toolchain_and_config_are_not_used() {
        enable();
        let tmp = tempfile::tempdir().unwrap();
        crate_in(tmp.path(), PACKAGE);
        std::fs::write(
            tmp.path().join("rust-toolchain.toml"),
            "[toolchain]\nchannel = \"nightly-1999-01-01\"\n",
        )
        .unwrap();
        std::fs::create_dir(tmp.path().join(".cargo")).unwrap();
        std::fs::write(
            tmp.path().join(".cargo/config.toml"),
            "[build]\nrustc = \"/nonexistent/rustc\"\n[net]\noffline = \"not a bool\"\n",
        )
        .unwrap();
        assert!(CargoProject::discover(tmp.path()).is_some());
    }

    #[cfg(unix)]
    #[test]
    fn a_command_that_hangs_is_given_up_on() {
        let started = Instant::now();
        let mut command = std::process::Command::new("sh");
        command.args(["-c", "echo {}; exec sleep 30"]);
        assert_eq!(output_within(command, Duration::from_millis(200)), None);
        assert!(started.elapsed() < Duration::from_secs(10));
        let mut command = std::process::Command::new("sh");
        command.args(["-c", "echo said"]);
        let said = output_within(command, Duration::from_secs(10));
        assert_eq!(said.as_deref(), Some(&b"said\n"[..]));
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
        assert_eq!(
            roots,
            [
                PathBuf::from("/ws/plugin/src/main_lib.rs"),
                PathBuf::from("/ws/crates/helper/src/lib.rs")
            ]
        );
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
        let files = [
            PathBuf::from("/ws/other/src/lib.rs"),
            PathBuf::from("/ws/other/src/acmd.rs"),
        ];
        assert_eq!(project.plugin_for(&files).unwrap().name, "other");
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
        let Some(project) = CargoProject::discover(&root.join("code/mod/src")) else {
            eprintln!("cargo unavailable; skipping");
            return;
        };
        let plugin = project.plugin_for(&[]).unwrap();
        assert_eq!(plugin.cdylib.as_deref(), Some("deep_mod"));
        assert!(
            plugin.crate_roots[0].ends_with("entry.rs"),
            "{:?}",
            plugin.crate_roots
        );
    }
}
