//! User settings, stored as `darkshot.toml` next to the exe so Darkshot stays
//! portable. If that folder cannot be written (say, the exe is in Program Files), the
//! file goes to `%APPDATA%\Darkshot` instead. Missing or unreadable values fall back
//! to their defaults, so an old or hand-edited file never stops Darkshot starting.

use std::path::PathBuf;
use std::str::FromStr;

use eframe::egui::Color32;
use serde::{Deserialize, Serialize};

use crate::save::Format;

pub const FILE_NAME: &str = "darkshot.toml";

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Settings {
    /// The capture shortcut, such as `Ctrl+F9` or `PrintScreen`.
    pub hotkey: String,
    /// The four colour swatches in the toolbar, as `#RRGGBB`.
    pub swatches: [String; 4],
    /// The highlighter's starting colour.
    pub marker_color: String,
    pub save: SaveSettings,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct SaveSettings {
    /// The file name is `prefix` + `suffix` + extension. Either may be empty, and both
    /// may contain `{date}` (2026-11-12) and `{time}` (21-50-03).
    pub prefix: String,
    pub suffix: String,
    pub format: FormatSetting,
    /// JPEG quality, 1-100.
    pub jpeg_quality: u8,
    /// The folder the save dialog opens in. Empty means the Pictures folder.
    pub folder: String,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum FormatSetting {
    Png,
    Jpeg,
}

impl From<FormatSetting> for Format {
    fn from(f: FormatSetting) -> Self {
        match f {
            FormatSetting::Png => Format::Png,
            FormatSetting::Jpeg => Format::Jpeg,
        }
    }
}

impl From<Format> for FormatSetting {
    fn from(f: Format) -> Self {
        match f {
            Format::Png => FormatSetting::Png,
            Format::Jpeg => FormatSetting::Jpeg,
        }
    }
}

pub const DEFAULT_HOTKEY: &str = "Ctrl+F9";
pub const DEFAULT_SWATCHES: [Color32; 4] = [
    Color32::from_rgb(0xE5, 0x48, 0x4D),
    Color32::from_rgb(0x30, 0xA4, 0x6C),
    Color32::from_rgb(0x3E, 0x63, 0xDD),
    Color32::from_rgb(0xFF, 0xE6, 0x00),
];
pub const DEFAULT_MARKER_COLOR: Color32 = Color32::from_rgb(0xFF, 0xE6, 0x00);

impl Default for Settings {
    fn default() -> Self {
        Self {
            hotkey: DEFAULT_HOTKEY.into(),
            swatches: DEFAULT_SWATCHES.map(to_hex),
            marker_color: to_hex(DEFAULT_MARKER_COLOR),
            save: SaveSettings::default(),
        }
    }
}

impl Default for SaveSettings {
    fn default() -> Self {
        Self {
            prefix: "darkshot-".into(),
            suffix: "{date}_{time}".into(),
            format: FormatSetting::Png,
            jpeg_quality: crate::save::QUALITY_HIGH,
            folder: String::new(),
        }
    }
}

impl Settings {
    /// Read the settings file, or the defaults if there is none or it cannot be read.
    pub fn load() -> Self {
        candidates()
            .into_iter()
            .find_map(|path| std::fs::read_to_string(path).ok())
            .map(|text| Self::from_toml(&text))
            .unwrap_or_default()
    }

    /// Parse settings, keeping defaults for anything missing or invalid.
    pub fn from_toml(text: &str) -> Self {
        let mut settings: Self = toml::from_str(text).unwrap_or_default();
        if parse_hotkey(&settings.hotkey).is_none() {
            settings.hotkey = DEFAULT_HOTKEY.into();
        }
        settings.save.jpeg_quality = settings.save.jpeg_quality.clamp(1, 100);
        settings
    }

    /// Write the settings next to the exe, or to `%APPDATA%\Darkshot` if that fails.
    /// Returns where they were written.
    pub fn store(&self) -> std::io::Result<PathBuf> {
        let text = toml::to_string_pretty(self).map_err(std::io::Error::other)?;
        let mut last_error = None;
        for path in candidates() {
            if let Some(dir) = path.parent() {
                let _ = std::fs::create_dir_all(dir);
            }
            match std::fs::write(&path, &text) {
                Ok(()) => return Ok(path),
                Err(e) => last_error = Some(e),
            }
        }
        Err(last_error.unwrap_or_else(|| std::io::Error::other("no place to store settings")))
    }

    pub fn swatch_colors(&self) -> [Color32; 4] {
        std::array::from_fn(|i| parse_hex(&self.swatches[i]).unwrap_or(DEFAULT_SWATCHES[i]))
    }

    pub fn marker(&self) -> Color32 {
        parse_hex(&self.marker_color).unwrap_or(DEFAULT_MARKER_COLOR)
    }

    /// The folder the save dialog opens in.
    pub fn save_folder(&self) -> Option<PathBuf> {
        let folder = self.save.folder.trim();
        if folder.is_empty() {
            pictures_folder()
        } else {
            Some(PathBuf::from(folder))
        }
    }
}

/// Where the settings file may be, in order of preference: next to the exe (portable),
/// then `%APPDATA%\Darkshot`.
fn candidates() -> Vec<PathBuf> {
    let mut paths = Vec::new();
    if let Some(dir) = std::env::current_exe().ok().and_then(|exe| exe.parent().map(PathBuf::from)) {
        paths.push(dir.join(FILE_NAME));
    }
    if let Some(appdata) = std::env::var_os("APPDATA") {
        paths.push(PathBuf::from(appdata).join("Darkshot").join(FILE_NAME));
    }
    paths
}

pub fn pictures_folder() -> Option<PathBuf> {
    let home = std::env::var_os("USERPROFILE")?;
    let pictures = PathBuf::from(home).join("Pictures");
    pictures.is_dir().then_some(pictures)
}

/// Parse a shortcut such as `Ctrl+F9`, `Win+Shift+S` or `PrintScreen`. `Win` (or
/// `Windows`) is accepted for the Windows key.
pub fn parse_hotkey(text: &str) -> Option<global_hotkey::hotkey::HotKey> {
    let normalized: Vec<&str> = text
        .split('+')
        .map(|token| {
            let token = token.trim();
            if token.eq_ignore_ascii_case("win") || token.eq_ignore_ascii_case("windows") {
                "Super"
            } else {
                token
            }
        })
        .collect();
    global_hotkey::hotkey::HotKey::from_str(&normalized.join("+")).ok()
}

pub fn to_hex(c: Color32) -> String {
    format!("#{:02X}{:02X}{:02X}", c.r(), c.g(), c.b())
}

pub fn parse_hex(text: &str) -> Option<Color32> {
    let hex = text.trim().trim_start_matches('#');
    if hex.len() != 6 {
        return None;
    }
    let byte = |i: usize| u8::from_str_radix(&hex[i..i + 2], 16).ok();
    Some(Color32::from_rgb(byte(0)?, byte(2)?, byte(4)?))
}

/// Fill in `{date}` and `{time}` with the given parts.
pub fn expand(template: &str, date: &str, time: &str) -> String {
    template.replace("{date}", date).replace("{time}", time)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn round_trip_and_defaults() {
        let settings = Settings::default();
        let text = toml::to_string_pretty(&settings).unwrap();
        assert_eq!(Settings::from_toml(&text), settings);
        assert!(text.contains("hotkey = \"Ctrl+F9\""), "{text}");

        // Missing fields and bad values fall back to defaults; good ones are kept.
        let partial = "hotkey = \"Banana+Q\"\n[save]\nprefix = \"shot \"\njpeg_quality = 250\n";
        let s = Settings::from_toml(partial);
        assert_eq!(s.hotkey, DEFAULT_HOTKEY);
        assert_eq!(s.save.prefix, "shot ");
        assert_eq!(s.save.suffix, "{date}_{time}");
        assert_eq!(s.save.jpeg_quality, 100);
        assert_eq!(s.swatch_colors(), DEFAULT_SWATCHES);

        // A file that is not TOML at all gives the defaults.
        assert_eq!(Settings::from_toml("not = [valid"), Settings::default());
    }

    #[test]
    fn colours_hotkeys_and_templates() {
        assert_eq!(parse_hex("#ffe600"), Some(Color32::from_rgb(0xFF, 0xE6, 0x00)));
        assert_eq!(parse_hex("12345"), None);
        assert_eq!(to_hex(Color32::from_rgb(1, 2, 255)), "#0102FF");
        let mut s = Settings::default();
        s.swatches[2] = "nonsense".into();
        assert_eq!(s.swatch_colors()[2], DEFAULT_SWATCHES[2]);

        assert!(parse_hotkey("Ctrl+F9").is_some());
        assert!(parse_hotkey("PrintScreen").is_some());
        assert!(parse_hotkey("Ctrl+Shift+A").is_some());
        assert!(parse_hotkey("Ctrl+").is_none());
        use global_hotkey::hotkey::{Code, Modifiers};
        let win = parse_hotkey("Win+Shift+S").unwrap();
        assert_eq!((win.mods, win.key), (Modifiers::META | Modifiers::SHIFT, Code::KeyS));
        assert_eq!(parse_hotkey("windows + shift + s"), Some(win));

        assert_eq!(expand("darkshot-{date}_{time}", "2026-11-12", "21-50-03"), "darkshot-2026-11-12_21-50-03");
        assert_eq!(expand("plain", "d", "t"), "plain");
    }
}
