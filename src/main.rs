// Visionary: character/circle preview, authored effect editing, and live in-game rendering.
mod acmd;
mod acmd_src;
mod acmd_verify;
mod app;
mod app_icon;
mod asset_snapshots;
mod capture_hitbox;
mod carrier_motion;
mod carrier_support;
mod credits;
mod data;
mod eff_attrs;
mod eff_editor;
mod eff_export;
mod eff_subsections;
mod effect_pool;
mod effects;
mod emulator;
mod game_link;
mod gimmicks;
mod mod_export;
mod mod_import;
mod mod_project;
mod move_kinds;
mod move_nodes;
mod param_labels;
mod plugin_deploy;
mod project_hub;
mod renderer;
mod roster;
mod scratch_dirs;
mod texture_import;
mod ui;
#[cfg(target_os = "linux")]
mod wayland_icon;

fn main() -> anyhow::Result<()> {
    let mut args = std::env::args_os().skip(1);
    let first = args.next();
    if matches!(first.as_deref(), Some(arg) if arg == "--help" || arg == "-h") {
        println!(
            "Visionary - move and character editor\n\
             Usage: visionary [COMMAND]\n\n\
             Run without arguments to open the editor.\n\
             Headless commands:\n\
               --gimmick-package       Package a behavior adapter\n\
               --install-carrier-support [SD] Install live asset support\n\n\
               --help, -h              Show this help without launching\n\
               --version, -V           Show the version without launching"
        );
        return Ok(());
    }
    if matches!(first.as_deref(), Some(arg) if arg == "--version" || arg == "-V") {
        println!("Visionary {}", env!("CARGO_PKG_VERSION"));
        return Ok(());
    }
    if first.as_deref() == Some(std::ffi::OsStr::new("--install-carrier-support")) {
        let sd_arg = args.next();
        if matches!(sd_arg.as_deref(), Some(arg) if arg == "--help" || arg == "-h") {
            println!("Usage: visionary --install-carrier-support [SD]\nInstall live asset support into SD, or the configured emulator SD directory.");
            return Ok(());
        }
        let sd = sd_arg
            .map(std::path::PathBuf::from)
            .or_else(scratch_dirs::emulator_sd_root)
            .ok_or_else(|| anyhow::anyhow!("emulator SD directory is not configured"))?;
        println!("{}", carrier_support::install(&sd)?.display());
        return Ok(());
    }
    if first.as_deref() == Some(std::ffi::OsStr::new("--gimmick-package")) {
        return gimmicks::cli::run_package(args);
    }
    if let Some(arg) = &first {
        if arg.to_string_lossy().starts_with('-') {
            anyhow::bail!("unknown command {arg:?}; use --help for headless commands");
        }
    }
    if let Some(sd) = scratch_dirs::emulator_sd_root() {
        if let Err(error) = carrier_support::install(&sd) {
            eprintln!("[visionary] Live asset support: {error:#}");
        }
    }
    // Best-effort auto-deploy the Skyline plugin to the local Eden install so the
    // user only has to run Eden + Visionary to test. This is non-blocking and
    // never fails the desktop launch — check stderr for "[visionary] Plugin auto-deploy".
    plugin_deploy::spawn_background_check();

    // RADV can fail silently during wgpu's Linux auto-detection. Keep the working Vulkan
    // default there, while respecting an explicit user choice and leaving Windows to select
    // DirectX/Vulkan normally.
    #[cfg(target_os = "linux")]
    if std::env::var_os("WGPU_BACKEND").is_none() {
        std::env::set_var("WGPU_BACKEND", "vulkan");
    }
    let options = eframe::NativeOptions {
        viewport: egui::ViewportBuilder::default()
            .with_app_id(app_icon::APP_ID)
            .with_icon(app_icon::viewport_icon())
            .with_title("Visionary")
            .with_inner_size([1400.0, 900.0])
            .with_min_inner_size([800.0, 600.0]),
        renderer: eframe::Renderer::Wgpu,
        wgpu_options: egui_wgpu::WgpuConfiguration {
            wgpu_setup: egui_wgpu::WgpuSetup::CreateNew(egui_wgpu::WgpuSetupCreateNew {
                instance_descriptor: wgpu::InstanceDescriptor::new_without_display_handle(),
                display_handle: None,
                power_preference: wgpu::PowerPreference::HighPerformance,
                native_adapter_selector: None,
                device_descriptor: std::sync::Arc::new(|adapter| {
                    // Only request ssbh_wgpu features if the adapter supports them.
                    // This prevents a blank window on GPUs/drivers that lack BC compression etc.
                    let supported = adapter.features();
                    let wanted = ssbh_wgpu::REQUIRED_FEATURES;
                    let features = if supported.contains(wanted) {
                        wanted
                    } else {
                        eprintln!(
                            "Warning: GPU does not support all ssbh_wgpu features. \
                             Missing: {:?}. 3D rendering will be disabled.",
                            wanted - supported
                        );
                        wgpu::Features::empty()
                    };
                    wgpu::DeviceDescriptor {
                        label: Some("visionary"),
                        required_features: features,
                        required_limits: adapter.limits(),
                        memory_hints: wgpu::MemoryHints::default(),
                        ..Default::default()
                    }
                }),
            }),
            ..Default::default()
        },
        ..Default::default()
    };

    eframe::run_native(
        "Visionary",
        options,
        Box::new(|cc| Ok(Box::new(app::VisionaryApp::new(cc)))),
    )
    .map_err(|e| anyhow::anyhow!("{e}"))
}

pub(crate) fn debug_enabled() -> bool {
    std::env::var_os("VISIONARY_DEBUG").is_some()
}
