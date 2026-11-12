//! The background process: a tray icon plus a global capture hotkey.
//!
//! Each capture runs as a short-lived child process (`darkshot --capture`), so the
//! background process holds no GPU resources and a crash in the overlay cannot take
//! the hotkey down with it. The settings window is a child process too
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

use crate::keyhook::Hook;
use crate::settings::{hook_spec, parse_hotkey, Settings};
use crate::AnyResult;

const MENU_CAPTURE: &str = "capture";
const MENU_SETTINGS: &str = "settings";
const MENU_QUIT: &str = "quit";

/// Thread messages to the main thread, which owns the hotkey and the tray.
const WM_OPEN_SETTINGS: u32 = WM_APP + 1;
const WM_SETTINGS_CLOSED: u32 = WM_APP + 2;
/// Posted by the keyboard hook when a taken-over shortcut is pressed.
const WM_HOOK_CAPTURE: u32 = WM_APP + 3;

/// How the capture shortcut is currently being caught.
enum Shortcut {
    /// Registered with Windows as an ordinary hotkey.
    Hotkey(HotKey),
    /// Caught by a low-level keyboard hook (taking it over from Windows).
    Hook(Hook),
}

fn release(hotkeys: &GlobalHotKeyManager, active: &mut Option<Shortcut>) {
    match active.take() {
        Some(Shortcut::Hotkey(hotkey)) => {
            let _ = hotkeys.unregister(hotkey);
        }
        Some(Shortcut::Hook(hook)) => drop(hook),
        None => {}
    }
}

pub fn run() -> AnyResult<()> {
    if !acquire_single_instance() {
        return Ok(()); // already running in the tray
    }

    let overlay: Arc<Mutex<Option<Child>>> = Arc::default();
    let main_thread = unsafe { GetCurrentThreadId() };

    let hotkeys = GlobalHotKeyManager::new()?;
    {
        let overlay = overlay.clone();
        GlobalHotKeyEvent::set_event_handler(Some(move |e: GlobalHotKeyEvent| {
            if e.state == HotKeyState::Pressed {
                start_capture(&overlay);
            }
        }));
    }

    // Tray icon: left click captures, right click opens the menu.
    let capture_item = MenuItem::with_id(MENU_CAPTURE, "Capture", true, None);
    let menu = Menu::with_items(&[
        &capture_item,
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
            } else if e.id == MENU_SETTINGS {
                post(WM_OPEN_SETTINGS);
            } else if e.id == MENU_QUIT {
                post(WM_QUIT);
            }
        }));
    }

    let mut active = None;
    apply_hotkey(&hotkeys, &mut active, &capture_item, &tray, main_thread);
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
                        release(&hotkeys, &mut active);
                        settings_open = open_settings(main_thread);
                        if !settings_open {
                            apply_hotkey(&hotkeys, &mut active, &capture_item, &tray, main_thread);
                        }
                    }
                }
                WM_SETTINGS_CLOSED if msg.hwnd.is_null() => {
                    settings_open = false;
                    apply_hotkey(&hotkeys, &mut active, &capture_item, &tray, main_thread);
                }
                WM_HOOK_CAPTURE if msg.hwnd.is_null() => start_capture(&overlay),
                _ => {
                    TranslateMessage(&msg);
                    DispatchMessageW(&msg);
                }
            }
        }
    }
    Ok(())
}

/// Read the shortcut from the settings and start catching it, replacing whatever was
/// caught before: as an ordinary hotkey, or with the keyboard hook if the settings say
/// to take it over. Updates the menu and tooltip, and explains if it is not available.
fn apply_hotkey(
    hotkeys: &GlobalHotKeyManager,
    active: &mut Option<Shortcut>,
    capture_item: &MenuItem,
    tray: &TrayIcon,
    main_thread: u32,
) {
    release(hotkeys, active);
    let settings = Settings::load();
    let label = settings.hotkey.trim().to_owned();
    let result = if settings.take_over {
        match hook_spec(&label) {
            Some((vk, mods)) => Hook::watch(vk, mods, main_thread, WM_HOOK_CAPTURE)
                .map(Shortcut::Hook)
                .ok_or_else(|| "the keyboard hook could not be installed".to_owned()),
            None => Err("not a valid key combination".to_owned()),
        }
    } else {
        match parse_hotkey(&label) {
            Some(hotkey) => hotkeys
                .register(hotkey)
                .map(|()| Shortcut::Hotkey(hotkey))
                .map_err(|e| e.to_string()),
            None => Err("not a valid key combination".to_owned()),
        }
    };
    match result {
        Ok(shortcut) => {
            *active = Some(shortcut);
            capture_item.set_text(format!("Capture\t{label}"));
            let _ = tray.set_tooltip(Some(format!("Darkshot: press {label} to capture")));
        }
        Err(e) => {
            eprintln!("could not register {label}: {e}");
            capture_item.set_text("Capture");
            let _ = tray.set_tooltip(Some(format!("Darkshot: {label} is unavailable, click here to capture")));
            rfd::MessageDialog::new()
                .set_level(rfd::MessageLevel::Warning)
                .set_title("Darkshot")
                .set_description(format!(
                    "The capture shortcut {label} could not be registered. Windows or another \
                     application may already use it.\n\nYou can still capture by clicking the \
                     Darkshot tray icon. Under Settings you can choose another shortcut, or let \
                     Darkshot take this one over."
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

/// Launch the overlay as a child process, unless one is already on screen.
fn start_capture(overlay: &Mutex<Option<Child>>) {
    let mut slot = overlay.lock().unwrap();
    if let Some(child) = slot.as_mut() {
        if matches!(child.try_wait(), Ok(None)) {
            return;
        }
    }
    let spawned = std::env::current_exe()
        .and_then(|exe| {
            // Pass our own flags (such as `--trace`) on to the overlay.
            Command::new(exe)
                .arg("--capture")
                .args(std::env::args().skip(1).filter(|a| a != "--capture"))
                .spawn()
        });
    match spawned {
        Ok(child) => *slot = Some(child),
        Err(e) => {
            eprintln!("could not start capture: {e}");
            *slot = None;
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
