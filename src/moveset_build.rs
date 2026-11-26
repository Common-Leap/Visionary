//! Build the linked moveset's plugin and put it where the game loads it.
//!
//! The loop a moveset author runs dozens of times a session is: edit, `cargo skyline build
//! --release`, find the `.nro` under `target/aarch64-skyline-switch/release/`, copy it over the
//! one the emulator loads, restart. This does the middle three in one step:
//!
//! - the crate is the linked source's `cdylib` package as `cargo metadata` reports it (the one
//!   most indexed files belong to, when a workspace has several), and its plugin is
//!   `lib<name>.nro` in cargo's own target directory. Without cargo, the `Cargo.toml` most
//!   indexed files sit under stands in, per `[lib] name` or `[package] name`;
//! - the copies of that file already deployed are looked for: in an emulator SD's
//!   `ultimate/mods`, its Skyline plugins folder, or a LayeredFS `load/` mod. ARCropolis mods
//!   usually ship the plugin renamed to `plugin.nro`, which a name cannot find; a compiled plugin
//!   carries its crate's source paths in its panic locations (`src\shyguy\article_kind.rs`), so
//!   the `plugin.nro` that carries the most of THIS crate's own paths, by a clear margin, is
//!   offered too;
//! - when the plugin exports `smashline_install` (smashline's development entry point),
//!   smashline's `development.nro` is offered as well: smashline reloads it in place when
//!   L + R + D-pad Up is pressed in the menus, so a change needs no restart at all.
//!
//! What is found is only ever offered. A build writes to the places the user has said yes to for
//! this moveset and nowhere else: two movesets made from one template look alike, and the wrong
//! guess would replace another mod's plugin. The file a build replaces is kept, one copy per
//! place, in Visionary's own folder, never beside the plugin, where Skyline would load it as a
//! second plugin. The build's output is written there too, not into the user's repository.

use crate::console::NoConsoleWindow;
use std::path::{Path, PathBuf};

use anyhow::{bail, Context, Result};

/// The game's title id; every Switch-side path below is under it.
const TITLE_ID: &str = "01006A800016E000";

/// What one build will do.
#[derive(Debug, Clone, PartialEq)]
pub struct Plan {
    pub crate_dir: PathBuf,
    /// `libshyguy.nro`.
    pub nro_name: String,
    /// Where the built file is expected, in order of preference.
    pub built: Vec<PathBuf>,
    /// The places a build of this plugin could go, as found. None is written unasked.
    pub found: Vec<Place>,
}

/// Somewhere the built plugin could be copied.
#[derive(Debug, Clone, PartialEq)]
pub struct Place {
    pub path: PathBuf,
    pub why: Why,
}

/// Why a place is offered.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum Why {
    /// A file there has the plugin's own name.
    SameName,
    /// A `plugin.nro` there carries this crate's source paths.
    BuiltFromThisSource,
    /// smashline's development plugin, which it reloads in the menus.
    DevReload,
}

impl Why {
    pub fn words(self) -> &'static str {
        match self {
            Self::SameName => "A file with this plugin's name is here.",
            Self::BuiltFromThisSource => {
                "This plugin.nro looks built from this source: it names this crate's own files. \
                 Another moveset made from the same template can look alike, so check the folder."
            }
            Self::DevReload => {
                "smashline's development plugin. Press L + R + D-pad Up in the game's menus and \
                 smashline loads it again without a restart."
            }
        }
    }
}

/// Where Visionary keeps what a build leaves behind: its output, and the files it replaced.
pub fn keep_dir() -> PathBuf {
    crate::scratch_dirs::app_storage_root().join("moveset-build")
}

/// How a build ended.
#[derive(Debug, Clone, PartialEq)]
pub struct Outcome {
    pub ok: bool,
    pub message: String,
    /// Where the full cargo output was written.
    pub log: Option<PathBuf>,
}

