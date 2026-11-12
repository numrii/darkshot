//! A low-level keyboard hook, for shortcuts that Windows keeps for itself (such as
//! Win+Shift+S) and that therefore cannot be registered as an ordinary hotkey.
//!
//! The hook sees every key press before Windows acts on it. It has two uses:
//! - [`Hook::watch`]: when the capture shortcut is pressed, tell the main thread and
//!   swallow the key, so Windows never sees the combination.
//! - [`Hook::record`]: in the settings window, catch the next combination the user
//!   presses (whatever Windows would otherwise do with it) and swallow it.
//!
//! Nothing in Windows is changed; the hook is gone when dropped or when Darkshot quits.
//! Windows removes hooks that take too long, so the callback only compares a few
//! numbers and posts a message or stores one.

use std::sync::atomic::{AtomicU32, AtomicU8, Ordering};

use windows_sys::Win32::Foundation::{LPARAM, LRESULT, WPARAM};
use windows_sys::Win32::System::LibraryLoader::GetModuleHandleW;
use windows_sys::Win32::UI::Input::KeyboardAndMouse::{
    GetAsyncKeyState, SendInput, INPUT, INPUT_0, INPUT_KEYBOARD, KEYBDINPUT, KEYEVENTF_KEYUP, VK_CONTROL,
    VK_LWIN, VK_MENU, VK_RWIN, VK_SHIFT,
};
use windows_sys::Win32::System::Threading::GetCurrentThreadId;
use windows_sys::Win32::UI::WindowsAndMessaging::{
    CallNextHookEx, DispatchMessageW, GetMessageW, PeekMessageW, PostThreadMessageW, SetWindowsHookExW,
    TranslateMessage, UnhookWindowsHookEx, HC_ACTION, KBDLLHOOKSTRUCT, MSG, PM_NOREMOVE, WH_KEYBOARD_LL,
    WM_KEYDOWN, WM_QUIT, WM_SYSKEYDOWN,
};

use crate::settings::{MOD_ALT, MOD_CTRL, MOD_SHIFT, MOD_WIN};

const MODE_OFF: u8 = 0;
const MODE_WATCH: u8 = 1;
const MODE_RECORD: u8 = 2;

/// The hook's state. Statics, because the hook callback receives no user data.
static MODE: AtomicU8 = AtomicU8::new(MODE_OFF);
static TARGET_VK: AtomicU32 = AtomicU32::new(0);
static TARGET_MODS: AtomicU8 = AtomicU8::new(0);
static NOTIFY_THREAD: AtomicU32 = AtomicU32::new(0);
static NOTIFY_MESSAGE: AtomicU32 = AtomicU32::new(0);
/// The recorded combination: virtual-key code << 8 | modifier bits; 0 = none yet.
static RECORDED: AtomicU32 = AtomicU32::new(0);
/// The key whose press was swallowed, so its release is swallowed too; 0 = none.
static SWALLOWED: AtomicU32 = AtomicU32::new(0);

/// Marks the key events Darkshot sends itself, so the hook lets them through.
const OWN_EVENT: usize = 0xDA5C_0001;
/// An unassigned virtual-key code, used to stop a lone Windows-key release from
/// opening the Start menu.
const VK_MASK: u16 = 0xE8;

/// An installed hook. It runs on its own thread, which does nothing but process
/// messages, so it always answers Windows in time however busy the rest of the app is.
/// Dropping it removes the hook and ends the thread.
pub struct Hook {
    thread_id: u32,
    thread: Option<std::thread::JoinHandle<()>>,
}

impl Hook {
    /// Watch for the key `vk` with exactly `mods` held, and post `message` to the
    /// thread `notify_thread` (which must have a message queue) when it is pressed.
    pub fn watch(vk: u32, mods: u8, notify_thread: u32, message: u32) -> Option<Self> {
        TARGET_VK.store(vk, Ordering::SeqCst);
        TARGET_MODS.store(mods, Ordering::SeqCst);
        NOTIFY_THREAD.store(notify_thread, Ordering::SeqCst);
        NOTIFY_MESSAGE.store(message, Ordering::SeqCst);
        Self::install(MODE_WATCH)
    }

    /// Catch the next key combination pressed anywhere; read it with [`Hook::recorded`].
    pub fn record() -> Option<Self> {
        RECORDED.store(0, Ordering::SeqCst);
        Self::install(MODE_RECORD)
    }

    /// The combination caught by a recording hook, as (virtual-key code, `MOD_*` bits).
    pub fn recorded(&self) -> Option<(u32, u8)> {
        match RECORDED.load(Ordering::SeqCst) {
            0 => None,
            packed => Some((packed >> 8, (packed & 0xFF) as u8)),
        }
    }

    fn install(mode: u8) -> Option<Self> {
        SWALLOWED.store(0, Ordering::SeqCst);
        MODE.store(mode, Ordering::SeqCst);
        let (ready, started) = std::sync::mpsc::channel();
        let thread = std::thread::spawn(move || unsafe {
            // Make sure the thread has a message queue before anyone posts to it.
            let mut msg: MSG = std::mem::zeroed();
            PeekMessageW(&mut msg, std::ptr::null_mut(), 0, 0, PM_NOREMOVE);
            let hook = SetWindowsHookExW(WH_KEYBOARD_LL, Some(callback), GetModuleHandleW(std::ptr::null()), 0);
            let _ = ready.send((!hook.is_null()).then(|| GetCurrentThreadId()));
            if hook.is_null() {
                return;
            }
            while GetMessageW(&mut msg, std::ptr::null_mut(), 0, 0) > 0 {
                TranslateMessage(&msg);
                DispatchMessageW(&msg);
            }
            UnhookWindowsHookEx(hook);
        });
        match started.recv() {
            Ok(Some(thread_id)) => Some(Self { thread_id, thread: Some(thread) }),
            _ => {
                MODE.store(MODE_OFF, Ordering::SeqCst);
                let _ = thread.join();
                None
            }
        }
    }
}

