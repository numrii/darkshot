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
mod handoff;
mod help;
mod keys;
mod hdr;
mod icons;
#[cfg(feature = "open-with")]
mod openwith;
mod overlay;
mod perf;
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
/// `darkshot --reopen`     reopens a kept capture, read from standard input (the tray's
///                         Reopen last capture; see `handoff`).
/// Add `--trace` to either to log frame timings to `darkshot-trace.log` and memory, CPU
/// and GPU use to `darkshot-perf.log`, both in the temp folder (debug builds and builds
/// with the `trace` feature only; see `perf`).
/// `darkshot --trace-cycles 20` opens and closes the capture window 20 times, logging
///                         resources after each, for numbers to compare across versions.
fn main() -> AnyResult<()> {
    // Load DLLs only from System32, never from the exe's own folder or the current
    // directory, so a DLL planted next to a portable darkshot.exe is not picked up.
    // This covers DLLs loaded at run time (graphics and dialog libraries); the linker
    // flag /DEPENDENTLOADFLAG in .cargo/config.toml covers those loaded at start.
    use windows_sys::Win32::System::LibraryLoader::{SetDefaultDllDirectories, LOAD_LIBRARY_SEARCH_SYSTEM32};
    unsafe { SetDefaultDllDirectories(LOAD_LIBRARY_SEARCH_SYSTEM32) };

    let args: Vec<String> = std::env::args().skip(1).collect();
    let flag = |name: &str| args.iter().any(|a| a == name);
    let cycles = args.iter().position(|a| a == "--trace-cycles");
    let file = args
        .iter()
        .enumerate()
        .find(|&(i, a)| !a.starts_with("--") && cycles.is_none_or(|c| i != c + 1))
        .map(|(_, a)| std::path::PathBuf::from(a));
    if let Some(c) = cycles.filter(|_| perf::enabled() && !flag("--trace-autoclose")) {
        let n = args.get(c + 1).and_then(|n| n.parse().ok()).unwrap_or(20);
        daemon::cycles(n)
    } else if flag("--capture") {
        overlay::run(overlay::Source::Capture)
    } else if flag("--reopen") {
        overlay::run(overlay::Source::Kept)
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