/// Work out what to build and where it goes.
///
/// `files` are the linked source's indexed files; `sources` their text (for the development
/// entry point check); `sd_root` the emulator SD if one was found.
pub fn plan(files: &[PathBuf], sources: &[String], sd_root: Option<&Path>) -> Result<Plan> {
    // Asked inside the moveset's folder, as the build itself runs: its `.cargo/config.toml` can
    // move the output, and its own toolchain reads its manifest.
    let folder = files.iter().find_map(|file| file.parent());
    let cargo = folder
        .and_then(crate::cargo_project::CargoProject::ask_as_a_build_does)
        .or_else(|| folder.and_then(crate::cargo_project::CargoProject::discover));
    let resolved = cargo.as_ref().and_then(|project| {
        let package = project.plugin_for(files)?;
        let built = project.built_plugin(package, "release")?;
        let name = built.file_name()?.to_string_lossy().into_owned();
        Some((package.dir().to_path_buf(), name, vec![built]))
    });
    let (crate_dir, nro_name, built) = match resolved {
        Some(resolved) => resolved,
        None => {
            let crate_dir =
                owning_crate(files).context("the linked source is not inside a Cargo crate")?;
            let toml = std::fs::read_to_string(crate_dir.join("Cargo.toml"))
                .with_context(|| format!("reading {}", crate_dir.join("Cargo.toml").display()))?;
            let Some(lib) = library_name(&toml) else {
                bail!(
                    "{} names no package",
                    crate_dir.join("Cargo.toml").display()
                );
            };
            let nro_name = format!("lib{}.nro", lib.replace('-', "_"));
            let built = crate_dir
                .ancestors()
                .take(4)
                .map(|dir| {
                    dir.join("target")
                        .join("aarch64-skyline-switch")
                        .join("release")
                        .join(&nro_name)
                })
                .collect();
            (crate_dir, nro_name, built)
        }
    };
    let mut found: Vec<Place> = sd_root
        .map(|sd| deployed_copies(sd, &nro_name))
        .unwrap_or_default()
        .into_iter()
        .map(|path| Place {
            path,
            why: Why::SameName,
        })
        .collect();
    if let Some(sd) = sd_root {
        if let Some(path) = fingerprinted_plugin(sd, &crate_dir, files) {
            found.push(Place {
                path,
                why: Why::BuiltFromThisSource,
            });
        }
    }
    let exports_dev_entry = sources
        .iter()
        .any(|text| text.contains("fn smashline_install") && text.contains("no_mangle"));
    if let Some(sd) = sd_root.filter(|_| exports_dev_entry) {
        found.push(Place {
            path: sd
                .join("atmosphere")
                .join("contents")
                .join(TITLE_ID)
                .join("romfs")
                .join("smashline")
                .join("development.nro"),
            why: Why::DevReload,
        });
    }
    Ok(Plan {
        crate_dir,
        nro_name,
        built,
        found,
    })
}

/// The places among `found` the user has not been asked about yet.
pub fn unasked<'a>(
    found: &'a [Place],
    approved: &[PathBuf],
    declined: &[PathBuf],
) -> Vec<&'a Place> {
    found
        .iter()
        .filter(|place| !approved.contains(&place.path) && !declined.contains(&place.path))
        .collect()
}

