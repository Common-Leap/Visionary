#![feature(proc_macro_hygiene)]
// The replica intentionally retains framework-compatible facade APIs that this standalone plugin
// does not call directly. They are documented and audited in AUDIT.md.
#![allow(dead_code)]

mod offsets;
mod rust_extender;
mod slight;

pub const PLUGIN_VERSION: &str = "0.1.0-slight-replica";

/// Jorge entry — `smashline_install` @ 71000013d0.
#[skyline::main(name = "_effect_viewer")]
pub fn smashline_install() {
    let game_version = match offsets::initialize() {
        Some(offsets::GameVersion::V13_0_4) => "13.0.4",
        Some(offsets::GameVersion::V13_0_5) => "13.0.5",
        None => {
            skyline::println!("[SLight] Unsupported SSBU version. Expected 13.0.4 or 13.0.5.");
            return;
        }
    };
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
            "lib_effect_viewer v{PLUGIN_VERSION}\ngame={game_version}\nbuild={}\nloader={}\ndisabled={}\n",
            slight::effect_viewer::live_eff::BUILD_TAG,
            slight::loader::current().name(),
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
