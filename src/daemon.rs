//! The background process: a tray icon plus a global capture hotkey.
//!
//! Each capture runs as a short-lived child process (`duskshot --capture`), so the
//! background process holds no GPU resources and a crash in the overlay cannot take
//! the hotkey down with it. With Reopen last capture on, a closing capture hands its
//! state to this process, which keeps it in memory and passes it to the next overlay
//! (see `handoff`). The settings window is a child process too
//! (`duskshot --settings`); when it closes, the settings are read again.

use std::process::{Child, Command};
use std::sync::{Arc, Condvar, Mutex};
use std::time::{Duration, Instant};

use global_hotkey::hotkey::HotKey;
use global_hotkey::{GlobalHotKeyEvent, GlobalHotKeyManager, HotKeyState};
use tray_icon::menu::{Menu, MenuEvent, MenuItem, PredefinedMenuItem};
use tray_icon::{Icon, MouseButton, MouseButtonState, TrayIcon, TrayIconBuilder, TrayIconEvent};
use windows_sys::Win32::Foundation::{GetLastError, ERROR_ALREADY_EXISTS};
use windows_sys::Win32::System::Threading::{CreateMutexW, GetCurrentThreadId};
use windows_sys::Win32::UI::WindowsAndMessaging::{
    DispatchMessageW, GetMessageW, PostThreadMessageW, TranslateMessage, MSG, WM_APP, WM_QUIT,
};

use crate::settings::{parse_hotkey, Settings};
use crate::AnyResult;

const MENU_CAPTURE: &str = "capture";
const MENU_SETTINGS: &str = "settings";
const MENU_OPEN: &str = "open";
const MENU_CLIPBOARD: &str = "clipboard";
const MENU_REOPEN: &str = "reopen";
const MENU_QUIT: &str = "quit";

/// Thread messages to the main thread, which owns the hotkey and the tray.
const WM_OPEN_SETTINGS: u32 = WM_APP + 1;
const WM_SETTINGS_CLOSED: u32 = WM_APP + 2;
/// What a registered shortcut does.
#[derive(Clone, Copy, PartialEq)]
enum Shortcut {
    Capture,
    Clipboard,
    Reopen,
}

/// The registered shortcuts, by hotkey id.
type Active = Arc<Mutex<Vec<(HotKey, Shortcut)>>>;

/// Let go of every registered shortcut.
fn release(hotkeys: &GlobalHotKeyManager, active: &Active) {
    for (hotkey, _) in active.lock().unwrap().drain(..) {
        let _ = hotkeys.unregister(hotkey);
    }
}

/// The menu entries whose text shows a shortcut.
struct Items {
    capture: MenuItem,
    clipboard: MenuItem,
    reopen: MenuItem,
}

