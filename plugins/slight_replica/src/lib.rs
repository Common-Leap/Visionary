#![feature(proc_macro_hygiene)]
// The replica intentionally retains framework-compatible facade APIs that this standalone plugin
// does not call directly. They are documented and audited in AUDIT.md.
#![allow(dead_code)]

mod rust_extender;
mod slight;

pub const PLUGIN_VERSION: &str = "0.1.0-slight-replica";

/// Every copy of this plugin exports this: the live one and the one inside each exported mod.
/// The game resolves the name to the first copy it loaded, and only that copy installs, so two
/// copies never hook the game twice. The live plugin loads first and replays every mod's edits.
#[no_mangle]
pub static VISIONARY_RUNTIME_OWNER: u8 = 0;

fn this_copy_runs() -> bool {
    let mut address = 0usize;
    let result = unsafe {
        skyline::nn::ro::LookupSymbol(&mut address, c"VISIONARY_RUNTIME_OWNER".as_ptr().cast())
    };
    result != 0 || address == 0 || address == core::ptr::addr_of!(VISIONARY_RUNTIME_OWNER) as usize
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
            "lib_effect_viewer v{PLUGIN_VERSION}\nbuild={}\ndisabled={}\n",
            slight::effect_viewer::live_eff::BUILD_TAG,
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
