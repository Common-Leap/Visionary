//! User-local cache and temporary storage used by Visionary.

use std::path::{Path, PathBuf};
use std::sync::Mutex;

/// The name of the scratch .eff earlier builds wrote NEXT TO a fighter's source eff. Files left
/// by them are hidden from the source lists and deleted.
pub const TRANSPLANT_PREVIEW_FILE: &str = "_transplant_preview.eff";

/// The effect file `owner`'s borrowed effects are shown in: its source with every recorded
/// transplant merged in, the eff editor's pristine baseline. Kept in the editor's own folder,
/// since the source can be a file of the project's or of a mod's, which would ship it.
pub fn transplant_preview(owner: &str) -> PathBuf {
    app_storage_root()
        .join("borrowed")
        .join(format!("{owner}{TRANSPLANT_PREVIEW_FILE}"))
}

/// The pre-rename name of [`TRANSPLANT_PREVIEW_FILE`]. Kept so stale files written by
/// older builds are still hidden from the source lists and still deleted alongside the
/// current one, instead of being orphaned next to the user's game dump forever.
pub const LEGACY_TRANSPLANT_PREVIEW_FILE: &str = "_oneslot_preview.eff";

/// True for either the current or the legacy transplant-preview file name. Callers match
/// on a substring because the check also runs against full relative paths.
pub fn is_transplant_preview_name(name: &str) -> bool {
    name.contains("_transplant_preview") || name.contains("_oneslot_preview")
}

/// Delete the transplant preview next to `source_eff`, current and legacy names both.
pub fn remove_transplant_previews(dir: &Path) {
    let _ = std::fs::remove_file(dir.join(TRANSPLANT_PREVIEW_FILE));
    let _ = std::fs::remove_file(dir.join(LEGACY_TRANSPLANT_PREVIEW_FILE));
}

// ── The running emulator's SD root ────────────────────────────────────────────

/// Explicit SD root chosen by the user, if any. Set once at startup from the persisted
/// setting; portable emulator installs keep their SD next to the `.exe` rather than under any
/// platform directory, so probing alone cannot find them.
static SD_ROOT_OVERRIDE: Mutex<Option<PathBuf>> = Mutex::new(None);

/// Record the user's explicit SD root. `None` clears it and returns to probing.
pub fn set_emulator_sd_override(path: Option<PathBuf>) {
    if let Ok(mut slot) = SD_ROOT_OVERRIDE.lock() {
        *slot = path;
    }
}

/// Locate the SD root of the emulator running the game, or `None` if it cannot be found.
///
/// This used to be four copies of `home_dir()/.local/share/eden/sdmc` inlined at the call
/// sites, which is Eden's Linux layout and nothing else's. On Windows `home_dir()` is
/// `C:\Users\<name>`, so that resolved to `C:\Users\<name>\.local\share\eden\sdmc` — a path no
/// emulator uses. The write did not fail, because the caller created the directory first; it
/// succeeded into a junk tree in the user's profile, and the plugin was then told to read a
/// payload that was never anywhere it could see. That is what made every edit crash the game
/// for Windows testers.
///
/// `dirs::data_dir()` is the portable spelling of the same location: `~/.local/share` on
/// Linux (byte-identical to the old hardcoded path), `%APPDATA%` on Windows, and
/// `~/Library/Application Support` on macOS.
pub fn emulator_sd_root() -> Option<PathBuf> {
    // An explicit choice is taken at face value — the user may not have installed any mods
    // yet, so the `ultimate/` check below would reject a perfectly good directory.
    if let Some(path) = std::env::var_os("VISIONARY_SD_DIR")
        .map(PathBuf::from)
        .filter(|p| !p.as_os_str().is_empty())
        .or_else(|| SD_ROOT_OVERRIDE.lock().ok().and_then(|slot| slot.clone()))
    {
        return path.is_dir().then_some(path);
    }

    // Several emulators may be installed, each with a real SD holding `ultimate/`. Taking the
    // first in probe order wrote payloads to an idle eden SD while the game ran in sudachi, so
    // the plugin reported "could not read the payload". Pick the SD the game touched most
    // recently instead; probe order only breaks ties (or decides when nothing has been written).
    most_active_sd_root(sd_root_candidates())
}