pub fn run() -> AnyResult<()> {
    if !acquire_single_instance() {
        return Ok(()); // already running in the tray
    }

    let overlay: Shared = Arc::default();
    let main_thread = unsafe { GetCurrentThreadId() };

    let hotkeys = GlobalHotKeyManager::new()?;
    let active: Active = Arc::default();
    {
        let (overlay, active) = (overlay.clone(), active.clone());
        GlobalHotKeyEvent::set_event_handler(Some(move |e: GlobalHotKeyEvent| {
            if e.state != HotKeyState::Pressed {
                return;
            }
            let which = active.lock().unwrap().iter().find(|(h, _)| h.id() == e.id).map(|(_, s)| *s);
            match which {
                Some(Shortcut::Capture) => start_capture(&overlay),
                Some(Shortcut::Clipboard) => start_overlay(&overlay, "--clipboard", None),
                Some(Shortcut::Reopen) => reopen(&overlay),
                None => {}
            }
        }));
    }

    // Tray icon: left click captures, right click opens the menu.
    let items = Items {
        capture: MenuItem::with_id(MENU_CAPTURE, "Capture", true, None),
        clipboard: MenuItem::with_id(MENU_CLIPBOARD, "Edit clipboard image", true, None),
        reopen: MenuItem::with_id(MENU_REOPEN, "Reopen last capture", false, None),
    };
    let menu = Menu::with_items(&[
        &items.capture,
        &items.reopen,
        &MenuItem::with_id(MENU_OPEN, "Open image…", true, None),
        &items.clipboard,
        &PredefinedMenuItem::separator(),
        &MenuItem::with_id(MENU_SETTINGS, "Settings…", true, None),
        &PredefinedMenuItem::separator(),
        &MenuItem::with_id(MENU_QUIT, "Quit Duskshot", true, None),
    ])?;
    let tray = TrayIconBuilder::new()
        .with_icon(tray_icon()?)
        .with_tooltip("Duskshot")
        .with_menu(Box::new(menu))
        .with_menu_on_left_click(false)
        .build()?;
    {
        let overlay = overlay.clone();
        TrayIconEvent::set_event_handler(Some(move |e: TrayIconEvent| {
            if let TrayIconEvent::Click {
                button: MouseButton::Left,
                button_state: MouseButtonState::Up,
                ..
            } = e
            {
                start_capture(&overlay);
            }
        }));
    }
    {
        let overlay = overlay.clone();
        MenuEvent::set_event_handler(Some(move |e: MenuEvent| {
            let post = |message| unsafe { PostThreadMessageW(main_thread, message, 0, 0) };
            if e.id == MENU_CAPTURE {
                start_capture(&overlay);
            } else if e.id == MENU_OPEN {
                start_overlay(&overlay, "--open", None);
            } else if e.id == MENU_CLIPBOARD {
                start_overlay(&overlay, "--clipboard", None);
            } else if e.id == MENU_REOPEN {
                reopen(&overlay);
            } else if e.id == MENU_SETTINGS {
                post(WM_OPEN_SETTINGS);
            } else if e.id == MENU_QUIT {
                post(WM_QUIT);
            }
        }));
    }

    apply_hotkey(&hotkeys, &active, &items, &tray);
    let mut settings_open = false;
    crate::perf::sample("ready");
    if crate::perf::enabled() {
        std::thread::spawn(|| loop {
            std::thread::sleep(crate::perf::IDLE_EVERY);
            crate::perf::sample("idle");
        });
    }

    // The tray icon and the hotkey both deliver their events through this thread's
    // Win32 message queue, so pump it until Quit posts WM_QUIT.
    unsafe {
        let mut msg: MSG = std::mem::zeroed();
        while GetMessageW(&mut msg, std::ptr::null_mut(), 0, 0) > 0 {
            match msg.message {
                WM_OPEN_SETTINGS if msg.hwnd.is_null() => {
                    if !settings_open {
                        // Let go of the shortcut while the settings are open, so pressing
                        // the current combination to record it does not start a capture.
                        release(&hotkeys, &active);
                        settings_open = open_settings(main_thread);
                        if !settings_open {
                            apply_hotkey(&hotkeys, &active, &items, &tray);
                        }
                    }
                }
                WM_SETTINGS_CLOSED if msg.hwnd.is_null() => {
                    settings_open = false;
                    apply_hotkey(&hotkeys, &active, &items, &tray);
                    crate::perf::sample("settings closed");
                    // Reopen last capture switched off: let go of a kept capture now,
                    // rather than when its time would have run out.
                    if Settings::load().keep_minutes == 0 {
                        overlay.lock().unwrap().kept = None;
                    }
                }
                _ => {
                    TranslateMessage(&msg);
                    DispatchMessageW(&msg);
                }
            }
        }
    }
    Ok(())
}

