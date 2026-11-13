// Darkshot: a privacy-first screenshot tool. No network code, ever:
// a capture can only leave the app via the clipboard or a file on disk.
#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

mod annotate;
mod appicon;
#[cfg(feature = "autostart")]
mod autostart;
mod cursor;
mod daemon;
mod effects;
mod hdr;
mod icons;
mod overlay;
mod picker;
mod save;
mod settings;
mod settings_window;
mod snap;
mod text;
mod tools;
#[cfg(feature = "free-shortcut")]
mod winkeys;

type AnyResult<T> = Result<T, Box<dyn std::error::Error>>;

/// `darkshot`            sits in the tray and waits for the hotkey.
/// `darkshot --capture`  takes one capture right now and exits.
/// `darkshot --settings` opens the settings window.
/// Add `--trace` to either to log frame timings to `darkshot-trace.log` in the temp folder
/// (debug builds and builds with the `trace` feature only).
fn main() -> AnyResult<()> {
    // Load DLLs only from System32, never from the exe's own folder or the current
    // directory, so a DLL planted next to a portable darkshot.exe is not picked up.
    // This covers DLLs loaded at run time (graphics and dialog libraries); the linker
    // flag /DEPENDENTLOADFLAG in .cargo/config.toml covers those loaded at start.
    use windows_sys::Win32::System::LibraryLoader::{SetDefaultDllDirectories, LOAD_LIBRARY_SEARCH_SYSTEM32};
    unsafe { SetDefaultDllDirectories(LOAD_LIBRARY_SEARCH_SYSTEM32) };

    if std::env::args().any(|a| a == "--capture") {
        overlay::run()
    } else if std::env::args().any(|a| a == "--settings") {
        settings_window::run()
    } else {
        daemon::run()
    }
}