/// Run the build and copy the plugin to `targets`, the places the user said yes to. Blocking:
/// call it on a worker thread. `keep` is where the output and the replaced files go.
pub fn run(plan: &Plan, targets: &[PathBuf], keep: &Path) -> Outcome {
    let output = std::process::Command::new("cargo")
        .no_console_window()
        .args(["skyline", "build", "--release"])
        .current_dir(&plan.crate_dir)
        .output();
    let output = match output {
        Ok(output) => output,
        Err(error) => {
            return Outcome {
                ok: false,
                message: format!(
                    "could not start `cargo skyline build` ({error}): is cargo-skyline installed?"
                ),
                log: None,
            }
        }
    };
    let stderr = String::from_utf8_lossy(&output.stderr).to_string();
    let mut log_text = format!(
        "--- stdout ---\n{}\n--- stderr ---\n{stderr}",
        String::from_utf8_lossy(&output.stdout)
    );
    let log_path = keep.join(format!("{}.log", plan.nro_name.trim_end_matches(".nro")));
    let write_log = |text: &str| {
        std::fs::create_dir_all(keep)
            .and_then(|_| std::fs::write(&log_path, text))
            .ok()
            .map(|_| log_path.clone())
    };
    if !output.status.success() {
        return Outcome {
            ok: false,
            message: format!("build failed: {}", first_error(&stderr)),
            log: write_log(&log_text),
        };
    }
    // The newest candidate is the one this build wrote.
    let Some(built) = plan
        .built
        .iter()
        .filter_map(|path| Some((std::fs::metadata(path).ok()?.modified().ok()?, path)))
        .max_by_key(|(modified, _)| *modified)
        .map(|(_, path)| path.clone())
    else {
        return Outcome {
            ok: false,
            message: format!(
                "the build succeeded but {} was not found under target/",
                plan.nro_name
            ),
            log: write_log(&log_text),
        };
    };
    let (written, failed) = deploy(&built, targets, keep, &mut log_text);
    let message = if written.is_empty() && failed.is_empty() {
        format!(
            "built {}. It was copied nowhere: choose where it goes with Deploy to",
            built.display()
        )
    } else {
        let mut message = format!(
            "built and deployed {} to {} place(s)",
            plan.nro_name,
            written.len()
        );
        if written.iter().any(|path| is_dev_reload(path)) {
            message.push_str(": press L + R + D-pad Up in the game's menus to hot-reload it");
        } else {
            message.push_str(": restart the game to load it");
        }
        if !failed.is_empty() {
            message.push_str(&format!("; could not write {}", failed.join(", ")));
        }
        message
    };
    Outcome {
        ok: failed.is_empty(),
        message,
        log: write_log(&log_text),
    }
}

fn is_dev_reload(path: &Path) -> bool {
    path.ends_with(Path::new("smashline").join("development.nro"))
}

/// Copy `built` over each of `targets`, whole or not at all, keeping the file each one held in
/// `keep`. A place whose old file cannot be kept is left as it is.
fn deploy(
    built: &Path,
    targets: &[PathBuf],
    keep: &Path,
    log: &mut String,
) -> (Vec<PathBuf>, Vec<String>) {
    let mut written = Vec::new();
    let mut failed = Vec::new();
    let bytes = match std::fs::read(built) {
        Ok(bytes) => bytes,
        Err(error) => return (written, vec![format!("{} ({error})", built.display())]),
    };
    for target in targets {
        let result = (|| -> Result<Option<PathBuf>> {
            let kept = match std::fs::read(target) {
                Ok(old) if old != bytes => {
                    let kept = kept_copy(keep, target);
                    crate::user_files::write_outside_dump(&kept, &old)?;
                    Some(kept)
                }
                _ => None,
            };
            crate::user_files::write_outside_dump(target, &bytes)?;
            Ok(kept)
        })();
        match result {
            Ok(kept) => {
                log.push_str(&format!("\n--- deployed to {} ---\n", target.display()));
                if let Some(kept) = kept {
                    log.push_str(&format!("the file it replaced is at {}\n", kept.display()));
                }
                written.push(target.clone());
            }
            Err(error) => failed.push(format!("{} ({error:#})", target.display())),
        }
    }
    (written, failed)
}

/// Where the file a build replaced at `target` is kept: one copy per place.
fn kept_copy(keep: &Path, target: &Path) -> PathBuf {
    let place = target.to_string_lossy().to_lowercase();
    let name = target
        .file_name()
        .map(|name| name.to_string_lossy().into_owned())
        .unwrap_or_else(|| "plugin.nro".into());
    keep.join("replaced")
        .join(format!("{:010x}", hash40::hash40(&place).0))
        .join(name)
}