/// Read the shortcuts from the settings and register them, replacing whatever was
/// registered before. Updates the menu and tooltip, and explains if one is not available.
fn apply_hotkey(hotkeys: &GlobalHotKeyManager, active: &Active, items: &Items, tray: &TrayIcon) {
    release(hotkeys, active);
    let settings = Settings::load();
    items.reopen.set_enabled(settings.keep_minutes > 0);
    for (text, which, item, name) in [
        (&settings.clipboard_hotkey, Shortcut::Clipboard, &items.clipboard, "Edit clipboard image"),
        (&settings.reopen_hotkey, Shortcut::Reopen, &items.reopen, "Reopen last capture"),
    ] {
        let label = text.trim();
        let shown = crate::keys::shortcut_label(label);
        item.set_text(name);
        if label.is_empty() || which == Shortcut::Reopen && settings.keep_minutes == 0 {
            continue;
        }
        match parse_hotkey(label).ok_or_else(|| "not a valid key combination".to_owned()).and_then(|h| {
            hotkeys.register(h).map(|()| h).map_err(|e| e.to_string())
        }) {
            Ok(hotkey) => {
                active.lock().unwrap().push((hotkey, which));
                item.set_text(format!("{name}\t{shown}"));
            }
            Err(e) => {
                eprintln!("could not register {label}: {e}");
                rfd::MessageDialog::new()
                    .set_level(rfd::MessageLevel::Warning)
                    .set_title("Duskshot")
                    .set_description(format!(
                        "The {name} shortcut {shown} could not be registered. Windows or another \
                         application may already use it. You can choose another under Settings."
                    ))
                    .show();
            }
        }
    }
    let capture_item = &items.capture;
    let label = settings.hotkey.trim().to_owned();
    let shown = crate::keys::shortcut_label(&label);
    let result = match parse_hotkey(&label) {
        Some(hotkey) => hotkeys.register(hotkey).map(|()| hotkey).map_err(|e| e.to_string()),
        None => Err("not a valid key combination".to_owned()),
    };
    match result {
        Ok(shortcut) => {
            active.lock().unwrap().push((shortcut, Shortcut::Capture));
            capture_item.set_text(format!("Capture\t{shown}"));
            let _ = tray.set_tooltip(Some(format!("Duskshot: press {shown} to capture")));
        }
        Err(e) => {
            eprintln!("could not register {label}: {e}");
            capture_item.set_text("Capture");
            let _ = tray.set_tooltip(Some(format!("Duskshot: {shown} is unavailable, click here to capture")));
            rfd::MessageDialog::new()
                .set_level(rfd::MessageLevel::Warning)
                .set_title("Duskshot")
                .set_description(format!(
                    "The capture shortcut {shown} could not be registered. Windows or another \
                     application may already use it.\n\nYou can still capture by clicking the \
                     Duskshot tray icon. Under Settings you can choose another shortcut{}.",
                    if crate::settings::FREE_SHORTCUT_AVAILABLE {
                        ", or free a Windows-key shortcut from Windows"
                    } else {
                        ""
                    }
                ))
                .show();
        }
    }
}

/// Start the settings window as a child process. When it exits, the main thread is
/// told to read the settings again. Returns false if it could not be started.
fn open_settings(main_thread: u32) -> bool {
    let child = std::env::current_exe().and_then(|exe| Command::new(exe).arg("--settings").spawn());
    match child {
        Ok(mut child) => {
            std::thread::spawn(move || {
                let _ = child.wait();
                unsafe { PostThreadMessageW(main_thread, WM_SETTINGS_CLOSED, 0, 0) };
            });
            true
        }
        Err(e) => {
            eprintln!("could not open settings: {e}");
            false
        }
    }
}

/// The capture window, if one is open, and the last capture kept for Reopen last
/// capture (see `handoff`): only its bytes, in memory, never on disk.
#[derive(Default)]
struct Captures {
    child: Option<Child>,
    kept: Option<Vec<u8>>,
    /// When `kept` is let go.
    expires: Option<Instant>,
    /// Whether the thread that lets `kept` go (see `keep_for`) is running.
    timer: bool,
    /// Wakes that thread when `kept` changes.
    kept_changed: Arc<Condvar>,
}

impl Captures {
    /// The process id of the capture window, if one is open.
    fn running(&mut self) -> Option<u32> {
        let child = self.child.as_mut()?;
        matches!(child.try_wait(), Ok(None)).then(|| child.id())
    }
}

type Shared = Arc<Mutex<Captures>>;

/// Launch the overlay to capture the screen, unless one is already on screen.
fn start_capture(captures: &Shared) {
    start_overlay(captures, "--capture", None);
}

/// Bring back the last capture, if one is kept (Reopen last capture).
fn reopen(captures: &Shared) {
    let bytes = {
        let mut c = captures.lock().unwrap();
        if let Some(pid) = c.running() {
            bring_forward(pid);
            return;
        }
        c.kept.take()
    };
    if let Some(bytes) = bytes {
        start_overlay(captures, "--reopen", Some(bytes));
    }
}

/// Keep a capture the overlay handed over as it closed, for the minutes set in the
/// settings, then let it go.
fn keep(captures: &Shared, bytes: Vec<u8>) {
    let minutes = Settings::load().keep_minutes;
    if minutes == 0 {
        return;
    }
    keep_for(captures, bytes, Duration::from_secs(u64::from(minutes) * 60));
}

