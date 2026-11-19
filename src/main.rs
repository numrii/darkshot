// Duskshot: a privacy-first screenshot tool. No network code, ever:
// a capture can only leave the app via the clipboard or a file on disk.
#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

mod annotate;
#[cfg(windows)]
mod appicon;
#[cfg(all(windows, feature = "autostart"))]
mod autostart;
mod clipboard;
#[cfg(windows)]
mod cursor;
#[cfg(windows)]
mod daemon;
mod effects;
mod emoji;
mod handoff;
mod help;
mod keys;
#[cfg(windows)]
mod hdr;
mod icons;
#[cfg(all(windows, feature = "open-with"))]
mod openwith;
mod overlay;
mod perf;
mod picker;
mod platform;
mod read;
mod save;
mod settings;
mod settings_window;
mod snap;
mod text;
mod theme;
mod tools;
#[cfg(all(windows, feature = "free-shortcut"))]
mod winkeys;

type AnyResult<T> = Result<T, Box<dyn std::error::Error>>;

/// Load DLLs only from System32, never from the executable directory or current
/// directory. This is a Windows-only hardening measure; other platforms do not have
/// this loader API.
#[cfg(windows)]
fn harden_dynamic_loader() {
    use windows_sys::Win32::System::LibraryLoader::{
        SetDefaultDllDirectories, LOAD_LIBRARY_SEARCH_SYSTEM32,
    };
    unsafe { SetDefaultDllDirectories(LOAD_LIBRARY_SEARCH_SYSTEM32) };
}

#[cfg(not(windows))]
fn harden_dynamic_loader() {}

#[cfg(windows)]
fn run_trace_cycles(n: u32) -> AnyResult<()> {
    daemon::cycles(n)
}

#[cfg(not(windows))]
fn run_trace_cycles(_n: u32) -> AnyResult<()> {
    Err("--trace-cycles requires the Windows tray implementation".into())
}

#[cfg(windows)]
fn run_default() -> AnyResult<()> {
    daemon::run()
}

#[cfg(not(windows))]
fn run_default() -> AnyResult<()> {
    Err("use --capture, --open, --clipboard, or --settings; Linux has no background tray daemon".into())
}

#[cfg(windows)]
fn reopen_kept_capture() -> AnyResult<()> {
    overlay::run(overlay::Source::Kept)
}

#[cfg(not(windows))]
fn reopen_kept_capture() -> AnyResult<()> {
    Err("--reopen requires the Windows tray implementation".into())
}

/// `duskshot`              sits in the tray and waits for the hotkey.
/// `duskshot --capture`    takes one capture right now and exits.
/// `duskshot --settings`   opens the settings window.
/// `duskshot picture.png`  opens an image to edit (also: dragging it onto the exe).
/// `duskshot --open`       asks which image to edit.
/// `duskshot --clipboard`  edits the image on the clipboard.
/// `duskshot --reopen`     reopens a kept capture, read from standard input (the tray's
///                         Reopen last capture; see `handoff`).
/// Add `--trace` to either to log frame timings to `duskshot-trace.log` and memory, CPU
/// and GPU use to `duskshot-perf.log`, both in the temp folder (debug builds and builds
/// with the `trace` feature only; see `perf`).
/// `duskshot --trace-cycles 20` opens and closes the capture window 20 times, logging
///                         resources after each, for numbers to compare across versions.
fn main() -> AnyResult<()> {
    harden_dynamic_loader();

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
        run_trace_cycles(n)
    } else if flag("--capture") {
        overlay::run(overlay::Source::Capture)
    } else if flag("--reopen") {
        reopen_kept_capture()
    } else if flag("--settings") {
        settings_window::run()
    } else if flag("--clipboard") {
        overlay::run(overlay::Source::Clipboard)
    } else if flag("--open") || file.is_some() {
        overlay::run(overlay::Source::File(file))
    } else {
        run_default()
    }
}
