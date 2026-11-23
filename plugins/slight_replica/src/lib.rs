#![feature(proc_macro_hygiene)]
// The replica intentionally retains framework-compatible facade APIs that this standalone plugin
// does not call directly. They are documented and audited in AUDIT.md.
#![allow(dead_code)]

mod offsets;
#[cfg(not(feature = "mod_plugin"))]
mod rust_extender;
mod slight;

pub const PLUGIN_VERSION: &str = "0.1.0-slight-replica";

/// The plugin inside an exported mod: the framework and that mod's moves and gimmicks, with no
/// editor connection, no files written to the SD card, and nothing shared with other plugins,
/// so mods that change different things load side by side. Export names it after the mod.
#[cfg(feature = "mod_plugin")]
#[skyline::main(name = "visionary_mod")]
pub fn mod_plugin_main() {
    // Offset-backed reads (the fighter-name table) need the game build first.
    if offsets::initialize().is_none() {
        skyline::println!("[visionary_mod] Unsupported SSBU version. Expected 13.0.4 or 13.0.5.");
        return;
    }
    slight::agent_extender::install();
    slight::main_smash::install_all();
}

#[cfg(not(feature = "mod_plugin"))]
/// Every copy of this plugin exports this flag: the live one and the one inside each exported
/// mod. The game's loader binds every copy's use of the name to the first copy loaded, so all
/// copies share one flag; the first to raise it installs and the rest stay off, and two copies
/// never hook the game twice. The live plugin loads first and replays every mod's saved edits.
/// A mod built with its own code compiled in (`carries_code`) always runs.
#[cfg(not(feature = "mod_plugin"))]
#[no_mangle]
pub static VISIONARY_RUNTIME_CLAIMED: core::sync::atomic::AtomicBool =
    core::sync::atomic::AtomicBool::new(false);

#[cfg(not(feature = "mod_plugin"))]
fn this_copy_runs() -> bool {
    let first = !VISIONARY_RUNTIME_CLAIMED.swap(true, core::sync::atomic::Ordering::AcqRel);
    let runs = first || cfg!(feature = "carries_code");
    // One line per copy, so a boot shows which copy installed.
    use std::io::Write;
    if let Ok(mut log) = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open("sd:/slight/runtime_copies.txt")
    {
        let _ = writeln!(
            log,
            "{} copy {}",
            if cfg!(feature = "exported") {
                "mod"
            } else {
                "live"
            },
            if runs {
                "installed"
            } else {
                "stayed off: another copy was already running"
            }
        );
    }
    runs
}

/// Jorge entry — `smashline_install` @ 71000013d0.
#[cfg(not(feature = "mod_plugin"))]
#[skyline::main(name = "_effect_viewer")]
pub fn smashline_install() {
    if !this_copy_runs() {
        skyline::println!(
            "[SLight] Another copy of the Visionary runtime is loaded; this one stays off"
        );
        return;
    }
    // A panic aborts the game; keep its message where a tester can find and send it.
    std::panic::set_hook(Box::new(|info| {
        let _ = std::fs::write("sd:/slight/panic.txt", format!("{info}\n"));
    }));
    let Some(game_version) = offsets::initialize().map(offsets::GameVersion::name) else {
        let version = skyline::nn::oe::get_display_version();
        // Nothing listens on this version, so this file is how the editor learns why.
        let _ = std::fs::create_dir_all("sd:/slight");
        let _ = std::fs::write(slight::smash_utils::UNSUPPORTED_GAME, version.as_str());
        skyline::println!(
            "[SLight] Unsupported SSBU version {}. Expected 13.0.4 or 13.0.5.",
            version.as_str()
        );
        return;
    };
    let _ = std::fs::remove_file(slight::smash_utils::UNSUPPORTED_GAME);
    skyline::println!("[SLight] Start preinstall on SSBU {game_version}");
    skyline::println!("[SLight] plugin loader: {}", slight::loader::current().name());
    slight::smash_utils::ensure_slight_dirs();
    slight::diag::start_session();
    // The disabled set belongs in the boot log, not diag.txt: diag only flushes from the
    // per-frame driver, so a boot that never reaches a match leaves no other record of which
    // subsystems this run actually installed.
    let _ = std::fs::write(
        format!("{}/effect_viewer_boot.txt", slight::smash_utils::ERROR_LOGS),
        format!(
            "lib_effect_viewer v{PLUGIN_VERSION}\ngame={game_version}\nbuild={}\nloader={}\ncopy={}\ndisabled={}\n",
            slight::effect_viewer::live_eff::BUILD_TAG,
            slight::loader::current().name(),
            if cfg!(feature = "exported") {
                "mod"
            } else {
                "live"
            },
            slight::smash_utils::disabled_subsystems(),
        ),
    );

    slight::effect_viewer::install_hooks();
    if !slight::smash_utils::subsystem_disabled("agent") {
        slight::agent_extender::install();
    }
    if !slight::smash_utils::subsystem_disabled("systems") {
        slight::main_smash::install_all();
    }

    skyline::println!(
        "[SLight] Mod powered using the SLight framework by Jorge Rico Vivas (replica v{PLUGIN_VERSION})"
    );
}