/// The crate directory most of `files` sit under.
fn owning_crate(files: &[PathBuf]) -> Option<PathBuf> {
    let mut counts: Vec<(PathBuf, usize)> = Vec::new();
    for file in files {
        let Some(dir) = file
            .ancestors()
            .skip(1)
            .find(|dir| dir.join("Cargo.toml").is_file() && is_package(&dir.join("Cargo.toml")))
        else {
            continue;
        };
        match counts.iter_mut().find(|(known, _)| known == dir) {
            Some((_, count)) => *count += 1,
            None => counts.push((dir.to_path_buf(), 1)),
        }
    }
    counts
        .into_iter()
        .max_by_key(|(_, count)| *count)
        .map(|(dir, _)| dir)
}

fn is_package(toml: &Path) -> bool {
    std::fs::read_to_string(toml).is_ok_and(|text| text.lines().any(|l| l.trim() == "[package]"))
}

/// `[lib] name`, else `[package] name`.
fn library_name(toml: &str) -> Option<String> {
    let mut section = "";
    let mut package = None;
    let mut lib = None;
    for line in toml.lines() {
        let line = line.split('#').next().unwrap_or("").trim();
        if line.starts_with('[') {
            section = line;
            continue;
        }
        let Some((key, value)) = line.split_once('=') else {
            continue;
        };
        if key.trim() != "name" {
            continue;
        }
        let value = value.trim().trim_matches('"').to_string();
        match section {
            "[package]" => package = Some(value),
            "[lib]" => lib = Some(value),
            _ => {}
        }
    }
    lib.or(package)
}

/// The `ultimate/mods/*/plugin.nro` built from this crate, recognised by the crate's own source
/// paths embedded in it. `None` unless one candidate clearly beats the rest. Only ever offered:
/// the user says whether it is theirs.
fn fingerprinted_plugin(sd: &Path, crate_dir: &Path, files: &[PathBuf]) -> Option<PathBuf> {
    // Each file counts once, in either spelling of its path. The files every template has
    // (`src/lib.rs`, a `mod.rs`) say nothing about whose plugin it is.
    let needles: Vec<[Vec<u8>; 2]> = files
        .iter()
        .filter_map(|file| file.strip_prefix(crate_dir).ok())
        .filter(|relative| {
            relative.components().count() >= 3
                && !relative
                    .file_name()
                    .is_some_and(|name| name == "lib.rs" || name == "mod.rs")
        })
        .map(|relative| {
            let forward = relative.to_string_lossy().replace('\\', "/");
            [
                forward.replace('/', "\\").into_bytes(),
                forward.into_bytes(),
            ]
        })
        .collect();
    if needles.is_empty() {
        return None;
    }
    let mut scored: Vec<(usize, PathBuf)> = Vec::new();
    let Ok(mods) = std::fs::read_dir(sd.join("ultimate").join("mods")) else {
        return None;
    };
    for entry in mods.flatten() {
        let plugin = entry.path().join("plugin.nro");
        let Ok(bytes) = std::fs::read(&plugin) else {
            continue;
        };
        let score = needles
            .iter()
            .filter(|spellings| spellings.iter().any(|needle| contains(&bytes, needle)))
            .count();
        if score > 0 {
            scored.push((score, plugin));
        }
    }
    scored.sort_by(|a, b| b.0.cmp(&a.0));
    match scored.as_slice() {
        [(best, path)] if *best >= 3 => Some(path.clone()),
        [(best, path), (second, _), ..] if *best >= 3 && *best >= second * 2 => Some(path.clone()),
        _ => None,
    }
}

fn contains(haystack: &[u8], needle: &[u8]) -> bool {
    !needle.is_empty()
        && haystack
            .windows(needle.len())
            .any(|window| window == needle)
}

/// Every copy of `nro_name` the game would load from this SD (and the emulator's LayeredFS
/// `load/` folder beside it).
fn deployed_copies(sd: &Path, nro_name: &str) -> Vec<PathBuf> {
    let mut found = Vec::new();
    find_named(&sd.join("ultimate").join("mods"), nro_name, 3, &mut found);
    let skyline = sd
        .join("atmosphere")
        .join("contents")
        .join(TITLE_ID)
        .join("romfs")
        .join("skyline")
        .join("plugins")
        .join(nro_name);
    if skyline.is_file() {
        found.push(skyline);
    }
    if let Some(emulator) = sd.parent() {
        find_named(
            &emulator.join("load").join(TITLE_ID),
            nro_name,
            5,
            &mut found,
        );
    }
    found.sort();
    found.dedup();
    found
}

