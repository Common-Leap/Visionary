//! Which plugin loader is running, and hook targets that are safe under both.
//!
//! Skyline and ozone (an exlaunch-based drop-in replacement) expose the same plugin API, with
//! two differences that matter here:
//!
//! - Ozone loads plugins with `BindFlag_Now`, so an import that no loaded module exports is
//!   bound to address 0 when the plugin loads. Symbols that live in a game module loaded
//!   later, such as `lua2cpp_common`, are still unresolved at that point.
//! - Skyline's `A64HookFunction` ignores a null target. Ozone aborts the process.
//!
//! Hooks on symbols outside the main executable therefore resolve their target through
//! `nn::ro::LookupSymbol` when the owning module is present, and are skipped until then.

use std::sync::LazyLock;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Loader {
    Skyline,
    Ozone,
}

impl Loader {
    pub fn name(self) -> &'static str {
        match self {
            Loader::Skyline => "skyline",
            Loader::Ozone => "ozone",
        }
    }
}

/// Exports only ozone provides: its abort logger and the exlaunch module-info bridge.
static LOADER: LazyLock<Loader> = LazyLock::new(|| {
    if lookup(b"ozone_log_abort\0").is_some() || lookup(b"sky_GetModuleInfo\0").is_some() {
        Loader::Ozone
    } else {
        Loader::Skyline
    }
});

pub fn current() -> Loader {
    *LOADER
}

/// `name` must be NUL-terminated.
pub fn lookup(name: &[u8]) -> Option<usize> {
    debug_assert_eq!(name.last(), Some(&0));
    let mut addr = 0usize;
    let rc = unsafe { skyline::nn::ro::LookupSymbol(&mut addr, name.as_ptr() as *const _) };
    (rc == 0 && addr != 0).then_some(addr)
}

/// Hook target for a symbol that may live outside the main executable. Prefers the address the
/// loader bound for `import`, falls back to a runtime lookup of `mangled`, and returns null
/// when neither is available. Callers must not install a hook on a null target.
pub fn target(import: *const (), mangled: &[u8]) -> *const u8 {
    // Rust assumes function addresses are non-null; the opaque copy keeps the check.
    let import = core::hint::black_box(import);
    if !import.is_null() {
        return import as *const u8;
    }
    lookup(mangled).map_or(core::ptr::null(), |addr| addr as *const u8)
}
