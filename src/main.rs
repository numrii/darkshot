// Darkshot: a privacy-first screenshot tool. No network code, ever:
// a capture can only leave the app via the clipboard or a file on disk.
#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

mod daemon;
mod overlay;

type AnyResult<T> = Result<T, Box<dyn std::error::Error>>;

/// `darkshot`            sits in the tray and waits for the hotkey.
/// `darkshot --capture`  takes one capture right now and exits.
fn main() -> AnyResult<()> {
    if std::env::args().any(|a| a == "--capture") {
        overlay::run()
    } else {
        daemon::run()
    }
}