fn find_named(dir: &Path, name: &str, depth: usize, out: &mut Vec<PathBuf>) {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return;
    };
    for entry in entries.flatten() {
        let path = entry.path();
        if path.is_dir() {
            if depth > 0 {
                find_named(&path, name, depth - 1, out);
            }
        } else if path
            .file_name()
            .is_some_and(|n| n.eq_ignore_ascii_case(name))
        {
            out.push(path);
        }
    }
}

/// The first compiler error, for the status line; the log has the rest.
fn first_error(stderr: &str) -> String {
    let mut lines = stderr.lines().skip_while(|line| !line.starts_with("error"));
    let Some(head) = lines.next() else {
        return stderr
            .lines()
            .rev()
            .find(|l| !l.trim().is_empty())
            .unwrap_or("")
            .trim()
            .to_string();
    };
    match lines.find(|line| line.trim_start().starts_with("-->")) {
        Some(at) => format!(
            "{head} ({})",
            at.trim_start().trim_start_matches("-->").trim()
        ),
        None => head.to_string(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn put(root: &Path, rel: &str, text: &str) {
        let path = root.join(rel);
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(path, text).unwrap();
    }

    #[test]
    fn the_plan_finds_the_crate_its_plugin_and_every_deployed_copy() {
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path();
        put(
            root,
            "plugin/Cargo.toml",
            "[package]\nname = \"shy-guy\"\n\n[lib]\ncrate-type = [\"cdylib\"]\n",
        );
        put(
            root,
            "plugin/src/lib.rs",
            "#[no_mangle]\npub extern \"C\" fn smashline_install() {}\n",
        );
        put(
            root,
            "crates/hash_lookup/Cargo.toml",
            "[package]\nname = \"hash_lookup\"\n",
        );
        put(root, "crates/hash_lookup/src/lib.rs", "");
        let sd = root.join("emu/sdmc");
        put(&sd, "ultimate/mods/Shy Guy/libshy_guy.nro", "old");
        put(&sd, "ultimate/mods/Other/libother.nro", "old");
        put(
            root,
            "emu/load/01006A800016E000/ShyGuy/romfs/skyline/plugins/libshy_guy.nro",
            "old",
        );

        let files = vec![root.join("plugin/src/lib.rs")];
        let sources = vec![std::fs::read_to_string(&files[0]).unwrap()];
        let plan = plan(&files, &sources, Some(&sd)).unwrap();
        assert_eq!(plan.crate_dir, root.join("plugin"));
        assert_eq!(plan.nro_name, "libshy_guy.nro");
        let named = |plan: &Plan, why: Why| -> Vec<PathBuf> {
            plan.found
                .iter()
                .filter(|place| place.why == why)
                .map(|place| place.path.clone())
                .collect()
        };
        assert_eq!(named(&plan, Why::SameName).len(), 2, "{:?}", plan.found);
        assert!(named(&plan, Why::DevReload)
            .iter()
            .any(|p| p.ends_with("romfs/smashline/development.nro")));

        // Renamed to `plugin.nro` by the mod: found by the crate's own paths, not by name. A
        // sibling built from a shared template matches only the shared files and loses.
        put(
            &sd,
            "ultimate/mods/[Moveset] Shy Guy/plugin.nro",
            "....src\\lib.rs....src\\shyguy\\a.rs..src\\shyguy\\b.rs..src\\shyguy\\c.rs..",
        );
        put(
            &sd,
            "ultimate/mods/[Moveset] Sceptile/plugin.nro",
            "....src\\lib.rs....src\\sceptile\\a.rs..",
        );
        let mut files = files.clone();
        for name in ["a", "b", "c"] {
            let file = root.join(format!("plugin/src/shyguy/{name}.rs"));
            put(root, &format!("plugin/src/shyguy/{name}.rs"), "");
            files.push(file);
        }
        let plan = super::plan(&files, &sources, Some(&sd)).unwrap();
        let guessed = named(&plan, Why::BuiltFromThisSource);
        assert_eq!(guessed.len(), 1, "{:?}", plan.found);
        assert!(guessed[0].ends_with("[Moveset] Shy Guy/plugin.nro"));

        // Pretend cargo ran: deploy only.
        put(
            root,
            "plugin/target/aarch64-skyline-switch/release/libshy_guy.nro",
            "new",
        );
        let built = plan.built.iter().find(|p| p.is_file()).unwrap();
        assert!(built.starts_with(root.join("plugin")));

        // Nothing found is written until the user says yes to it: with nothing approved, every
        // deployed copy and the guessed plugin stay as they were.
        let keep = root.join("keep");
        let mut log = String::new();
        let (written, failed) = deploy(built, &[], &keep, &mut log);
        assert!(written.is_empty() && failed.is_empty());
        for place in &plan.found {
            if place.why != Why::DevReload {
                assert_ne!(std::fs::read_to_string(&place.path).unwrap(), "new");
            }
        }
        assert_eq!(unasked(&plan.found, &[], &[]).len(), plan.found.len());
        let yes = vec![plan.found[0].path.clone()];
        let no = vec![guessed[0].clone()];
        assert_eq!(unasked(&plan.found, &yes, &no).len(), plan.found.len() - 2);

        // An approved place gets the build, and the file it held is kept away from the plugin's
        // folder, where Skyline would load a second copy.
        let (written, failed) = deploy(built, &yes, &keep, &mut log);
        assert_eq!((written.clone(), failed), (yes.clone(), Vec::new()));
        assert_eq!(std::fs::read_to_string(&yes[0]).unwrap(), "new");
        let kept = kept_copy(&keep, &yes[0]);
        assert_eq!(std::fs::read_to_string(&kept).unwrap(), "old");
        assert!(log.contains(&kept.display().to_string()));
        let beside: Vec<_> = std::fs::read_dir(yes[0].parent().unwrap())
            .unwrap()
            .flatten()
            .map(|entry| entry.file_name())
            .collect();
        assert_eq!(beside.len(), 1, "{beside:?}");
        assert_ne!(std::fs::read_to_string(&guessed[0]).unwrap(), "new");
    }

    /// Two movesets made from one template share their template's files: those alone never
    /// name a plugin as this crate's.
    #[test]
    fn template_files_alone_do_not_name_a_plugin() {
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path();
        let sd = root.join("sd");
        put(
            &sd,
            "ultimate/mods/Other/plugin.nro",
            "src/lib.rs src\\lib.rs src/fighter/mod.rs src\\fighter\\mod.rs src/util.rs",
        );
        let files: Vec<PathBuf> = ["src/lib.rs", "src/fighter/mod.rs", "src/util.rs"]
            .iter()
            .map(|rel| root.join("plugin").join(rel))
            .collect();
        assert_eq!(
            fingerprinted_plugin(&sd, &root.join("plugin"), &files),
            None
        );
    }

    #[test]
    fn a_lib_name_wins_over_the_package_name() {
        assert_eq!(
            library_name("[package]\nname = \"a-b\" # x\n[lib]\nname = \"plugin\"\n").as_deref(),
            Some("plugin")
        );
        assert_eq!(
            library_name("[package]\nname = \"a-b\"\n").as_deref(),
            Some("a-b")
        );
    }

    #[test]
    fn the_first_compiler_error_is_reported_with_its_location() {
        let stderr = "   Compiling x\nwarning: unused\nerror[E0425]: cannot find value `y`\n  --> src/lib.rs:3:5\n   |\n";
        assert_eq!(
            first_error(stderr),
            "error[E0425]: cannot find value `y` (src/lib.rs:3:5)"
        );
    }
}
