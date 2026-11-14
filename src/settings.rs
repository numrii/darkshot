//! User settings, stored as `darkshot.toml` next to the exe so Darkshot stays
//! portable. With the `appdata-fallback` feature, if that folder cannot be written
//! (say, the exe is in Program Files), the file goes to `%APPDATA%\Darkshot` instead;
//! without it, saving settings fails with an error. Missing or unreadable values fall back
//! to their defaults, so an old or hand-edited file never stops Darkshot starting.

use std::path::PathBuf;
use std::str::FromStr;

use eframe::egui::Color32;
use serde::{Deserialize, Serialize};

use crate::save::Format;

pub const FILE_NAME: &str = "darkshot.toml";

/// Whether this build can free Win+ shortcuts from Windows (see `winkeys`).
pub const FREE_SHORTCUT_AVAILABLE: bool = cfg!(feature = "free-shortcut");

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Settings {
    /// The capture shortcut, such as `Ctrl+F9` or `PrintScreen`.
    pub hotkey: String,
    /// The four colour swatches in the toolbar, as `#RRGGBB`.
    pub swatches: [String; 4],
    /// Tools left out of the toolbar, by their id in [`crate::tools::TOOLS`]. Listing
    /// the ones turned off, not on, keeps tools added later switched on.
    pub disabled_tools: Vec<String>,
    /// The highlighter's starting colour.
    pub marker_color: String,
    /// Light or dark windows and toolbars, or follow Windows.
    pub theme: ThemeSetting,
    /// Draw the mouse cursor into the capture.
    pub include_cursor: bool,
    /// Ask before Esc or Close throws away a capture that has annotations.
    pub confirm_discard: bool,
    pub save: SaveSettings,
    pub frame: FrameSettings,
}

/// A border and a drop shadow added around the image when it is copied or saved.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct FrameSettings {
    pub border: bool,
    /// The border's colour, as `#RRGGBB`.
    pub border_color: String,
    /// The border's thickness in pixels, 1-20.
    pub border_width: u32,
    /// The border's line style: `solid`, `dash`, `long-dash`, `dash-dot` or `dot`.
    pub border_style: crate::annotate::Dash,
    pub shadow: bool,
}

pub const DEFAULT_BORDER_COLOR: Color32 = Color32::from_rgb(0x80, 0x80, 0x80);
pub const MAX_BORDER_WIDTH: u32 = 20;

