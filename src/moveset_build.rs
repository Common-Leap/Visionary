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
//! - every copy of that file already deployed — in an emulator SD's `ultimate/mods`, its Skyline
//!   plugins folder, or a LayeredFS `load/` mod — is replaced, plus any destination the user
//!   chose before. ARCropolis mods usually ship the plugin renamed to `plugin.nro`, which a name
//!   cannot find; a compiled plugin carries its crate's source paths in its panic locations
//!   (`src\shyguy\article_kind.rs`), so the `plugin.nro` that carries the most of THIS crate's
//!   paths — by a clear margin, since template code is shared between movesets — is the one;
//! - when the plugin exports `smashline_install` (smashline's development entry point), it is
//!   also written as smashline's `development.nro`, which smashline reloads in place when
//!   L + R + D-pad Up is pressed in the menus, so a change needs no restart at all.

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
    /// Deployed copies to replace.
    pub targets: Vec<PathBuf>,
    /// smashline's development plugin path, when the crate exports `smashline_install`.
    pub dev_reload: Option<PathBuf>,
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
/// entry point check); `sd_root` the emulator SD if one was found; `remembered` a destination
/// the user picked before.
pub fn plan(
    files: &[PathBuf],
    sources: &[String],
    sd_root: Option<&Path>,
    remembered: Option<&Path>,
) -> Result<Plan> {
    let cargo = files
        .iter()
        .find_map(|file| file.parent())
        .and_then(crate::cargo_project::CargoProject::discover);
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
                bail!("{} names no package", crate_dir.join("Cargo.toml").display());
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
    let mut targets = sd_root.map(|sd| deployed_copies(sd, &nro_name)).unwrap_or_default();
    if let Some(sd) = sd_root {
        if let Some(renamed) = fingerprinted_plugin(sd, &crate_dir, files) {
            targets.push(renamed);
        }
    }
    if let Some(chosen) = remembered {
        if !targets.iter().any(|t| t == chosen) {
            targets.push(chosen.to_path_buf());
        }
    }
    let exports_dev_entry = sources
        .iter()
        .any(|text| text.contains("fn smashline_install") && text.contains("no_mangle"));
    let dev_reload = sd_root.filter(|_| exports_dev_entry).map(|sd| {
        sd.join("atmosphere")
            .join("contents")
            .join(TITLE_ID)
            .join("romfs")
            .join("smashline")
            .join("development.nro")
    });
    Ok(Plan {
        crate_dir,
        nro_name,
        built,
        targets,
        dev_reload,
    })
}

