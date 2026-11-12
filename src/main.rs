// Darkshot: a privacy-first screenshot tool. No network code, ever:
// a capture can only leave the app via the clipboard or a file on disk.
#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

mod annotate;
mod daemon;
mod effects;
mod icons;
mod keyhook;
mod overlay;
mod save;
mod settings;
mod settings_window;
mod snap;
mod text;

type AnyResult<T> = Result<T, Box<dyn std::error::Error>>;

/// `darkshot`            sits in the tray and waits for the hotkey.
/// `darkshot --capture`  takes one capture right now and exits.
/// `darkshot --settings` opens the settings window.
/// Add `--trace` to either to log frame timings to `darkshot-trace.log` in the temp folder.
fn main() -> AnyResult<()> {
    if std::env::args().any(|a| a == "--capture") {
        overlay::run()
    } else if std::env::args().any(|a| a == "--settings") {
        settings_window::run()
    } else {
        daemon::run()
    }
}
