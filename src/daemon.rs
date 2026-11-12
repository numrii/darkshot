//! The background process: a tray icon plus a global capture hotkey.
//!
//! Each capture runs as a short-lived child process (`darkshot --capture`), so the
//! background process holds no GPU resources and a crash in the overlay cannot take
//! the hotkey down with it.

use std::process::{Child, Command};
use std::sync::{Arc, Mutex};

use global_hotkey::hotkey::{Code, HotKey, Modifiers};
use global_hotkey::{GlobalHotKeyEvent, GlobalHotKeyManager, HotKeyState};
use tray_icon::menu::{Menu, MenuEvent, MenuItem, PredefinedMenuItem};
use tray_icon::{Icon, MouseButton, MouseButtonState, TrayIconBuilder, TrayIconEvent};
use windows_sys::Win32::Foundation::{GetLastError, ERROR_ALREADY_EXISTS};
use windows_sys::Win32::System::Threading::{CreateMutexW, GetCurrentThreadId};
use windows_sys::Win32::UI::WindowsAndMessaging::{
    DispatchMessageW, GetMessageW, PostThreadMessageW, TranslateMessage, MSG, WM_QUIT,
};

use crate::AnyResult;

const MENU_CAPTURE: &str = "capture";
const MENU_QUIT: &str = "quit";

// Temporary fixed hotkey for testing; this will become configurable.
const HOTKEY_MODIFIERS: Modifiers = Modifiers::CONTROL;
const HOTKEY_CODE: Code = Code::F9;
const HOTKEY_LABEL: &str = "Ctrl+F9";

pub fn run() -> AnyResult<()> {
    if !acquire_single_instance() {
        return Ok(()); // already running in the tray
    }

    let overlay: Arc<Mutex<Option<Child>>> = Arc::default();
    let main_thread = unsafe { GetCurrentThreadId() };

    // Global hotkey. Registration fails if another tool already owns the combination.
    let hotkeys = GlobalHotKeyManager::new()?;
    let hotkey_ok = match hotkeys.register(HotKey::new(Some(HOTKEY_MODIFIERS), HOTKEY_CODE)) {
        Ok(()) => true,
        Err(e) => {
            eprintln!("could not register {HOTKEY_LABEL}: {e}");
            false
        }
    };
    {
        let overlay = overlay.clone();
        GlobalHotKeyEvent::set_event_handler(Some(move |e: GlobalHotKeyEvent| {
            if e.state == HotKeyState::Pressed {
                start_capture(&overlay);
            }
        }));
    }

    // Tray icon: left click captures, right click opens the menu.
    let capture_label = if hotkey_ok {
        format!("Capture\t{HOTKEY_LABEL}")
    } else {
        "Capture".to_owned()
    };
    let menu = Menu::with_items(&[
        &MenuItem::with_id(MENU_CAPTURE, capture_label, true, None),
        &PredefinedMenuItem::separator(),
        &MenuItem::with_id(MENU_QUIT, "Quit Darkshot", true, None),
    ])?;
    let tooltip = if hotkey_ok {
        format!("Darkshot: press {HOTKEY_LABEL} to capture")
    } else {
        format!("Darkshot: {HOTKEY_LABEL} is taken by another app, click here to capture")
    };
    let _tray = TrayIconBuilder::new()
        .with_icon(tray_icon()?)
        .with_tooltip(tooltip)
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
            if e.id == MENU_CAPTURE {
                start_capture(&overlay);
            } else if e.id == MENU_QUIT {
                unsafe { PostThreadMessageW(main_thread, WM_QUIT, 0, 0) };
            }
        }));
    }

    if !hotkey_ok {
        rfd::MessageDialog::new()
            .set_level(rfd::MessageLevel::Warning)
            .set_title("Darkshot")
            .set_description(format!(
                "{HOTKEY_LABEL} is already registered by another application, so the hotkey \
                 is unavailable.\n\nYou can still capture by clicking the Darkshot tray icon."
            ))
            .show();
    }

    // The tray icon and the hotkey both deliver their events through this thread's
    // Win32 message queue, so pump it until Quit posts WM_QUIT.
    unsafe {
        let mut msg: MSG = std::mem::zeroed();
        while GetMessageW(&mut msg, std::ptr::null_mut(), 0, 0) > 0 {
            TranslateMessage(&msg);
            DispatchMessageW(&msg);
        }
    }
    Ok(())
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

/// A 32x32 placeholder icon drawn in code: four white selection corners on a dark tile.
fn tray_icon() -> AnyResult<Icon> {
    const SIZE: u32 = 32;
    let near_edge = |v: u32| (7..14).contains(&v) || (18..25).contains(&v);
    let on_line = |v: u32| (7..10).contains(&v) || (22..25).contains(&v);
    let outer = |v: u32| v < 2 || v >= SIZE - 2;

    let mut rgba = Vec::with_capacity((SIZE * SIZE * 4) as usize);
    for y in 0..SIZE {
        for x in 0..SIZE {
            let pixel = if outer(x) && outer(y) {
                [0, 0, 0, 0] // clip the tile's corners
            } else if (on_line(x) && near_edge(y)) || (on_line(y) && near_edge(x)) {
                [255, 255, 255, 255]
            } else {
                [30, 30, 34, 255]
            };
            rgba.extend_from_slice(&pixel);
        }
    }
    Ok(Icon::from_rgba(rgba, SIZE, SIZE)?)
}
