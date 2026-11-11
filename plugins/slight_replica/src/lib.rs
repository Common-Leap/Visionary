#![feature(proc_macro_hygiene)]
// The replica intentionally retains framework-compatible facade APIs that this standalone plugin
// does not call directly. They are documented and audited in AUDIT.md.
#![allow(dead_code)]

mod rust_extender;
mod slight;

pub const PLUGIN_VERSION: &str = "0.1.0-slight-replica";

/// Every copy of this plugin exports this flag: the live one and the one inside each exported
/// mod. The game's loader binds every copy's use of the name to the first copy loaded, so all
/// copies share one flag; the first to raise it installs and the rest stay off, and two copies
/// never hook the game twice. The live plugin loads first and replays every mod's saved edits.
/// A mod built with its own code compiled in (`carries_code`) always runs.
#[no_mangle]
pub static VISIONARY_RUNTIME_CLAIMED: core::sync::atomic::AtomicBool =
    core::sync::atomic::AtomicBool::new(false);

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
#[skyline::main(name = "_effect_viewer")]
pub fn smashline_install() {
    if !this_copy_runs() {
        skyline::println!(
            "[SLight] Another copy of the Visionary runtime is loaded; this one stays off"
        );
        return;
    }
    skyline::println!("[SLight] Start preinstall");
    slight::smash_utils::ensure_slight_dirs();
    slight::diag::start_session();
    // The disabled set belongs in the boot log, not diag.txt: diag only flushes from the
    // per-frame driver, so a boot that never reaches a match leaves no other record of which
    // subsystems this run actually installed.
    let _ = std::fs::write(
        format!("{}/effect_viewer_boot.txt", slight::smash_utils::ERROR_LOGS),
        format!(
            "lib_effect_viewer v{PLUGIN_VERSION}\nbuild={}\ncopy={}\ndisabled={}\n",
            slight::effect_viewer::live_eff::BUILD_TAG,
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
