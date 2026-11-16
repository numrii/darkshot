//! "Open with" in Explorer (`open-with` feature): Duskshot offered for PNG and JPEG
//! files.
//!
//! Registered for the current user only, under `HKCU\Software\Classes`, so it needs no
//! administrator rights: the app itself (`Applications\duskshot.exe`, with the types it
//! opens) and a file type (`Duskshot.Image`) listed under each extension's
//! `OpenWithProgids`. Nothing becomes the default app; Windows only lists Duskshot.

use windows_sys::Win32::Foundation::{ERROR_FILE_NOT_FOUND, ERROR_SUCCESS};
use windows_sys::Win32::System::Registry::{
    RegDeleteKeyValueW, RegDeleteTreeW, RegGetValueW, RegSetKeyValueW, HKEY_CURRENT_USER, REG_SZ, RRF_RT_REG_SZ,
};
use windows_sys::Win32::UI::Shell::{SHChangeNotify, SHCNE_ASSOCCHANGED, SHCNF_IDLIST};

const APP_KEY: &str = r"Software\Classes\Applications\duskshot.exe";
const PROG_ID: &str = "Duskshot.Image";
const PROG_KEY: &str = r"Software\Classes\Duskshot.Image";
const EXTENSIONS: [&str; 3] = [".png", ".jpg", ".jpeg"];

fn wide(s: &str) -> Vec<u16> {
    s.encode_utf16().chain(std::iter::once(0)).collect()
}

/// The command Explorer runs: this exe with the file.
fn command() -> std::io::Result<String> {
    Ok(format!("\"{}\" \"%1\"", std::env::current_exe()?.display()))
}

fn check(result: u32) -> std::io::Result<()> {
    if result == ERROR_SUCCESS { Ok(()) } else { Err(std::io::Error::from_raw_os_error(result as i32)) }
}

fn set_value(key: &str, name: Option<&str>, value: &str) -> std::io::Result<()> {
    let (key, data) = (wide(key), wide(value));
    let name = name.map(wide);
    let name_ptr = name.as_ref().map_or(std::ptr::null(), |n| n.as_ptr());
    check(unsafe {
        RegSetKeyValueW(HKEY_CURRENT_USER, key.as_ptr(), name_ptr, REG_SZ, data.as_ptr().cast(), (data.len() * 2) as u32)
    })
}

fn get_value(key: &str) -> Option<String> {
    let key = wide(key);
    let mut buf = vec![0u16; 1024];
    let mut bytes = (buf.len() * 2) as u32;
    let result = unsafe {
        RegGetValueW(
            HKEY_CURRENT_USER,
            key.as_ptr(),
            std::ptr::null(),
            RRF_RT_REG_SZ,
            std::ptr::null_mut(),
            buf.as_mut_ptr().cast(),
            &mut bytes,
        )
    };
    if result != ERROR_SUCCESS {
        return None;
    }
    let len = (bytes as usize / 2).saturating_sub(1);
    Some(String::from_utf16_lossy(&buf[..len.min(buf.len())]))
}

/// Whether Explorer offers this exe under Open with.
pub fn is_on() -> bool {
    let own = command().ok();
    own.is_some() && get_value(&format!(r"{PROG_KEY}\shell\open\command")).as_deref() == own.as_deref()
}

/// Offer this exe under Open with for PNG and JPEG files (`true`), or remove it.
pub fn set(enabled: bool) -> std::io::Result<()> {
    if enabled {
        let command = command()?;
        set_value(APP_KEY, Some("FriendlyAppName"), "Duskshot")?;
        set_value(&format!(r"{APP_KEY}\shell\open\command"), None, &command)?;
        set_value(PROG_KEY, None, "Image (Duskshot)")?;
        set_value(&format!(r"{PROG_KEY}\shell\open\command"), None, &command)?;
        for ext in EXTENSIONS {
            set_value(&format!(r"{APP_KEY}\SupportedTypes"), Some(ext), "")?;
            set_value(&format!(r"Software\Classes\{ext}\OpenWithProgids"), Some(PROG_ID), "")?;
        }
    } else {
        for key in [APP_KEY, PROG_KEY] {
            let key = wide(key);
            match unsafe { RegDeleteTreeW(HKEY_CURRENT_USER, key.as_ptr()) } {
                ERROR_FILE_NOT_FOUND => {}
                other => check(other)?,
            }
            let _ = unsafe { windows_sys::Win32::System::Registry::RegDeleteKeyW(HKEY_CURRENT_USER, key.as_ptr()) };
        }
        for ext in EXTENSIONS {
            let (key, name) = (wide(&format!(r"Software\Classes\{ext}\OpenWithProgids")), wide(PROG_ID));
            match unsafe { RegDeleteKeyValueW(HKEY_CURRENT_USER, key.as_ptr(), name.as_ptr()) } {
                ERROR_FILE_NOT_FOUND => {}
                other => check(other)?,
            }
        }
    }
    // Tell Explorer the associations changed.
    unsafe { SHChangeNotify(SHCNE_ASSOCCHANGED as i32, SHCNF_IDLIST, std::ptr::null(), std::ptr::null()) };
    Ok(())
}
