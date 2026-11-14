// Darkshot: a privacy-first screenshot tool. No network code, ever:
// a capture can only leave the app via the clipboard or a file on disk.
#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

mod annotate;
mod appicon;
#[cfg(feature = "autostart")]
mod autostart;
mod clipboard;
mod cursor;
mod daemon;
mod effects;
mod emoji;
mod help;
mod hdr;
mod icons;
mod overlay;
mod picker;
mod read;
mod save;
mod settings;
mod settings_window;
mod snap;
mod text;
mod tools;
#[cfg(feature = "free-shortcut")]
mod winkeys;

type AnyResult<T> = Result<T, Box<dyn std::error::Error>>;

/// `darkshot`              sits in the tray and waits for the hotkey.
/// `darkshot --capture`    takes one capture right now and exits.
/// `darkshot --settings`   opens the settings window.
/// `darkshot picture.png`  opens an image to edit (also: dragging it onto the exe).
/// `darkshot --open`       asks which image to edit.
/// `darkshot --clipboard`  edits the image on the clipboard.
/// Add `--trace` to either to log frame timings to `darkshot-trace.log` in the temp folder
/// (debug builds and builds with the `trace` feature only).
fn main() -> AnyResult<()> {
    // Load DLLs only from System32, never from the exe's own folder or the current
    // directory, so a DLL planted next to a portable darkshot.exe is not picked up.
    // This covers DLLs loaded at run time (graphics and dialog libraries); the linker
    // flag /DEPENDENTLOADFLAG in .cargo/config.toml covers those loaded at start.
    use windows_sys::Win32::System::LibraryLoader::{SetDefaultDllDirectories, LOAD_LIBRARY_SEARCH_SYSTEM32};
    unsafe { SetDefaultDllDirectories(LOAD_LIBRARY_SEARCH_SYSTEM32) };

    let args: Vec<String> = std::env::args().skip(1).collect();
    let flag = |name: &str| args.iter().any(|a| a == name);
    let file = args.iter().find(|a| !a.starts_with("--")).map(std::path::PathBuf::from);
    if flag("--capture") {
        overlay::run(overlay::Source::Capture)
    } else if flag("--settings") {
        settings_window::run()
    } else if flag("--clipboard") {
        overlay::run(overlay::Source::Clipboard)
    } else if flag("--open") || file.is_some() {
        overlay::run(overlay::Source::File(file))
    } else {
        daemon::run()
    }
}
