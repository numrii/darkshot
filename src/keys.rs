//! Keys as the local keyboard layout has them.
//!
//! egui names a key after the character it types when it can (`+`, `-`), and otherwise
//! after where it sits on a US keyboard: Ö on a Swedish keyboard arrives as
//! `Semicolon`. Shortcuts are stored the same way, by Windows key code. These helpers
//! find the key code a press really was, and the label printed on that key here.

use eframe::egui::Key;
#[cfg(windows)]
use windows_sys::Win32::UI::Input::KeyboardAndMouse::{
    GetKeyboardLayout, MapVirtualKeyExW, MAPVK_VK_TO_CHAR, MAPVK_VSC_TO_VK,
};

/// The scan code of the key in `key`'s place on a US keyboard, for keys that type a
/// letter, digit or punctuation (whose place differs between layouts).
#[cfg(windows)]
fn scan_code(key: Key) -> Option<u32> {
    const ROWS: [(&str, u32); 4] = [("1234567890", 0x02), ("QWERTYUIOP", 0x10), ("ASDFGHJKL", 0x1E), ("ZXCVBNM", 0x2C)];
    let name = key.name();
    if let [c] = name.as_bytes() {
        return ROWS.iter().find_map(|(row, first)| row.find(char::from(*c)).map(|i| first + i as u32));
    }
    Some(match key {
        Key::Minus => 0x0C,
        Key::Equals => 0x0D,
        Key::OpenBracket => 0x1A,
        Key::CloseBracket => 0x1B,
        Key::Semicolon => 0x27,
        Key::Quote => 0x28,
        Key::Backtick => 0x29,
        Key::Backslash => 0x2B,
        Key::Comma => 0x33,
        Key::Period => 0x34,
        Key::Slash => 0x35,
        Key::IntlBackslash => 0x56,
        _ => return None,
    })
}

#[cfg(windows)]
fn layout() -> isize {
    // SAFETY: plain query of this thread's keyboard layout.
    unsafe { GetKeyboardLayout(0) as isize }
}

/// The Windows key code of the key at `physical` (a place on a US keyboard, as egui
/// reports it) in the current layout. `None` for keys whose code does not depend on
/// the layout.
#[cfg(windows)]
pub fn virtual_key_at(physical: Key) -> Option<u32> {
    let scan = scan_code(physical)?;
    // SAFETY: plain lookups in the current keyboard layout.
    let vk = unsafe { MapVirtualKeyExW(scan, MAPVK_VSC_TO_VK, layout() as _) };
    (vk != 0).then_some(vk)
}

/// The character the key with Windows key code `vk` types without Shift: `ö` for
/// VK_OEM_3 on a Swedish keyboard.
#[cfg(windows)]
fn typed(vk: u32) -> Option<char> {
    // SAFETY: plain lookup in the current keyboard layout. Dead keys set the top bit.
    let code = unsafe { MapVirtualKeyExW(vk, MAPVK_VK_TO_CHAR, layout() as _) } & 0x7FFF_FFFF;
    char::from_u32(code).filter(|c| !c.is_control() && !c.is_whitespace())
}

/// The character as printed on the key: upper case.
#[cfg(windows)]
fn printed(vk: u32) -> Option<String> {
    typed(vk).map(|c| c.to_uppercase().collect())
}

/// How to show a tool's key: what is printed on it in the current layout.
pub fn label(key: Key) -> String {
    let symbol = match key {
        Key::Minus => "-",
        Key::Equals => "=",
        Key::Plus => "+",
        _ => key.symbol_or_name(),
    };
    if key.name().len() == 1 {
        return symbol.to_owned(); // a letter or digit
    }
    // egui names a key after the character it types, unless it has no name for that
    // character (Ö): then after its place on a US keyboard, so show what the key in
    // that place types here.
    #[cfg(windows)]
    if let Some(c) = virtual_key_at(key).and_then(typed)
        && Key::from_name(&c.to_string()).is_none()
    {
        return c.to_uppercase().collect();
    }
    symbol.to_owned()
}

/// How to show a shortcut stored as text (`Ctrl+Backquote`): with punctuation keys
/// as printed in the current layout (`Ctrl+Ö` on a Swedish keyboard).
pub fn shortcut_label(text: &str) -> String {
    text.split('+')
        .map(|token| {
            let token = token.trim();
            #[cfg(windows)]
            {
                crate::settings::punctuation_key(token)
                    .and_then(printed)
                    .unwrap_or_else(|| token.to_owned())
            }
            #[cfg(not(windows))]
            {
                token.to_owned()
            }
        })
        .collect::<Vec<_>>()
        .join("+")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn labels() {
        // Keys that do not depend on the layout read as they are.
        assert_eq!(label(Key::K), "K");
        assert_eq!(label(Key::F2), "F2");
        assert_eq!(label(Key::Home), "Home");
        #[cfg(windows)]
        {
            assert_eq!(scan_code(Key::Q), Some(0x10));
            assert_eq!(scan_code(Key::Num0), Some(0x0B));
            assert_eq!(scan_code(Key::M), Some(0x32));
        }
        assert_eq!(shortcut_label("Win+Shift+S"), "Win+Shift+S");
        // Punctuation shows a single character, whatever the layout.
        for key in [Key::Semicolon, Key::Comma, Key::Backtick] {
            assert_eq!(label(key).chars().count(), 1, "{key:?}");
        }
    }
}
