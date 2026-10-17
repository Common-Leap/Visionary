//! The console window behind Visionary's log output.
//!
//! On Windows Visionary is a GUI-subsystem program, so nothing opens a console for it. Started
//! from a terminal, it attaches to that terminal and logs there as before. Otherwise a console
//! is opened only while the "Show console window" preference is on.
//!
//! Elsewhere the process's stdout and stderr are whatever launched it, and there is nothing to
//! show or hide.

#[cfg(windows)]
mod imp {
    use std::sync::atomic::{AtomicBool, Ordering};

    const ATTACH_PARENT_PROCESS: u32 = u32::MAX;
    const SC_CLOSE: u32 = 0xF060;
    const MF_BYCOMMAND: u32 = 0;

    #[link(name = "kernel32")]
    extern "system" {
        fn AttachConsole(process_id: u32) -> i32;
        fn AllocConsole() -> i32;
        fn FreeConsole() -> i32;
        fn GetConsoleWindow() -> *mut core::ffi::c_void;
        fn SetConsoleTitleW(title: *const u16) -> i32;
    }

    #[link(name = "user32")]
    extern "system" {
        fn GetSystemMenu(window: *mut core::ffi::c_void, revert: i32) -> *mut core::ffi::c_void;
        fn DeleteMenu(menu: *mut core::ffi::c_void, position: u32, flags: u32) -> i32;
    }

    static FROM_TERMINAL: AtomicBool = AtomicBool::new(false);
    static OWN_CONSOLE: AtomicBool = AtomicBool::new(false);

    pub fn init(show: bool) {
        // SAFETY: plain Win32 calls with no pointers handed over.
        if unsafe { AttachConsole(ATTACH_PARENT_PROCESS) } != 0 {
            FROM_TERMINAL.store(true, Ordering::Relaxed);
        } else if show {
            set_visible(true);
        }
    }

    pub fn from_terminal() -> bool {
        FROM_TERMINAL.load(Ordering::Relaxed)
    }

    pub fn has_console() -> bool {
        from_terminal() || OWN_CONSOLE.load(Ordering::Relaxed)
    }

    pub fn set_visible(show: bool) {
        if from_terminal() || OWN_CONSOLE.load(Ordering::Relaxed) == show {
            return;
        }
        if show {
            // SAFETY: the title is a NUL-terminated UTF-16 buffer that outlives the call; the
            // window and menu handles come straight from Win32 and are checked for null.
            unsafe {
                if AllocConsole() == 0 {
                    return;
                }
                let title: Vec<u16> = "Visionary console".encode_utf16().chain([0]).collect();
                SetConsoleTitleW(title.as_ptr());
                // Closing a console ends every process attached to it. Hide the close button
                // so the window is dismissed through the preference instead.
                let window = GetConsoleWindow();
                if !window.is_null() {
                    let menu = GetSystemMenu(window, 0);
                    if !menu.is_null() {
                        DeleteMenu(menu, SC_CLOSE, MF_BYCOMMAND);
                    }
                }
            }
            OWN_CONSOLE.store(true, Ordering::Relaxed);
        } else {
            // SAFETY: detaches from the console this process allocated above.
            unsafe { FreeConsole() };
            OWN_CONSOLE.store(false, Ordering::Relaxed);
        }
    }
}

#[cfg(not(windows))]
mod imp {
    pub fn init(_show: bool) {}

    pub fn from_terminal() -> bool {
        true
    }

    pub fn set_visible(_show: bool) {}
}

pub use imp::{from_terminal, init, set_visible};
#[cfg(windows)]
use imp::has_console;

/// Whether this platform has a console window Visionary can show or hide.
pub const SUPPORTED: bool = cfg!(windows);

/// Keeps a console program Visionary runs (cargo, bash, python) from opening a console window
/// of its own. A GUI-subsystem parent has no console for it to share, so Windows would make one.
/// While Visionary has a console the child shares it, and its output shows there.
pub trait NoConsoleWindow {
    fn no_console_window(&mut self) -> &mut Self;
}

impl NoConsoleWindow for std::process::Command {
    #[cfg(windows)]
    fn no_console_window(&mut self) -> &mut Self {
        use std::os::windows::process::CommandExt;
        const CREATE_NO_WINDOW: u32 = 0x0800_0000;
        if has_console() {
            self
        } else {
            self.creation_flags(CREATE_NO_WINDOW)
        }
    }

    #[cfg(not(windows))]
    fn no_console_window(&mut self) -> &mut Self {
        self
    }
}