/// The mods ARCropolis loads on the game's SD card: it skips folders whose name starts with a dot.
pub fn installed_mods() -> Vec<PathBuf> {
    let mods = emulator_sd_root().map(|sd| sd.join("ultimate").join("mods"));
    let entries = mods.and_then(|dir| std::fs::read_dir(dir).ok());
    let mut mods: Vec<PathBuf> = entries
        .into_iter()
        .flatten()
        .flatten()
        .filter(|entry| !entry.file_name().to_string_lossy().starts_with('.'))
        .map(|entry| entry.path())
        .filter(|path| path.is_dir())
        .collect();
    mods.sort();
    mods
}

/// The valid SD root among `candidates` with the newest game activity; the earliest wins a tie.
fn most_active_sd_root(candidates: Vec<PathBuf>) -> Option<PathBuf> {
    let mut best: Option<(Option<std::time::SystemTime>, PathBuf)> = None;
    for path in candidates {
        if !is_emulator_sd_root(&path) {
            continue;
        }
        let activity = last_activity(&path);
        let newer = match &best {
            None => true,
            Some((best_activity, _)) => activity > *best_activity,
        };
        if newer {
            best = Some((activity, path));
        }
    }
    best.map(|(_, path)| path)
}

/// When the game last wrote to this SD: the emulator's own FS access log, plus the files the
/// plugin rewrites while a match runs. `None` when none of them exist.
fn last_activity(sd: &Path) -> Option<std::time::SystemTime> {
    [
        "FsAccessLog.txt",
        "effect_viewer_probe.txt",
        "effect_viewer_capture.txt",
        "effect_viewer_marks.txt",
    ]
    .iter()
    .filter_map(|name| std::fs::metadata(sd.join(name)).ok()?.modified().ok())
    .max()
}

/// The standard SD locations to probe, most likely first. Split out so the path spelling can
/// be asserted in a test without depending on what happens to exist on the machine running it.
fn sd_root_candidates() -> Vec<PathBuf> {
    let data = dirs::data_dir();
    let config = dirs::config_dir();
    [
        data.as_ref().map(|d| d.join("eden").join("sdmc")),
        data.as_ref().map(|d| d.join("yuzu").join("sdmc")),
        // yuzu forks keep the same layout under their own name.
        data.as_ref().map(|d| d.join("sudachi").join("sdmc")),
        data.as_ref().map(|d| d.join("suyu").join("sdmc")),
        data.as_ref().map(|d| d.join("citron").join("sdmc")),
        data.as_ref().map(|d| d.join("torzu").join("sdmc")),
        config.as_ref().map(|c| c.join("Ryujinx").join("sdcard")),
        data.as_ref().map(|d| d.join("Ryujinx").join("sdcard")),
    ]
    .into_iter()
    .flatten()
    .collect()
}

/// Does this directory look like a real emulator SD root?
///
/// Testing `is_dir()` on the root itself is NOT enough. Anyone who ran a build from before the
/// path fix has a junk `C:\Users\<name>\.local\share\eden\sdmc` sitting in their profile,
/// created by the old code, and it would match. Requiring `ultimate/` — the Arcropolis mods
/// directory, the same test the live-deploy path has always used — distinguishes the real SD
/// from that leftover. Being too strict is the safe direction: callers fall back to sending
/// payloads over the wire, which works everywhere.
fn is_emulator_sd_root(path: &Path) -> bool {
    path.join("ultimate").is_dir()
}

/// Persistent cache root. `VISIONARY_CACHE_DIR` overrides the platform cache directory.
pub fn app_storage_root() -> PathBuf {
    std::env::var_os("VISIONARY_CACHE_DIR")
        .map(PathBuf::from)
        .filter(|path| !path.as_os_str().is_empty())
        .or_else(|| dirs::cache_dir().map(|path| path.join("visionary")))
        .unwrap_or_else(|| std::env::temp_dir().join("visionary"))
}

/// Unique temporary directory for EffectLibrary operations that need filesystem scratch space.
pub fn app_scratch_dir(prefix: &str) -> anyhow::Result<tempfile::TempDir> {
    let base = app_storage_root().join("scratch");
    std::fs::create_dir_all(&base)?;
    tempfile::Builder::new()
        .prefix(prefix)
        .tempdir_in(&base)
        .map_err(|error| {
            anyhow::anyhow!(
                "failed to create scratch directory in {}: {error} (set VISIONARY_CACHE_DIR to another disk)",
                base.display()
            )
        })
}