/// Run the build and deploy. Blocking: call it on a worker thread.
pub fn run(plan: &Plan) -> Outcome {
    let log_path = plan.crate_dir.join("visionary-build.log");
    let output = std::process::Command::new("cargo")
        .args(["skyline", "build", "--release"])
        .current_dir(&plan.crate_dir)
        .output();
    let output = match output {
        Ok(output) => output,
        Err(error) => {
            return Outcome {
                ok: false,
                message: format!(
                    "could not start `cargo skyline build` ({error}) — is cargo-skyline installed?"
                ),
                log: None,
            }
        }
    };
    let stderr = String::from_utf8_lossy(&output.stderr).to_string();
    let log = format!(
        "--- stdout ---\n{}\n--- stderr ---\n{stderr}",
        String::from_utf8_lossy(&output.stdout)
    );
    let log = std::fs::write(&log_path, &log).ok().map(|_| log_path);
    if !output.status.success() {
        return Outcome {
            ok: false,
            message: format!("build failed: {}", first_error(&stderr)),
            log,
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
            message: format!("the build succeeded but {} was not found under target/", plan.nro_name),
            log,
        };
    };
    let mut written = Vec::new();
    let mut failed = Vec::new();
    for target in plan.targets.iter().chain(plan.dev_reload.iter()) {
        let result = target
            .parent()
            .map_or(Ok(()), std::fs::create_dir_all)
            .and_then(|_| std::fs::copy(&built, target).map(|_| ()));
        match result {
            Ok(()) => written.push(target.clone()),
            Err(error) => failed.push(format!("{} ({error})", target.display())),
        }
    }
    let message = if written.is_empty() && failed.is_empty() {
        format!(
            "built {} — no deployed copy was found to replace; use \"Deploy to…\" to choose where \
             it goes",
            built.display()
        )
    } else {
        let mut message = format!(
            "built and deployed {} to {} place(s)",
            plan.nro_name,
            written.len()
        );
        if plan.dev_reload.as_ref().is_some_and(|dev| written.contains(dev)) {
            message.push_str(" — press L + R + D-pad Up in the game's menus to hot-reload it");
        } else {
            message.push_str(" — restart the game to load it");
        }
        if !failed.is_empty() {
            message.push_str(&format!("; could not write {}", failed.join(", ")));
        }
        message
    };
    Outcome {
        ok: failed.is_empty(),
        message,
        log,
    }
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
    counts.into_iter().max_by_key(|(_, count)| *count).map(|(dir, _)| dir)
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
        let Some((key, value)) = line.split_once('=') else { continue };
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
/// paths embedded in it. `None` unless one candidate clearly beats the rest.
fn fingerprinted_plugin(sd: &Path, crate_dir: &Path, files: &[PathBuf]) -> Option<PathBuf> {
    let needles: Vec<Vec<u8>> = files
        .iter()
        .filter_map(|file| file.strip_prefix(crate_dir).ok())
        .flat_map(|relative| {
            let forward = relative.to_string_lossy().replace('\\', "/");
            [forward.replace('/', "\\").into_bytes(), forward.into_bytes()]
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
        let Ok(bytes) = std::fs::read(&plugin) else { continue };
        let score = needles
            .iter()
            .filter(|needle| contains(&bytes, needle))
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
    !needle.is_empty() && haystack.windows(needle.len()).any(|window| window == needle)
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
        find_named(&emulator.join("load").join(TITLE_ID), nro_name, 5, &mut found);
    }
    found.sort();
    found.dedup();
    found
}

fn find_named(dir: &Path, name: &str, depth: usize, out: &mut Vec<PathBuf>) {
    let Ok(entries) = std::fs::read_dir(dir) else { return };
    for entry in entries.flatten() {
        let path = entry.path();
        if path.is_dir() {
            if depth > 0 {
                find_named(&path, name, depth - 1, out);
            }
        } else if path.file_name().is_some_and(|n| n.eq_ignore_ascii_case(name)) {
            out.push(path);
        }
    }
}

/// The first compiler error, for the status line; the log has the rest.
fn first_error(stderr: &str) -> String {
    let mut lines = stderr.lines().skip_while(|line| !line.starts_with("error"));
    let Some(head) = lines.next() else {
        return stderr.lines().rev().find(|l| !l.trim().is_empty()).unwrap_or("").trim().to_string();
    };
    match lines.find(|line| line.trim_start().starts_with("-->")) {
        Some(at) => format!("{head} ({})", at.trim_start().trim_start_matches("-->").trim()),
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
        put(root, "plugin/Cargo.toml", "[package]\nname = \"shy-guy\"\n\n[lib]\ncrate-type = [\"cdylib\"]\n");
        put(root, "plugin/src/lib.rs", "#[no_mangle]\npub extern \"C\" fn smashline_install() {}\n");
        put(root, "crates/hash_lookup/Cargo.toml", "[package]\nname = \"hash_lookup\"\n");
        put(root, "crates/hash_lookup/src/lib.rs", "");
        let sd = root.join("emu/sdmc");
        put(&sd, "ultimate/mods/Shy Guy/libshy_guy.nro", "old");
        put(&sd, "ultimate/mods/Other/libother.nro", "old");
        put(root, "emu/load/01006A800016E000/ShyGuy/romfs/skyline/plugins/libshy_guy.nro", "old");

        let files = vec![root.join("plugin/src/lib.rs")];
        let sources = vec![std::fs::read_to_string(&files[0]).unwrap()];
        let plan = plan(&files, &sources, Some(&sd), None).unwrap();
        assert_eq!(plan.crate_dir, root.join("plugin"));
        assert_eq!(plan.nro_name, "libshy_guy.nro");
        assert_eq!(plan.targets.len(), 2, "{:?}", plan.targets);
        assert!(plan.dev_reload.as_ref().is_some_and(|p| p.ends_with("romfs/smashline/development.nro")));

        // Renamed to `plugin.nro` by the mod: found by the crate's own paths, not by name. A
        // sibling built from a shared template matches only the shared files and loses.
        put(&sd, "ultimate/mods/[Moveset] Shy Guy/plugin.nro", "....src\\lib.rs....src\\shyguy\\a.rs..src\\shyguy\\b.rs..src\\shyguy\\c.rs..");
        put(&sd, "ultimate/mods/[Moveset] Sceptile/plugin.nro", "....src\\lib.rs....src\\sceptile\\a.rs..");
        let mut files = files.clone();
        for name in ["a", "b", "c"] {
            let file = root.join(format!("plugin/src/shyguy/{name}.rs"));
            put(root, &format!("plugin/src/shyguy/{name}.rs"), "");
            files.push(file);
        }
        let plan = super::plan(&files, &sources, Some(&sd), None).unwrap();
        assert!(
            plan.targets.iter().any(|t| t.ends_with("[Moveset] Shy Guy/plugin.nro")),
            "{:?}",
            plan.targets
        );
        assert!(!plan.targets.iter().any(|t| t.to_string_lossy().contains("Sceptile")));

        // Pretend cargo ran: deploy only.
        put(root, "plugin/target/aarch64-skyline-switch/release/libshy_guy.nro", "new");
        let built = plan.built.iter().find(|p| p.is_file()).unwrap();
        assert!(built.starts_with(root.join("plugin")));
    }

    #[test]
    fn a_lib_name_wins_over_the_package_name() {
        assert_eq!(
            library_name("[package]\nname = \"a-b\" # x\n[lib]\nname = \"plugin\"\n").as_deref(),
            Some("plugin")
        );
        assert_eq!(library_name("[package]\nname = \"a-b\"\n").as_deref(), Some("a-b"));
    }

    #[test]
    fn the_first_compiler_error_is_reported_with_its_location() {
        let stderr = "   Compiling x\nwarning: unused\nerror[E0425]: cannot find value `y`\n  --> src/lib.rs:3:5\n   |\n";
        assert_eq!(first_error(stderr), "error[E0425]: cannot find value `y` (src/lib.rs:3:5)");
    }
}
