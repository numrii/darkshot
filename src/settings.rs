//! User settings, stored as `duskshot.toml` next to the exe so Duskshot stays
//! portable. With the `appdata-fallback` feature, if that folder cannot be written
//! (say, the exe is in Program Files), the file goes to `%APPDATA%\Duskshot` instead;
//! without it, saving settings fails with an error. Missing or unreadable values fall back
//! to their defaults, so an old or hand-edited file never stops Duskshot starting.

use std::collections::BTreeMap;
use std::path::PathBuf;
#[cfg(windows)]
use std::str::FromStr;

use eframe::egui::{Color32, Key};
use serde::{Deserialize, Serialize};

use crate::save::Format;

pub const FILE_NAME: &str = "duskshot.toml";

/// Whether this build can free Win+ shortcuts from Windows (see `winkeys`).
pub const FREE_SHORTCUT_AVAILABLE: bool = cfg!(all(windows, feature = "free-shortcut"));

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Settings {
    /// The capture shortcut, such as `Ctrl+F9` or `PrintScreen`.
    pub hotkey: String,
    /// A shortcut for Edit clipboard image; empty for none.
    pub clipboard_hotkey: String,
    /// A shortcut for Reopen last capture; empty for none.
    pub reopen_hotkey: String,
    /// Reopen last capture: minutes the last capture is kept, in memory only, after
    /// the overlay closes. 0 is off.
    pub keep_minutes: u32,
    /// The four colour swatches in the toolbar, as `#RRGGBB`.
    pub swatches: [String; 4],
    /// Tools left out of the toolbar, by their id in [`crate::tools::TOOLS`]. Listing
    /// the ones turned off, not on, keeps tools added later switched on.
    pub disabled_tools: Vec<String>,
    /// Keys chosen for tools, by tool id, as egui key names (`K`, `F2`, `1`). Tools not
    /// listed use their own key.
    pub tool_keys: BTreeMap<String, String>,
    /// The highlighter's starting colour.
    pub marker_color: String,
    /// Light or dark windows and toolbars, or follow Windows.
    pub theme: ThemeSetting,
    /// Draw the mouse cursor into the capture.
    pub include_cursor: bool,
    /// Ask before Esc or Close throws away a capture that has annotations.
    pub confirm_discard: bool,
    /// How much the box beside the selection explains about the current tool.
    pub help_level: crate::help::HelpLevel,
    pub save: SaveSettings,
    pub frame: FrameSettings,
    pub guides: GuideSettings,
}

/// Lines drawn over the region while the capture is being turned, to line things up
/// against. They are fixed on screen while the capture turns under them, and are never
/// part of the copy or the saved file.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct GuideSettings {
    /// A level line through the middle of the region.
    pub horizon: bool,
    /// A grid of level and upright lines, centred on the region.
    pub grid: bool,
    pub spacing: GridSpacing,
}

/// How far apart the grid lines are, in screen points.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum GridSpacing {
    Fine,
    Medium,
    #[default]
    Coarse,
}

impl GridSpacing {
    pub fn points(self) -> f32 {
        match self {
            Self::Fine => 40.0,
            Self::Medium => 60.0,
            Self::Coarse => 120.0,
        }
    }
}

impl Default for GuideSettings {
    fn default() -> Self {
        Self { horizon: true, grid: true, spacing: GridSpacing::Coarse }
    }
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
    Twilight,
    System,
    Light,
    Dark,
}