impl Default for FrameSettings {
    fn default() -> Self {
        Self {
            border: false,
            border_color: to_hex(DEFAULT_BORDER_COLOR),
            border_width: 1,
            border_style: crate::annotate::Dash::Solid,
            shadow: false,
        }
    }
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct SaveSettings {
    /// The file name, without extension. May contain `{date}` (2026-11-12), `{time}`
    /// (21-50-03) and `{counter}` (0001, the next number not used in the save folder).
    pub name: String,
    /// Older versions split the name in two. Read so the name carries over; never written.
    #[serde(skip_serializing)]
    prefix: Option<String>,
    #[serde(skip_serializing)]
    suffix: Option<String>,
    pub format: FormatSetting,
    /// JPEG quality, 1-100.
    pub jpeg_quality: u8,
    /// The folder the save dialog opens in. Empty means the Pictures folder.
    pub folder: String,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum ThemeSetting {
    #[default]
    System,
    Light,
    Dark,
}

impl From<ThemeSetting> for eframe::egui::ThemePreference {
    fn from(t: ThemeSetting) -> Self {
        match t {
            ThemeSetting::System => Self::System,
            ThemeSetting::Light => Self::Light,
            ThemeSetting::Dark => Self::Dark,
        }
    }
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

pub const DEFAULT_HOTKEY: &str = "Win+Shift+S";
pub const DEFAULT_NAME: &str = "darkshot-{date}_{time}";
pub const DEFAULT_SWATCHES: [Color32; 4] = [
    Color32::from_rgb(0xEF, 0x47, 0x6F),
    Color32::from_rgb(0xFF, 0xD1, 0x66),
    Color32::from_rgb(0x06, 0xD6, 0xA0),
    Color32::from_rgb(0x11, 0x8A, 0xB2),
];
pub const DEFAULT_MARKER_COLOR: Color32 = Color32::from_rgb(0xFF, 0xE6, 0x00);

impl Default for Settings {
    fn default() -> Self {
        Self {
            hotkey: DEFAULT_HOTKEY.into(),
            swatches: DEFAULT_SWATCHES.map(to_hex),
            disabled_tools: Vec::new(),
            marker_color: to_hex(DEFAULT_MARKER_COLOR),
            theme: ThemeSetting::System,
            include_cursor: false,
            confirm_discard: false,
            save: SaveSettings::default(),
            frame: FrameSettings::default(),
        }
    }
}

impl Default for SaveSettings {
    fn default() -> Self {
        Self {
            name: DEFAULT_NAME.into(),
            prefix: None,
            suffix: None,
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
        // An older file's `take_over` (from when the personal build used a keyboard
        // hook) is ignored like any unknown key.
        let mut settings: Self = toml::from_str(text).unwrap_or_default();
        if !shortcut_valid(&settings.hotkey) {
            settings.hotkey = DEFAULT_HOTKEY.into();
        }
        settings.save.jpeg_quality = settings.save.jpeg_quality.clamp(1, 100);
        settings.frame.border_width = settings.frame.border_width.clamp(1, MAX_BORDER_WIDTH);
        let (prefix, suffix) = (settings.save.prefix.take(), settings.save.suffix.take());
        if (prefix.is_some() || suffix.is_some()) && settings.save.name == DEFAULT_NAME {
            settings.save.name = format!("{}{}", prefix.unwrap_or_default(), suffix.unwrap_or_default());
        }
        settings
    }

    /// Write the settings next to the exe, or (with `appdata-fallback`) to
    /// `%APPDATA%\Darkshot` if that fails.
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

    /// Whether the tool with this id is shown in the toolbar.
    pub fn tool_enabled(&self, id: &str) -> bool {
        !self.disabled_tools.iter().any(|t| t == id)
    }

    /// Show or leave out the tool with this id.
    pub fn set_tool_enabled(&mut self, id: &str, enabled: bool) {
        self.disabled_tools.retain(|t| t != id);
        if !enabled {
            self.disabled_tools.push(id.to_owned());
        }
    }

    pub fn marker(&self) -> Color32 {
        parse_hex(&self.marker_color).unwrap_or(DEFAULT_MARKER_COLOR)
    }

    /// The border a capture starts with (whether or not it is switched on).
    pub fn border(&self) -> crate::effects::Border {
        let f = &self.frame;
        crate::effects::Border {
            color: parse_hex(&f.border_color).unwrap_or(DEFAULT_BORDER_COLOR),
            width: f.border_width.clamp(1, MAX_BORDER_WIDTH) as f32,
            dash: f.border_style,
        }
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
/// then, with `appdata-fallback`, `%APPDATA%\Darkshot`.
fn candidates() -> Vec<PathBuf> {
    let mut paths = Vec::new();
    if let Some(dir) = std::env::current_exe().ok().and_then(|exe| exe.parent().map(PathBuf::from)) {
        paths.push(dir.join(FILE_NAME));
    }
    #[cfg(feature = "appdata-fallback")]
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

/// Whether `text` can be used as the capture shortcut: registrable as a hotkey.
pub fn shortcut_valid(text: &str) -> bool {
    parse_hotkey(text).is_some()
}

/// Write a recorded combination as shortcut text, Windows-style:
/// `Win+Ctrl+Alt+Shift+Key`. `None` for keys without a name here.
pub fn shortcut_text(vk: u32, mods: u8) -> Option<String> {
    let key = key_name(vk)?;
    let mut parts = Vec::new();
    for (bit, name) in [(MOD_WIN, "Win"), (MOD_CTRL, "Ctrl"), (MOD_ALT, "Alt"), (MOD_SHIFT, "Shift")] {
        if mods & bit != 0 {
            parts.push(name.to_owned());
        }
    }
    parts.push(key);
    Some(parts.join("+"))
}

fn key_name(vk: u32) -> Option<String> {
    if let Some(c) = char::from_u32(vk).filter(|c| c.is_ascii_uppercase() || c.is_ascii_digit()) {
        return Some(c.to_string());
    }
    if (0x70..0x70 + 24).contains(&vk) {
        return Some(format!("F{}", vk - 0x70 + 1));
    }
    NAMED_KEYS.iter().find(|(_, code)| *code == vk).map(|(name, _)| (*name).to_owned())
}

/// Named keys, as written in shortcuts (all understood by both the hotkey parser and
/// [`key_spec`]), and their virtual-key codes.
const NAMED_KEYS: [(&str, u32); 28] = [
    ("PrintScreen", 0x2C),
    ("Pause", 0x13),
    ("Insert", 0x2D),
    ("Delete", 0x2E),
    ("Home", 0x24),
    ("End", 0x23),
    ("PageUp", 0x21),
    ("PageDown", 0x22),
    ("Space", 0x20),
    ("Tab", 0x09),
    ("Enter", 0x0D),
    ("Escape", 0x1B),
    ("Backspace", 0x08),
    ("ArrowLeft", 0x25),
    ("ArrowUp", 0x26),
    ("ArrowRight", 0x27),
    ("ArrowDown", 0x28),
    ("Minus", 0xBD),
    ("Equal", 0xBB),
    ("Comma", 0xBC),
    ("Period", 0xBE),
    ("Slash", 0xBF),
    ("Backquote", 0xC0),
    ("Semicolon", 0xBA),
    ("Quote", 0xDE),
    ("BracketLeft", 0xDB),
    ("Backslash", 0xDC),
    ("BracketRight", 0xDD),
];

/// Modifier bits used by [`key_spec`].
pub const MOD_CTRL: u8 = 1;
pub const MOD_ALT: u8 = 2;
pub const MOD_SHIFT: u8 = 4;
pub const MOD_WIN: u8 = 8;

/// A shortcut as Windows sees it: the virtual-key code of its main key and the
/// modifiers that must be held (`MOD_*` bits). `None` if the text is not a single key
/// with optional modifiers, or the key has no name here.
#[cfg_attr(not(any(test, feature = "free-shortcut")), allow(dead_code))]
pub fn key_spec(text: &str) -> Option<(u32, u8)> {
    let mut mods = 0;
    let mut key = None;
    for token in text.split('+').map(str::trim) {
        let upper = token.to_ascii_uppercase();
        let bit = match upper.as_str() {
            "CTRL" | "CONTROL" => MOD_CTRL,
            "ALT" | "OPTION" => MOD_ALT,
            "SHIFT" => MOD_SHIFT,
            "WIN" | "WINDOWS" | "SUPER" | "META" | "CMD" | "COMMAND" => MOD_WIN,
            _ => 0,
        };
        if bit != 0 {
            mods |= bit;
            continue;
        }
        if key.is_some() {
            return None; // two main keys
        }
        key = Some(virtual_key(&upper)?);
    }
    Some((key?, mods))
}

/// The Windows virtual-key code for a key name as written in shortcuts.
#[cfg_attr(not(any(test, feature = "free-shortcut")), allow(dead_code))]
fn virtual_key(name: &str) -> Option<u32> {
    let name = name.strip_prefix("KEY").filter(|n| n.len() == 1).unwrap_or(name);
    let name = name.strip_prefix("DIGIT").filter(|n| n.len() == 1).unwrap_or(name);
    if let [c] = name.as_bytes() {
        if c.is_ascii_uppercase() || c.is_ascii_digit() {
            return Some(u32::from(*c));
        }
    }
    if let Some(n) = name.strip_prefix('F').and_then(|n| n.parse::<u32>().ok()) {
        return (1..=24).contains(&n).then_some(0x70 + n - 1);
    }
    Some(match name {
        "PRINTSCREEN" | "PRINT" | "SNAPSHOT" => 0x2C,
        "PAUSE" | "PAUSEBREAK" => 0x13,
        "INSERT" => 0x2D,
        "DELETE" => 0x2E,
        "HOME" => 0x24,
        "END" => 0x23,
        "PAGEUP" => 0x21,
        "PAGEDOWN" => 0x22,
        "SPACE" => 0x20,
        "TAB" => 0x09,
        "ENTER" => 0x0D,
        "ESCAPE" | "ESC" => 0x1B,
        "BACKSPACE" => 0x08,
        "ARROWLEFT" | "LEFT" => 0x25,
        "ARROWUP" | "UP" => 0x26,
        "ARROWRIGHT" | "RIGHT" => 0x27,
        "ARROWDOWN" | "DOWN" => 0x28,
        "MINUS" => 0xBD,
        "EQUAL" | "EQUALS" => 0xBB,
        "COMMA" => 0xBC,
        "PERIOD" => 0xBE,
        "SLASH" => 0xBF,
        "BACKQUOTE" | "BACKTICK" => 0xC0,
        "SEMICOLON" => 0xBA,
        "QUOTE" => 0xDE,
        "BRACKETLEFT" | "OPENBRACKET" => 0xDB,
        "BACKSLASH" => 0xDC,
        "BRACKETRIGHT" | "CLOSEBRACKET" => 0xDD,
        _ => return None,
    })
}

pub fn to_hex(c: Color32) -> String {
    format!("#{:02X}{:02X}{:02X}", c.r(), c.g(), c.b())
}

/// `#RRGGBB`, or `#RRGGBBAA` when `c` is not fully opaque.
pub fn to_hex_alpha(c: Color32) -> String {
    let [r, g, b, a] = c.to_srgba_unmultiplied();
    if a == 255 { format!("#{r:02X}{g:02X}{b:02X}") } else { format!("#{r:02X}{g:02X}{b:02X}{a:02X}") }
}

/// `#RRGGBB`, or `#RRGGBBAA` with alpha.
pub fn parse_hex(text: &str) -> Option<Color32> {
    let hex = text.trim().trim_start_matches('#');
    if !hex.is_ascii() || (hex.len() != 6 && hex.len() != 8) {
        return None;
    }
    let byte = |i: usize| u8::from_str_radix(&hex[i..i + 2], 16).ok();
    let a = if hex.len() == 8 { byte(6)? } else { 255 };
    Some(Color32::from_rgba_unmultiplied(byte(0)?, byte(2)?, byte(4)?, a))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn hex_with_and_without_alpha() {
        let half = Color32::from_rgba_unmultiplied(0x12, 0x34, 0x56, 0x80);
        assert_eq!(to_hex_alpha(half), "#12345680");
        assert_eq!(parse_hex("#12345680").unwrap().to_srgba_unmultiplied(), [0x12, 0x34, 0x56, 0x80]);
        assert_eq!(to_hex_alpha(Color32::from_rgb(1, 2, 3)), "#010203");
        assert_eq!(parse_hex("#010203"), Some(Color32::from_rgb(1, 2, 3)));
        assert_eq!(parse_hex("#0102030"), None);
    }

    #[test]
    fn round_trip_and_defaults() {
        let settings = Settings::default();
        let text = toml::to_string_pretty(&settings).unwrap();
        assert_eq!(Settings::from_toml(&text), settings);
        assert!(text.contains("hotkey = \"Win+Shift+S\""), "{text}");
        assert!(!text.contains("prefix"), "{text}");

        // Missing fields and bad values fall back to defaults; good ones are kept.
        let partial = "hotkey = \"Banana+Q\"\n[save]\nprefix = \"shot \"\nsuffix = \"{counter}\"\njpeg_quality = 250\n";
        let s = Settings::from_toml(partial);
        assert_eq!(s.hotkey, DEFAULT_HOTKEY);
        assert_eq!(s.save.name, "shot {counter}"); // an old prefix and suffix carry over
        assert_eq!(s.save.jpeg_quality, 100);
        assert_eq!(s.swatch_colors(), DEFAULT_SWATCHES);
        assert_eq!(s.theme, ThemeSetting::System);
        assert_eq!(Settings::from_toml("theme = \"light\"").theme, ThemeSetting::Light);

        // Tools are all on by default; turning one off is remembered, and an old
        // file's `take_over` is ignored.
        assert!(Settings::default().tool_enabled("blur"));
        let mut t = Settings::from_toml("take_over = true\ndisabled_tools = [\"blur\"]");
        assert!(!t.tool_enabled("blur") && t.tool_enabled("pen"));
        t.set_tool_enabled("blur", true);
        t.set_tool_enabled("pen", false);
        t.set_tool_enabled("pen", false);
        assert_eq!(t.disabled_tools, ["pen"]);

        // The newer options are off unless asked for, and out-of-range values are pulled in.
        let d = Settings::default();
        assert!(!d.include_cursor && !d.confirm_discard);
        assert!(!d.frame.border && !d.frame.shadow);
        let f = Settings::from_toml(
            "include_cursor = true\n[frame]\nborder = true\nborder_width = 99\nborder_color = \"#102030\"\nborder_style = \"dash-dot\"\nshadow = true",
        );
        assert!(f.include_cursor && f.frame.border && f.frame.shadow);
        let border = crate::effects::Border {
            color: Color32::from_rgb(0x10, 0x20, 0x30),
            width: MAX_BORDER_WIDTH as f32,
            dash: crate::annotate::Dash::DashDot,
        };
        assert_eq!(f.border(), border);

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

    }

    #[test]
    fn key_specs() {
        assert_eq!(key_spec("Win+Shift+S"), Some((u32::from(b'S'), MOD_WIN | MOD_SHIFT)));
        assert_eq!(key_spec("ctrl + f9"), Some((0x78, MOD_CTRL)));
        assert_eq!(key_spec("PrintScreen"), Some((0x2C, 0)));
        assert_eq!(key_spec("Alt+KeyQ"), Some((u32::from(b'Q'), MOD_ALT)));
        assert_eq!(key_spec("Ctrl+Digit1"), Some((u32::from(b'1'), MOD_CTRL)));
        assert_eq!(key_spec("Ctrl+F25"), None);
        assert_eq!(key_spec("Ctrl+A+B"), None);
        assert_eq!(key_spec("Ctrl+Shift"), None);
        assert_eq!(key_spec("Ctrl+Banana"), None);

        // Recorded combinations come back as text both parsers understand.
        assert_eq!(shortcut_text(u32::from(b'S'), MOD_WIN | MOD_SHIFT).as_deref(), Some("Win+Shift+S"));
        assert_eq!(shortcut_text(0x78, MOD_CTRL | MOD_ALT).as_deref(), Some("Ctrl+Alt+F9"));
        for (name, code) in NAMED_KEYS {
            let text = shortcut_text(code, MOD_CTRL).unwrap();
            assert_eq!(key_spec(&text), Some((code, MOD_CTRL)), "{name}");
            assert!(parse_hotkey(&text).is_some(), "{name}");
        }
        assert_eq!(shortcut_text(0xE8, 0), None);
        assert!(shortcut_valid("Win+Shift+S"));
    }
}