/// Keep `bytes` for `time`, replacing any capture kept before.
fn keep_for(captures: &Shared, mut bytes: Vec<u8>, time: Duration) {
    bytes.shrink_to_fit();
    let mut c = captures.lock().unwrap();
    c.kept = Some(bytes);
    c.expires = Some(Instant::now() + time);
    // One thread waits for whichever capture is kept now, however many come and go:
    // woken here when a new one replaces it, gone once nothing is kept.
    if c.timer {
        c.kept_changed.notify_all();
        return;
    }
    c.timer = true;
    drop(c);
    let captures = captures.clone();
    std::thread::spawn(move || {
        let mut c = captures.lock().unwrap();
        while let (true, Some(expires)) = (c.kept.is_some(), c.expires) {
            let left = expires.saturating_duration_since(Instant::now());
            if left.is_zero() {
                // Still holding the lock, so no new capture can slip in unwatched
                // between letting this one go and the thread ending.
                c.kept = None;
                crate::perf::sample("kept let go");
                continue;
            }
            let kept_changed = c.kept_changed.clone();
            c = kept_changed.wait_timeout(c, left).unwrap().0;
        }
        c.timer = false;
    });
}


/// Launch the overlay as a child process with `mode` (`--capture`, `--open`,
/// `--clipboard`, or `--reopen` with the kept capture's `bytes`), unless one is already
/// on screen: that one is brought to the front. A new capture replaces a kept one.
fn start_overlay(captures: &Shared, mode: &str, bytes: Option<Vec<u8>>) {
    use std::io::{Read, Write};
    use std::process::Stdio;
    let mut c = captures.lock().unwrap();
    if let Some(pid) = c.running() {
        // Already on screen: bring it to the front, in case a second click on the tray
        // icon took the keyboard from it.
        bring_forward(pid);
        return;
    }
    if bytes.is_none() {
        c.kept = None;
    }
    let spawned = std::env::current_exe().and_then(|exe| {
        // Pass our own flags (such as `--trace`) on to the overlay.
        Command::new(exe)
            .arg(mode)
            .arg("--tray")
            .args(std::env::args().skip(1).filter(|a| {
                a.starts_with("--")
                    && !["--capture", "--open", "--clipboard", "--reopen", "--tray"].contains(&a.as_str())
            }))
            .args(crate::perf::enabled().then_some("--trace-autoclose").filter(|_| {
                std::env::args().any(|a| a == "--trace-cycles")
            }))
            .stdin(if bytes.is_some() { Stdio::piped() } else { Stdio::null() })
            .stdout(Stdio::piped())
            .spawn()
    });
    match spawned {
        Ok(mut child) => {
            // Let it take the foreground even if the taskbar gets it first.
            unsafe { windows_sys::Win32::UI::WindowsAndMessaging::AllowSetForegroundWindow(child.id()) };
            if let (Some(bytes), Some(mut input)) = (bytes, child.stdin.take()) {
                std::thread::spawn(move || {
                    let _ = input.write_all(&bytes);
                });
            }
            // What it hands over as it closes, if anything, is the capture to keep.
            if let Some(mut output) = child.stdout.take() {
                let captures = captures.clone();
                std::thread::spawn(move || {
                    let mut bytes = Vec::new();
                    let kept = output.read_to_end(&mut bytes).is_ok() && !bytes.is_empty();
                    if kept {
                        keep(&captures, bytes);
                    }
                    // Its output ends when it closes.
                    crate::perf::sample(if kept { "overlay closed kept" } else { "overlay closed" });
                });
            }
            c.child = Some(child);
        }
        Err(e) => {
            eprintln!("could not start capture: {e}");
            c.child = None;
        }
    }
}

/// `--trace-cycles N`: open the capture window `n` times the way the tray does, each one
/// closing itself once it is on screen (and handing over its capture, if Reopen last
/// capture is on), and log this process's resources after each. Growth in the tray
/// rows from cycle to cycle is a leak in the tray; the capture windows' own rows give
/// startup time and peak memory to compare across versions. Takes neither the hotkey
/// nor the tray icon, so it runs beside a Duskshot already in the tray.
pub fn cycles(n: u32) -> AnyResult<()> {
    let captures: Shared = Arc::default();
    crate::perf::sample("cycles start");
    for i in 1..=n {
        start_overlay(&captures, "--capture", None);
        let started = Instant::now();
        while captures.lock().unwrap().running().is_some() {
            if started.elapsed() > Duration::from_secs(30) {
                return Err(format!("cycle {i}: the capture window did not close by itself").into());
            }
            std::thread::sleep(Duration::from_millis(50));
        }
        // Let the handed-over capture arrive and settle before measuring.
        std::thread::sleep(Duration::from_millis(500));
        crate::perf::sample(&format!("cycle {i}"));
    }
    captures.lock().unwrap().kept = None;
    crate::perf::sample("cycles done");
    Ok(())
}

