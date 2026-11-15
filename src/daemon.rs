//! The background process: a tray icon plus a global capture hotkey.
//!
//! Each capture runs as a short-lived child process (`darkshot --capture`), so the
//! background process holds no GPU resources and a crash in the overlay cannot take
//! the hotkey down with it. With Reopen last capture on, a closing capture hands its
//! state to this process, which keeps it in memory and passes it to the next overlay
//! (see `handoff`). The settings window is a child process too
//! (`darkshot --settings`); when it closes, the settings are read again.

use std::process::{Child, Command};
use std::sync::{Arc, Mutex};

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
        &MenuItem::with_id(MENU_QUIT, "Quit Darkshot", true, None),
    ])?;
    let tray = TrayIconBuilder::new()
        .with_icon(tray_icon()?)
        .with_tooltip("Darkshot")
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
                    .set_title("Darkshot")
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
            let _ = tray.set_tooltip(Some(format!("Darkshot: press {shown} to capture")));
        }
        Err(e) => {
            eprintln!("could not register {label}: {e}");
            capture_item.set_text("Capture");
            let _ = tray.set_tooltip(Some(format!("Darkshot: {shown} is unavailable, click here to capture")));
            rfd::MessageDialog::new()
                .set_level(rfd::MessageLevel::Warning)
                .set_title("Darkshot")
                .set_description(format!(
                    "The capture shortcut {shown} could not be registered. Windows or another \
                     application may already use it.\n\nYou can still capture by clicking the \
                     Darkshot tray icon. Under Settings you can choose another shortcut{}.",
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
    /// Counts changes to `kept`, so a timer only drops the capture it was set for.
    generation: u64,
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
        c.generation += 1;
        c.kept.take()
    };
    if let Some(bytes) = bytes {
        start_overlay(captures, "--reopen", Some(bytes));
    }
}

/// Keep a capture the overlay handed over as it closed, for the minutes set in the
/// settings, then let it go.
fn keep(captures: &Shared, mut bytes: Vec<u8>) {
    let minutes = Settings::load().keep_minutes;
    if minutes == 0 {
        return;
    }
    bytes.shrink_to_fit();
    let generation = {
        let mut c = captures.lock().unwrap();
        c.kept = Some(bytes);
        c.generation += 1;
        c.generation
    };
    let captures = captures.clone();
    std::thread::spawn(move || {
        std::thread::sleep(std::time::Duration::from_secs(u64::from(minutes) * 60));
        let mut c = captures.lock().unwrap();
        if c.generation == generation {
            c.kept = None;
        }
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
        c.generation += 1;
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
                    if output.read_to_end(&mut bytes).is_ok() && !bytes.is_empty() {
                        keep(&captures, bytes);
                    }
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

/// Returns false if another Darkshot background process already holds the mutex.
fn acquire_single_instance() -> bool {
    let name: Vec<u16> = "Local\\DarkshotSingleInstance\0".encode_utf16().collect();
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