impl From<ThemeSetting> for eframe::egui::ThemePreference {
    fn from(t: ThemeSetting) -> Self {
        match t {
            // Twilight is a dark theme.
            ThemeSetting::Twilight => Self::Dark,
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
/// The longest Reopen last capture keeps a capture, in minutes.
pub const MAX_KEEP_MINUTES: u32 = 60;
pub const DEFAULT_NAME: &str = "duskshot-{date}_{time}";
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
            clipboard_hotkey: String::new(),
            reopen_hotkey: String::new(),
            keep_minutes: 0,
            swatches: DEFAULT_SWATCHES.map(to_hex),
            disabled_tools: Vec::new(),
            tool_keys: BTreeMap::new(),
            marker_color: to_hex(DEFAULT_MARKER_COLOR),
            theme: ThemeSetting::Twilight,
            include_cursor: false,
            confirm_discard: false,
            help_level: crate::help::HelpLevel::Off,
            save: SaveSettings::default(),
            frame: FrameSettings::default(),
            guides: GuideSettings::default(),
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
        #[cfg(windows)]
        {
            if !shortcut_valid(&settings.hotkey) {
                settings.hotkey = DEFAULT_HOTKEY.into();
            }
            for extra in [&mut settings.clipboard_hotkey, &mut settings.reopen_hotkey] {
                if !extra.is_empty() && !shortcut_valid(extra) {
                    extra.clear();
                }
            }
        }
        settings.keep_minutes = settings.keep_minutes.min(MAX_KEEP_MINUTES);
        settings.tool_keys.retain(|id, key| {
            crate::tools::TOOLS.iter().any(|t| t.id == id) && Key::from_name(key).is_some_and(tool_key_allowed)
        });
        settings.save.jpeg_quality = settings.save.jpeg_quality.clamp(1, 100);
        settings.frame.border_width = settings.frame.border_width.clamp(1, MAX_BORDER_WIDTH);
        let (prefix, suffix) = (settings.save.prefix.take(), settings.save.suffix.take());
        if (prefix.is_some() || suffix.is_some()) && settings.save.name == DEFAULT_NAME {
            settings.save.name = format!("{}{}", prefix.unwrap_or_default(), suffix.unwrap_or_default());
        }
        settings
    }

    /// Write the settings next to the exe, or (with `appdata-fallback`) to
    /// `%APPDATA%\Duskshot` if that fails.
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

    /// The key that picks `tool` in the capture toolbar.
    pub fn tool_key(&self, tool: &crate::tools::ToolInfo) -> Key {
        self.tool_keys.get(tool.id).and_then(|name| Key::from_name(name)).unwrap_or(tool.key)
    }

    /// Give `tool` its own key, or `None` for its usual one.
    pub fn set_tool_key(&mut self, tool: &crate::tools::ToolInfo, key: Option<Key>) {
        match key.filter(|&k| k != tool.key) {
            Some(key) => self.tool_keys.insert(tool.id.to_owned(), key.name().to_owned()),
            None => self.tool_keys.remove(tool.id),
        };
    }

    /// The tool, other than `except`, whose key is `key`, if any.
    pub fn tool_with_key(&self, key: Key, except: &str) -> Option<&'static crate::tools::ToolInfo> {
        crate::tools::TOOLS.iter().find(|t| t.id != except && self.tool_key(t) == key)
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

/// The emoji stamped most recently, newest first, one per line. Kept in a file of
/// its own beside the settings, so saving the settings never overwrites it.
pub const RECENT_FILE: &str = "duskshot-recent.txt";

/// The recently stamped emoji, if any were recorded.
pub fn load_recent() -> Option<Vec<String>> {
    let text = candidates().into_iter().find_map(|path| std::fs::read_to_string(path.with_file_name(RECENT_FILE)).ok())?;
    let list: Vec<String> = text.lines().map(str::trim).filter(|l| !l.is_empty()).map(str::to_owned).collect();
    (!list.is_empty()).then_some(list)
}

/// Record the recently stamped emoji, where the settings go.
pub fn store_recent(list: &[String]) {
    let text = list.join("\n");
    for path in candidates() {
        if std::fs::write(path.with_file_name(RECENT_FILE), &text).is_ok() {
            return;
        }
    }
}

/// Where the settings file may be, in order of preference: next to the exe (portable),
/// then, with `appdata-fallback`, `%APPDATA%\Duskshot`.
fn candidates() -> Vec<PathBuf> {
    let mut paths = Vec::new();
    if let Some(dir) = std::env::current_exe().ok().and_then(|exe| exe.parent().map(PathBuf::from)) {
        paths.push(dir.join(FILE_NAME));
    }
    #[cfg(feature = "appdata-fallback")]
    if let Some(appdata) = std::env::var_os("APPDATA") {
        paths.push(PathBuf::from(appdata).join("Duskshot").join(FILE_NAME));
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
#[cfg(windows)]
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

/// Whether `key` can pick a tool. Keys the capture already uses (Esc, Enter, Delete,
/// Backspace, the arrows, Tab, Space, and F1 for help) cannot, nor modifier-only or
/// media keys.
pub fn tool_key_allowed(key: Key) -> bool {
    if key == Key::F1 {
        return false;
    }
    let name = key.name();
    let alphanumeric = name.len() == 1 && name.as_bytes()[0].is_ascii_alphanumeric();
    let function = name.strip_prefix('F').and_then(|n| n.parse::<u8>().ok()).is_some_and(|n| (1..=24).contains(&n));
    alphanumeric
        || function
        || matches!(
            key,
            Key::Minus
                | Key::Equals
                | Key::Comma
                | Key::Period
                | Key::Slash
                | Key::Backslash
                | Key::Backtick
                | Key::Semicolon
                | Key::Quote
                | Key::OpenBracket
                | Key::CloseBracket
                | Key::Insert
                | Key::Home
                | Key::End
                | Key::PageUp
                | Key::PageDown
                | Key::Plus
                | Key::IntlBackslash
        )
}

/// Whether `text` can be used as the capture shortcut: registrable as a hotkey.
pub fn shortcut_valid(text: &str) -> bool {
    #[cfg(windows)]
    {
        return parse_hotkey(text).is_some();
    }
    #[cfg(not(windows))]
    {
        let _ = text;
        false
    }
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
const NAMED_KEYS: [(&str, u32); 29] = [
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
    ("IntlBackslash", 0xE2),
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

/// The Windows key code for a punctuation key's name in a shortcut (`Backquote`),
/// whose printed character depends on the keyboard layout.
pub fn punctuation_key(name: &str) -> Option<u32> {
    virtual_key(&name.to_ascii_uppercase()).filter(|vk| matches!(vk, 0xBA..=0xC0 | 0xDB..=0xDF | 0xE2))
}

/// The Windows virtual-key code for a key name as written in shortcuts.
fn virtual_key(name: &str) -> Option<u32> {
    let name = name.strip_prefix("KEY").filter(|n| n.len() == 1).unwrap_or(name);
    let name = name.strip_prefix("DIGIT").filter(|n| n.len() == 1).unwrap_or(name);
    if let [c] = name.as_bytes()
        && (c.is_ascii_uppercase() || c.is_ascii_digit()) {
            return Some(u32::from(*c));
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
        "INTLBACKSLASH" => 0xE2,
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
        // A Linux run preserves global-shortcut settings for a future Windows run;
        // it cannot validate or use those shortcuts itself.
        #[cfg(windows)]
        assert_eq!(s.hotkey, DEFAULT_HOTKEY);
        #[cfg(not(windows))]
        assert_eq!(s.hotkey, "Banana+Q");
        assert_eq!(s.save.name, "shot {counter}"); // an old prefix and suffix carry over
        assert_eq!(s.save.jpeg_quality, 100);
        assert_eq!(s.swatch_colors(), DEFAULT_SWATCHES);
        assert_eq!(s.theme, ThemeSetting::Twilight);
        assert_eq!(Settings::from_toml("theme = \"light\"").theme, ThemeSetting::Light);
        assert_eq!(Settings::from_toml("theme = \"system\"").theme, ThemeSetting::System);

        // Tools are all on by default; turning one off is remembered, and an old
        // file's `take_over` is ignored.
        assert!(Settings::default().tool_enabled("blur"));
        let mut t = Settings::from_toml("take_over = true\ndisabled_tools = [\"blur\"]");
        assert!(!t.tool_enabled("blur") && t.tool_enabled("pen"));
        t.set_tool_enabled("blur", true);
        t.set_tool_enabled("pen", false);
        t.set_tool_enabled("pen", false);
        assert_eq!(t.disabled_tools, ["pen"]);

        // With no settings file, or an empty one, everything is at its default: Twilight,
        // tool help off, nothing kept, a coarse grid with the horizon.
        let d = Settings::from_toml("");
        assert_eq!(d, Settings::default());
        assert_eq!(d.theme, ThemeSetting::Twilight);
        assert_eq!(d.help_level, crate::help::HelpLevel::Off);
        assert_eq!(d.keep_minutes, 0);
        assert_eq!(d.save.name, "duskshot-{date}_{time}");
        assert_eq!(d.hotkey, DEFAULT_HOTKEY);
        assert!(d.guides.horizon && d.guides.grid && d.guides.spacing == GridSpacing::Coarse);
        assert!(d.disabled_tools.is_empty());

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

        // Rotation guides: horizon and a coarse grid unless changed; an unknown spacing
        // falls back to the default along with the rest of the file.
        let g = Settings::default().guides;
        assert!(g.horizon && g.grid && g.spacing == GridSpacing::Coarse);
        let g = Settings::from_toml("[guides]
grid = false
spacing = \"fine\"").guides;
        assert!(g.horizon && !g.grid && g.spacing == GridSpacing::Fine);
        assert_eq!(GridSpacing::Fine.points(), 40.0);

        // Tool keys: chosen ones are kept; unknown tools and unusable keys are dropped.
        let tools = &crate::tools::TOOLS;
        let k = Settings::from_toml("[tool_keys]
rect = \"K\"
pen = \"Escape\"
nope = \"X\"
arrow = \"Banana\"");
        assert_eq!(k.tool_keys.len(), 1);
        assert_eq!(k.tool_key(&tools[1]), Key::K);
        assert_eq!(k.tool_key(&tools[5]), Key::P);
        assert_eq!(k.tool_with_key(Key::K, "pen").map(|t| t.id), Some("rect"));
        assert!(k.tool_with_key(Key::R, "pen").is_none());
        let mut k = k;
        k.set_tool_key(&tools[1], Some(Key::R)); // its own key: back to the default
        assert!(k.tool_keys.is_empty());
        k.set_tool_key(&tools[1], Some(Key::F2));
        assert_eq!(Settings::from_toml(&toml::to_string_pretty(&k).unwrap()).tool_key(&tools[1]), Key::F2);
        k.set_tool_key(&tools[1], None);
        assert!(k.tool_keys.is_empty());
        assert!(tool_key_allowed(Key::Num1) && !tool_key_allowed(Key::ArrowUp) && !tool_key_allowed(Key::Space));

        // A file that is not TOML at all gives the defaults.
        assert_eq!(Settings::from_toml("not = [valid"), Settings::default());
    }

    #[test]
    #[cfg(windows)]
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
    #[cfg(windows)]
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