/// Give the keyboard to the visible window of process `pid`, if it has one.
fn bring_forward(pid: u32) {
    use windows_sys::Win32::Foundation::{HWND, LPARAM};
    use windows_sys::Win32::UI::WindowsAndMessaging::{
        AllowSetForegroundWindow, EnumWindows, GetWindowThreadProcessId, IsWindowVisible, SetForegroundWindow,
    };
    unsafe extern "system" fn find(hwnd: HWND, lparam: LPARAM) -> i32 {
        // SAFETY: `lparam` points at the (pid, found) pair below, alive for the call.
        let wanted = unsafe { &mut *(lparam as *mut (u32, HWND)) };
        let mut pid = 0;
        unsafe { GetWindowThreadProcessId(hwnd, &mut pid) };
        if pid == wanted.0 && unsafe { IsWindowVisible(hwnd) } != 0 {
            wanted.1 = hwnd;
            return 0; // found: stop
        }
        1
    }
    let mut wanted: (u32, HWND) = (pid, std::ptr::null_mut());
    unsafe {
        AllowSetForegroundWindow(pid);
        EnumWindows(Some(find), std::ptr::from_mut(&mut wanted) as LPARAM);
        if !wanted.1.is_null() {
            SetForegroundWindow(wanted.1);
        }
    }
}

/// Returns false if another Duskshot background process already holds the mutex.
fn acquire_single_instance() -> bool {
    let name: Vec<u16> = "Local\\DuskshotSingleInstance\0".encode_utf16().collect();
    // The handle is deliberately never closed: it must live as long as the process.
    unsafe {
        let handle = CreateMutexW(std::ptr::null(), 0, name.as_ptr());
        !handle.is_null() && GetLastError() != ERROR_ALREADY_EXISTS
    }
}

/// The tray icon: the app icon at 32x32.
fn tray_icon() -> AnyResult<Icon> {
    Ok(Icon::from_rgba(crate::appicon::rgba(32), 32, 32)?)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn wait(ms: u64) {
        std::thread::sleep(Duration::from_millis(ms));
    }

    #[test]
    fn a_kept_capture_is_let_go_when_its_time_is_up() {
        let captures: Shared = Arc::default();
        keep_for(&captures, vec![1], Duration::from_millis(100));
        wait(50);
        assert_eq!(captures.lock().unwrap().kept, Some(vec![1]));
        wait(150);
        let c = captures.lock().unwrap();
        assert_eq!(c.kept, None);
        assert!(!c.timer, "the timer thread ends once nothing is kept");
    }

    #[test]
    fn a_newer_capture_gets_its_own_time_and_one_thread_watches_both() {
        let captures: Shared = Arc::default();
        keep_for(&captures, vec![1], Duration::from_millis(100));
        wait(60);
        keep_for(&captures, vec![2], Duration::from_millis(100));
        wait(80); // past the first one's time, within the second's
        assert_eq!(captures.lock().unwrap().kept, Some(vec![2]));
        wait(100);
        assert_eq!(captures.lock().unwrap().kept, None);
    }

    #[test]
    fn a_shorter_time_wakes_the_timer_early() {
        let captures: Shared = Arc::default();
        keep_for(&captures, vec![1], Duration::from_secs(60));
        wait(20);
        keep_for(&captures, vec![2], Duration::from_millis(50));
        wait(150);
        assert_eq!(captures.lock().unwrap().kept, None);
    }

    #[test]
    fn a_capture_kept_after_one_was_taken_is_still_let_go() {
        let captures: Shared = Arc::default();
        keep_for(&captures, vec![1], Duration::from_millis(80));
        captures.lock().unwrap().kept = None; // reopened, or replaced by a new capture
        wait(20);
        keep_for(&captures, vec![2], Duration::from_millis(80));
        wait(150);
        let c = captures.lock().unwrap();
        assert_eq!(c.kept, None);
        assert!(!c.timer);
    }
}