impl Drop for Hook {
    fn drop(&mut self) {
        MODE.store(MODE_OFF, Ordering::SeqCst);
        unsafe { PostThreadMessageW(self.thread_id, WM_QUIT, 0, 0) };
        if let Some(thread) = self.thread.take() {
            let _ = thread.join();
        }
    }
}

fn held(vk: u16) -> bool {
    unsafe { GetAsyncKeyState(i32::from(vk)) < 0 }
}

fn held_mods() -> u8 {
    let mut mods = 0;
    if held(VK_CONTROL) {
        mods |= MOD_CTRL;
    }
    if held(VK_MENU) {
        mods |= MOD_ALT;
    }
    if held(VK_SHIFT) {
        mods |= MOD_SHIFT;
    }
    if held(VK_LWIN) || held(VK_RWIN) {
        mods |= MOD_WIN;
    }
    mods
}

/// Shift, Ctrl, Alt and Windows keys (left, right and generic codes).
fn is_modifier(vk: u32) -> bool {
    matches!(vk, 0x10..=0x12 | 0x5B | 0x5C | 0xA0..=0xA5)
}

/// Press and release an unassigned key. Windows opens the Start menu when the Windows
/// key is released without another key in between; since the shortcut's own key was
/// swallowed, this stands in for it.
fn send_mask_key() {
    let key = |flags| INPUT {
        r#type: INPUT_KEYBOARD,
        Anonymous: INPUT_0 {
            ki: KEYBDINPUT { wVk: VK_MASK, wScan: 0, dwFlags: flags, time: 0, dwExtraInfo: OWN_EVENT },
        },
    };
    let inputs = [key(0), key(KEYEVENTF_KEYUP)];
    unsafe { SendInput(inputs.len() as u32, inputs.as_ptr(), std::mem::size_of::<INPUT>() as i32) };
}

unsafe extern "system" fn callback(code: i32, wparam: WPARAM, lparam: LPARAM) -> LRESULT {
    if code == HC_ACTION as i32 {
        let event = unsafe { &*(lparam as *const KBDLLHOOKSTRUCT) };
        let down = wparam == WM_KEYDOWN as usize || wparam == WM_SYSKEYDOWN as usize;
        if event.dwExtraInfo != OWN_EVENT && !is_modifier(event.vkCode) {
            if !down {
                let swallowed = SWALLOWED.load(Ordering::Relaxed);
                if swallowed == event.vkCode {
                    SWALLOWED.store(0, Ordering::Relaxed);
                    return 1; // the release of a swallowed press
                }
            } else {
                match MODE.load(Ordering::Relaxed) {
                    MODE_WATCH if event.vkCode == TARGET_VK.load(Ordering::Relaxed) => {
                        let mods = TARGET_MODS.load(Ordering::Relaxed);
                        if held_mods() == mods {
                            // Only the first press, not key repeats while held.
                            if SWALLOWED.swap(event.vkCode, Ordering::Relaxed) != event.vkCode {
                                if mods & MOD_WIN != 0 {
                                    send_mask_key();
                                }
                                let thread = NOTIFY_THREAD.load(Ordering::Relaxed);
                                let message = NOTIFY_MESSAGE.load(Ordering::Relaxed);
                                unsafe { PostThreadMessageW(thread, message, 0, 0) };
                            }
                            return 1;
                        }
                    }
                    MODE_RECORD => {
                        let mods = held_mods();
                        RECORDED.store(event.vkCode << 8 | u32::from(mods), Ordering::SeqCst);
                        MODE.store(MODE_OFF, Ordering::Relaxed);
                        SWALLOWED.store(event.vkCode, Ordering::Relaxed);
                        if mods & MOD_WIN != 0 {
                            send_mask_key();
                        }
                        return 1;
                    }
                    _ => {}
                }
            }
        }
    }
    unsafe { CallNextHookEx(std::ptr::null_mut(), code, wparam, lparam) }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Injects real key presses into the system, so it is not run by default:
    /// `cargo test keyhook -- --ignored`.
    #[test]
    #[ignore]
    fn records_an_injected_combination() {
        let hook = Hook::record().expect("hook installs");
        let external = std::env::var_os("DARKSHOT_EXTERNAL_KEYS").is_some();
        std::thread::spawn(move || {
            if external {
                return;
            }
            std::thread::sleep(std::time::Duration::from_millis(200));
            let key = |vk: u16, flags| INPUT {
                r#type: INPUT_KEYBOARD,
                Anonymous: INPUT_0 { ki: KEYBDINPUT { wVk: vk, wScan: 0, dwFlags: flags, time: 0, dwExtraInfo: 0 } },
            };
            // Ctrl+Alt+F8
            let inputs = [key(0x11, 0), key(0x12, 0), key(0x77, 0), key(0x77, KEYEVENTF_KEYUP), key(0x12, KEYEVENTF_KEYUP), key(0x11, KEYEVENTF_KEYUP)];
            unsafe { SendInput(inputs.len() as u32, inputs.as_ptr(), std::mem::size_of::<INPUT>() as i32) };
        });
        // The hook has its own thread: this one can just wait.
        let start = std::time::Instant::now();
        while hook.recorded().is_none() && start.elapsed() < std::time::Duration::from_secs(6) {
            std::thread::sleep(std::time::Duration::from_millis(5));
        }
        assert_eq!(hook.recorded(), Some((0x77, MOD_CTRL | MOD_ALT)));
    }
}
