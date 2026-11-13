//! Starting Darkshot when you sign in to Windows (`autostart` feature).
//!
//! Uses the current user's `Run` key, so it needs no administrator rights and affects
//! only this user. The entry holds the exe's full path; if the exe is moved, the old
//! entry points nowhere, which [`status`] reports so the settings window can offer to
//! fix it.

use std::path::Path;

use windows_sys::Win32::Foundation::{ERROR_FILE_NOT_FOUND, ERROR_SUCCESS};
use windows_sys::Win32::System::Registry::{
    RegDeleteKeyValueW, RegGetValueW, RegSetKeyValueW, HKEY_CURRENT_USER, REG_SZ, RRF_RT_REG_SZ,
};

const RUN_KEY: &str = r"Software\Microsoft\Windows\CurrentVersion\Run";
const VALUE_NAME: &str = "Darkshot";

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Status {
    Off,
    /// On, starting this exe.
    On,
    /// On, but starting a Darkshot at another path.
    Elsewhere(String),
}

fn wide(s: &str) -> Vec<u16> {
    s.encode_utf16().chain(std::iter::once(0)).collect()
}

/// The command the entry should hold: this exe's path, quoted.
fn command() -> std::io::Result<String> {
    Ok(quote(&std::env::current_exe()?))
}

fn quote(path: &Path) -> String {
    format!("\"{}\"", path.display())
}

/// Whether Darkshot starts with Windows, and if so whether it is this exe.
pub fn status() -> Status {
    let Some(current) = read() else { return Status::Off };
    match command() {
        Ok(own) if own.eq_ignore_ascii_case(&current) => Status::On,
        _ => Status::Elsewhere(current.trim_matches('"').to_owned()),
    }
}

fn read() -> Option<String> {
    let (key, name) = (wide(RUN_KEY), wide(VALUE_NAME));
    let mut buf = vec![0u16; 1024];
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
        return None;
    }
    let len = (bytes as usize / 2).saturating_sub(1); // without the terminating null
    Some(String::from_utf16_lossy(&buf[..len.min(buf.len())]))
}

/// Start this exe with Windows (`true`), or remove the entry (`false`).
pub fn set(enabled: bool) -> std::io::Result<()> {
    let (key, name) = (wide(RUN_KEY), wide(VALUE_NAME));
    let result = if enabled {
        let data = wide(&command()?);
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
    } else {
        match unsafe { RegDeleteKeyValueW(HKEY_CURRENT_USER, key.as_ptr(), name.as_ptr()) } {
            ERROR_FILE_NOT_FOUND => ERROR_SUCCESS, // already off
            other => other,
        }
    };
    if result == ERROR_SUCCESS { Ok(()) } else { Err(std::io::Error::from_raw_os_error(result as i32)) }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn quotes_paths_with_spaces() {
        assert_eq!(quote(Path::new(r"C:\My Tools\darkshot.exe")), r#""C:\My Tools\darkshot.exe""#);
    }
}
