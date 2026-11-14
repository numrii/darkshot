//! The annotation tools, in toolbar order, then the eyedropper and the readers (text and
//! QR codes): one list for the capture toolbar, its keyboard shortcuts and the "Enabled
//! tools" setting.

use eframe::egui::Key;

use crate::icons::Icon;

pub struct ToolInfo {
    /// Stored in the settings file's `disabled_tools`; never change one.
    pub id: &'static str,
    /// Shown in Settings.
    pub name: &'static str,
    pub icon: Icon,
    /// The key that picks the tool, unless the settings give it another.
    pub key: Key,
    /// The toolbar button's tooltip, with `{key}` where the tool's key goes.
    pub tip: &'static str,
}

impl ToolInfo {
    /// The tooltip, naming the tool's key.
    pub fn tip(&self, key: Key) -> String {
        self.tip.replace("{key}", key.name())
    }
}

pub const TOOLS: [ToolInfo; 14] = [
    ToolInfo { id: "select", name: "Select and move", icon: Icon::Select, key: Key::V, tip: "Select, move and resize ({key})" },
    ToolInfo { id: "rect", name: "Rectangle", icon: Icon::Rect, key: Key::R, tip: "Rectangle ({key})" },
    ToolInfo { id: "ellipse", name: "Ellipse", icon: Icon::Ellipse, key: Key::E, tip: "Ellipse ({key})" },
    ToolInfo { id: "line", name: "Line", icon: Icon::Line, key: Key::L, tip: "Line ({key})" },
    ToolInfo { id: "arrow", name: "Arrow", icon: Icon::Arrow, key: Key::A, tip: "Arrow ({key})" },
    ToolInfo { id: "pen", name: "Pen", icon: Icon::Pen, key: Key::P, tip: "Pen ({key}), Shift for a straight line" },
    ToolInfo {
        id: "highlighter",
        name: "Highlighter",
        icon: Icon::Marker,
        key: Key::M,
        tip: "Highlighter ({key}), Shift for a straight line",
    },
    ToolInfo { id: "text", name: "Text", icon: Icon::Text, key: Key::T, tip: "Text ({key})" },
    ToolInfo {
        id: "blur",
        name: "Blur",
        icon: Icon::Blur,
        key: Key::B,
        tip: "Blur: pixelate, Gaussian blur or redact ({key})",
    },
    ToolInfo { id: "step", name: "Numbered steps", icon: Icon::Step, key: Key::N, tip: "Numbered step ({key})" },
    ToolInfo {
        id: "emoji",
        name: "Emoji stamp",
        icon: Icon::Emoji,
        key: Key::J,
        tip: "Emoji stamp ({key}), Win+. in the picker for any emoji",
    },
    ToolInfo {
        id: "eyedropper",
        name: "Eyedropper",
        icon: Icon::Eyedropper,
        key: Key::I,
        tip: "Pick a colour from the screen ({key})",
    },
    ToolInfo {
        id: "read-text",
        name: "Copy text (OCR)",
        icon: Icon::ReadText,
        key: Key::O,
        tip: "Read the text in the region ({key})",
    },
    ToolInfo { id: "read-qr", name: "Read QR code", icon: Icon::ReadQr, key: Key::Q, tip: "Read a QR code in the region ({key})" },
];

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ids_and_keys_are_unique() {
        for (i, a) in TOOLS.iter().enumerate() {
            for b in &TOOLS[i + 1..] {
                assert!(a.id != b.id && a.key != b.key && a.icon != b.icon, "{} and {}", a.id, b.id);
            }
            assert!(a.tip.contains("{key}"), "{}", a.id);
            assert!(crate::settings::tool_key_allowed(a.key), "{}", a.id);
        }
    }
}