/// Replace a file the running game may be reading: write a sibling, then rename it over the
/// old one, so a reader sees the old file or the new one and never half of either.
pub fn replace_file(path: &Path, bytes: &[u8]) -> std::io::Result<()> {
    let mut partial = path.as_os_str().to_owned();
    partial.push(".partial");
    let partial = PathBuf::from(partial);
    std::fs::write(&partial, bytes)?;
    std::fs::rename(&partial, path).inspect_err(|_| {
        let _ = std::fs::remove_file(&partial);
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The whole point of routing through `dirs::data_dir()` is that it is the portable
    /// spelling of the path that was hardcoded before — so on Linux the first candidate has to
    /// come out byte-identical to the old `~/.local/share/eden/sdmc`, or this "fix" silently
    /// moves the working setup out from under the developer.
    #[cfg(target_os = "linux")]
    #[test]
    fn linux_first_candidate_matches_the_old_hardcoded_path() {
        let Some(home) = dirs::home_dir() else {
            return; // No home directory to compare against; nothing to assert.
        };
        // Only meaningful when XDG_DATA_HOME is unset or default — otherwise `data_dir()`
        // correctly points elsewhere and there is no "old path" to match.
        if std::env::var_os("XDG_DATA_HOME").is_some() {
            return;
        }
        assert_eq!(
            sd_root_candidates().first(),
            Some(&home.join(".local/share/eden/sdmc"))
        );
    }

    #[test]
    fn ryujinx_and_yuzu_layouts_are_probed_too() {
        let candidates = sd_root_candidates();
        let has = |needle: &str| {
            candidates
                .iter()
                .any(|c| c.to_string_lossy().replace('\\', "/").contains(needle))
        };
        assert!(has("eden/sdmc"), "{candidates:?}");
        assert!(has("yuzu/sdmc"), "{candidates:?}");
        assert!(has("Ryujinx/sdcard"), "{candidates:?}");
        assert!(has("sudachi/sdmc"), "{candidates:?}");
    }

    /// Two emulators both with a real SD: the one the game is running in must win, not the one
    /// probed first. Payloads written to the idle SD are invisible to the plugin.
    #[test]
    fn the_sd_the_game_wrote_to_last_wins() {
        let idle = tempfile::tempdir().unwrap();
        let live = tempfile::tempdir().unwrap();
        for sd in [idle.path(), live.path()] {
            std::fs::create_dir(sd.join("ultimate")).unwrap();
        }
        let old = std::time::SystemTime::now() - std::time::Duration::from_secs(3600);
        let probe = idle.path().join("effect_viewer_probe.txt");
        std::fs::write(&probe, b"x").unwrap();
        std::fs::File::options()
            .write(true)
            .open(&probe)
            .unwrap()
            .set_modified(old)
            .unwrap();
        std::fs::write(live.path().join("effect_viewer_probe.txt"), b"x").unwrap();

        let picked =
            most_active_sd_root(vec![idle.path().to_path_buf(), live.path().to_path_buf()]);
        assert_eq!(picked.as_deref(), Some(live.path()));
        // With no activity anywhere, probe order decides.
        let a = tempfile::tempdir().unwrap();
        let b = tempfile::tempdir().unwrap();
        std::fs::create_dir(a.path().join("ultimate")).unwrap();
        std::fs::create_dir(b.path().join("ultimate")).unwrap();
        let picked = most_active_sd_root(vec![a.path().to_path_buf(), b.path().to_path_buf()]);
        assert_eq!(picked.as_deref(), Some(a.path()));
    }

    /// A bare directory must NOT qualify. Testers who ran the broken build have an empty
    /// `~/.local/share/eden/sdmc` that the old code created; accepting it would send payloads
    /// straight back into the hole this fix exists to close.
    #[test]
    fn a_directory_without_ultimate_is_not_an_sd_root() {
        let dir = tempfile::tempdir().unwrap();
        assert!(!is_emulator_sd_root(dir.path()));
        std::fs::create_dir(dir.path().join("ultimate")).unwrap();
        assert!(is_emulator_sd_root(dir.path()));
    }

    // ── The plugin's frame path (R2) ──────────────────────────────────────────

    /// Every `fn` in the plugin, as (file, name, body text). Bodies are cut by brace depth.
    ///
    /// A `fn` is recognised only at depth 0 or 1 — a free function or one inside a single
    /// `impl`/`mod`. That is deliberate: a closure or a nested helper is attributed to the
    /// function containing it, which is the answer this check wants, since a closure runs on its
    /// caller's schedule.
    fn plugin_functions() -> Vec<(String, String, String)> {
        fn walk(dir: &Path, out: &mut Vec<PathBuf>) {
            let Ok(entries) = std::fs::read_dir(dir) else {
                return;
            };
            for entry in entries.flatten() {
                let path = entry.path();
                if path.is_dir() {
                    walk(&path, out);
                } else if path.extension().is_some_and(|e| e == "rs") {
                    out.push(path);
                }
            }
        }
        let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("plugins/slight_replica/src");
        let mut files = Vec::new();
        walk(&root, &mut files);
        assert!(
            !files.is_empty(),
            "no plugin sources under {} — this check is in the same repo as the plugin and \
             must not silently pass when it cannot find it",
            root.display()
        );

        let mut out = Vec::new();
        for file in files {
            let text = std::fs::read_to_string(&file).unwrap_or_default();
            let rel = file
                .strip_prefix(&root)
                .unwrap_or(&file)
                .to_string_lossy()
                .into_owned();
            let (mut current, mut depth, mut body) = (None::<String>, 0i32, String::new());
            for line in text.lines() {
                if depth <= 1 {
                    if let Some(name) = fn_name(line) {
                        if let Some(prev) = current.take() {
                            out.push((rel.clone(), prev, std::mem::take(&mut body)));
                        }
                        current = Some(name);
                    }
                }
                body.push_str(line);
                body.push('\n');
                depth += line.matches('{').count() as i32 - line.matches('}').count() as i32;
            }
            if let Some(prev) = current {
                out.push((rel, prev, body));
            }
        }
        out
    }

    /// The name declared by a `fn` line, ignoring visibility, `unsafe` and `extern "C"`.
    fn fn_name(line: &str) -> Option<String> {
        let mut rest = line.trim_start();
        for prefix in [
            "pub(crate) ",
            "pub(super) ",
            "pub ",
            "unsafe ",
            "extern \"C\" ",
        ] {
            rest = rest.strip_prefix(prefix).unwrap_or(rest);
        }
        let rest = rest.strip_prefix("fn ")?;
        let name: String = rest
            .chars()
            .take_while(|c| c.is_alphanumeric() || *c == '_')
            .collect();
        (!name.is_empty()).then_some(name)
    }

    /// Remove every `if <expr> % <n> == 0 { … }` block, leaving what runs on *every* frame.
    ///
    /// Matched on the source text rather than on a parse because the throttle in this plugin is
    /// always written this one way. If that ever stops being true the block stops being stripped
    /// and this check reports a violation that is not one — a false alarm, which is the failure
    /// direction to prefer here.
    fn strip_throttled(body: &str) -> String {
        let mut out = String::new();
        let mut rest = body;
        while let Some(start) = find_throttle(rest) {
            out.push_str(&rest[..start]);
            let after = &rest[start..];
            let Some(open) = after.find('{') else {
                break;
            };
            let mut depth = 0i32;
            let mut end = None;
            for (i, c) in after[open..].char_indices() {
                match c {
                    '{' => depth += 1,
                    '}' => {
                        depth -= 1;
                        if depth == 0 {
                            end = Some(open + i + 1);
                            break;
                        }
                    }
                    _ => {}
                }
            }
            match end {
                Some(e) => rest = &after[e..],
                None => break,
            }
        }
        out.push_str(rest);
        out
    }

    /// Drop `//` comments, so prose about the code is not read as the code.
    ///
    /// Line comments only. The plugin uses no block comments, and a `//` inside a string literal
    /// would cost this check a line it did not need to read rather than hide a call.
    fn strip_comments(body: &str) -> String {
        body.lines()
            .map(|line| line.split_once("//").map_or(line, |(code, _)| code))
            .collect::<Vec<_>>()
            .join("\n")
    }

    /// Offset of the next `if … % … == 0` header, if any.
    fn find_throttle(text: &str) -> Option<usize> {
        text.match_indices("if ").find_map(|(i, _)| {
            let line_end = text[i..].find('\n').map_or(text.len(), |n| i + n);
            let header = &text[i..line_end];
            (header.contains('%') && header.contains("== 0") && header.contains('{')).then_some(i)
        })
    }

    /// No `on_frame` may reach filesystem work without a throttle in between.
    ///
    /// This is R2's regression guard, and it is aimed at the defect R2 actually found rather
    /// than at the one the entry predicted. The prediction was that someone would *add* an SD
    /// probe to a frame path; what had really happened is that `poll_transactions` — a long
    /// standing, legitimately filesystem-touching function — was **called** from an ungated
    /// `on_frame`, costing nine filesystem operations per frame on the game thread. A ratchet on
    /// the *number* of filesystem call sites would have been green through all of it. So the
    /// property checked here is cadence, not count.
    ///
    /// **Both halves are derived, neither is a hand-maintained list.** The set of
    /// filesystem-touching functions comes from scanning for `std::fs::` and `.exists()`; the
    /// frame paths from every `on_frame`, `on_*_frame` and `run_one_frame` in the plugin — 32 of
    /// them. Nothing here has to be updated when the plugin gains a subsystem, which is the only
    /// reason it will still be true in a year.
    ///
    /// Comments are stripped first. A draft of this check reported `run_one_frame` calling
    /// `flush` on the strength of the words *"buffer flush (file I/O only every 30 frames)"* in
    /// a comment — the throttle it was describing being right there in the code below it.
    ///
    /// **What it does not prove.** It is one hop deep: `on_frame` → a function that itself does
    /// I/O. A chain through an intermediate that only *calls* an I/O function is invisible, and
    /// so is anything reached through a trait object or a function pointer. It is a tripwire on
    /// the shape that has now gone wrong twice, not a proof of absence — the honest verification
    /// for this bug class is still a Windows tester's frame time, which is why R2 says the class
    /// cannot be closed from this machine.
    #[test]
    fn no_plugin_frame_path_reaches_the_filesystem_ungated() {
        let functions = plugin_functions();
        let touches_fs: Vec<&str> = functions
            .iter()
            .filter(|(_, _, body)| body.contains("std::fs::") || body.contains(".exists()"))
            .map(|(_, name, _)| name.as_str())
            .collect();
        assert!(
            touches_fs.len() > 20,
            "only {} filesystem-touching functions found in the plugin — the scan is broken, \
             not the plugin (it had 45 when this was written)",
            touches_fs.len()
        );

        let is_frame_entry = |name: &str| {
            name == "on_frame"
                || name == "run_one_frame"
                || (name.starts_with("on_") && name.ends_with("_frame"))
        };
        let mut violations: Vec<String> = Vec::new();
        let mut entries = 0usize;
        for (file, _, body) in functions.iter().filter(|(_, n, _)| is_frame_entry(n)) {
            entries += 1;
            let ungated = strip_throttled(&strip_comments(body));
            for callee in &touches_fs {
                // `name(` with a non-identifier char before it, so `poll_transactions` does not
                // match inside `finish_poll_transactions` and a definition is not a call.
                let called = ungated.match_indices(&format!("{callee}(")).any(|(i, _)| {
                    i == 0
                        || !ungated.as_bytes()[i - 1].is_ascii_alphanumeric()
                            && ungated.as_bytes()[i - 1] != b'_'
                });
                if called {
                    violations.push(format!("{file}: on_frame calls `{callee}` every frame"));
                }
            }
        }
        assert!(
            entries > 25,
            "only {entries} frame entry points found — the scan is broken, not the plugin \
             (it had 32 when this was written)"
        );
        violations.sort();
        violations.dedup();
        assert!(
            violations.is_empty(),
            "filesystem work on a per-frame path — free on Linux, 20-200 µs per call on Windows \
             against a 16.6 ms budget. Put it behind a throttle (see `slight::sd_poll`) or, if \
             it genuinely must run every frame, say why here:\n  {}",
            violations.join("\n  ")
        );
    }
}
