//! Freeing Windows-key shortcuts from Windows (`free-shortcut` feature).
//!
//! Windows keeps many Win+letter combinations for itself (Win+Shift+S opens its own
//! snipping tool), so they cannot be registered as an ordinary hotkey. The current
//! user's `DisabledHotkeys` value lists letters whose Win+ shortcuts Explorer should
//! leave alone; with the letter there, Darkshot registers the shortcut like any other.
//! Windows then also keeps the Start menu shut when it is pressed, so Darkshot needs no
//! keyboard hook and never sends a key press of its own.
//!
//! Explorer reads the value when it starts, so a change takes effect after Explorer
//! restarts or the user signs in again. It needs no administrator rights and affects
//! only this user.

use windows_sys::Win32::Foundation::{CloseHandle, ERROR_FILE_NOT_FOUND, ERROR_SUCCESS};
use windows_sys::Win32::System::Registry::{
    RegDeleteKeyValueW, RegGetValueW, RegSetKeyValueW, HKEY_CURRENT_USER, REG_SZ, RRF_RT_REG_SZ,
};
use windows_sys::Win32::System::Threading::{OpenProcess, TerminateProcess, PROCESS_TERMINATE};
use windows_sys::Win32::UI::WindowsAndMessaging::{FindWindowW, GetShellWindow, GetWindowThreadProcessId};

use crate::settings::{key_spec, MOD_WIN};

const KEY: &str = r"Software\Microsoft\Windows\CurrentVersion\Explorer\Advanced";
const VALUE_NAME: &str = "DisabledHotkeys";

fn wide(s: &str) -> Vec<u16> {
    s.encode_utf16().chain(std::iter::once(0)).collect()
}

/// The letter or digit whose Win+ shortcuts would have to be freed for `hotkey`, if it
/// is a Windows-key shortcut on one. `None` for anything else.
pub fn letter_of(hotkey: &str) -> Option<char> {
    let (vk, mods) = key_spec(hotkey)?;
    let c = char::from_u32(vk)?;
    (mods & MOD_WIN != 0 && (c.is_ascii_uppercase() || c.is_ascii_digit())).then_some(c)
}

/// The letters and digits whose Win+ shortcuts Windows currently leaves alone, in
/// upper case. Empty if the value is not set.
pub fn freed() -> String {
    let (key, name) = (wide(KEY), wide(VALUE_NAME));
    let mut buf = vec![0u16; 512];
    let mut bytes = (buf.len() * 2) as u32;
    let result = unsafe {
        RegGetValueW(
            HKEY_CURRENT_USER,
            key.as_ptr(),
            name.as_ptr(),
            RRF_RT_REG_SZ,
            std::ptr::null_mut(),
            buf.as_mut_ptr().cast(),
            &mut bytes,
        )
    };
    if result != ERROR_SUCCESS {
        return String::new();
    }
    let len = (bytes as usize / 2).saturating_sub(1); // without the terminating null
    String::from_utf16_lossy(&buf[..len.min(buf.len())])
}

/// Free the Win+ shortcuts on `c` from Windows (`true`), or give them back (`false`).
/// Other letters in the value are kept; the value is removed once it is empty.
pub fn set(c: char, free: bool) -> std::io::Result<()> {
    let current = freed();
    let mut letters: String = current.chars().filter(|f| !f.eq_ignore_ascii_case(&c)).collect();
    if free {
        letters.push(c.to_ascii_uppercase());
    }
    if letters == current {
        return Ok(());
    }
    let (key, name) = (wide(KEY), wide(VALUE_NAME));
    let result = if letters.is_empty() {
        match unsafe { RegDeleteKeyValueW(HKEY_CURRENT_USER, key.as_ptr(), name.as_ptr()) } {
            ERROR_FILE_NOT_FOUND => ERROR_SUCCESS,
            other => other,
        }
    } else {
        let data = wide(&letters);
        unsafe {
            RegSetKeyValueW(
                HKEY_CURRENT_USER,
                key.as_ptr(),
                name.as_ptr(),
                REG_SZ,
                data.as_ptr().cast(),
                (data.len() * 2) as u32,
            )
        }
    };
    if result == ERROR_SUCCESS { Ok(()) } else { Err(std::io::Error::from_raw_os_error(result as i32)) }
}

/// Restart Explorer (the taskbar and desktop) so it reads the value again. Folder
/// windows in the same process close too. If Windows does not bring Explorer back by
/// itself, it is started again.
pub fn restart_explorer() -> std::io::Result<()> {
    unsafe {
        let shell = GetShellWindow();
        if !shell.is_null() {
            let mut pid = 0;
            GetWindowThreadProcessId(shell, &mut pid);
            let process = OpenProcess(PROCESS_TERMINATE, 0, pid);
            if process.is_null() {
                return Err(std::io::Error::last_os_error());
            }
            let ended = TerminateProcess(process, 1);
            CloseHandle(process);
            if ended == 0 {
                return Err(std::io::Error::last_os_error());
            }
        }
    }
    // Windows usually restarts the shell itself; give it a moment before starting one,
    // so there are never two.
    let taskbar = wide("Shell_TrayWnd");
    for _ in 0..30 {
        std::thread::sleep(std::time::Duration::from_millis(100));
        if !unsafe { FindWindowW(taskbar.as_ptr(), std::ptr::null()) }.is_null() {
            return Ok(());
        }
    }
    let windows = std::env::var_os("SystemRoot").unwrap_or_else(|| r"C:\Windows".into());
    std::process::Command::new(std::path::Path::new(&windows).join("explorer.exe")).spawn()?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn letters_of_shortcuts() {
        assert_eq!(letter_of("Win+Shift+S"), Some('S'));
        assert_eq!(letter_of("win + 5"), Some('5'));
        assert_eq!(letter_of("Ctrl+Shift+S"), None);
        assert_eq!(letter_of("Win+PrintScreen"), None);
        assert_eq!(letter_of("Win+F9"), None);
    }
}
