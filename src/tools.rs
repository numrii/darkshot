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
    pub key: Key,
    /// The toolbar button's tooltip.
    pub tip: &'static str,
}

pub const TOOLS: [ToolInfo; 13] = [
    ToolInfo { id: "select", name: "Select and move", icon: Icon::Select, key: Key::V, tip: "Select, move and resize (V)" },
    ToolInfo { id: "rect", name: "Rectangle", icon: Icon::Rect, key: Key::R, tip: "Rectangle (R)" },
    ToolInfo { id: "ellipse", name: "Ellipse", icon: Icon::Ellipse, key: Key::E, tip: "Ellipse (E)" },
    ToolInfo { id: "line", name: "Line", icon: Icon::Line, key: Key::L, tip: "Line (L)" },
    ToolInfo { id: "arrow", name: "Arrow", icon: Icon::Arrow, key: Key::A, tip: "Arrow (A)" },
    ToolInfo { id: "pen", name: "Pen", icon: Icon::Pen, key: Key::P, tip: "Pen (P), Shift for a straight line" },
    ToolInfo {
        id: "highlighter",
        name: "Highlighter",
        icon: Icon::Marker,
        key: Key::M,
        tip: "Highlighter (M), Shift for a straight line",
    },
    ToolInfo { id: "text", name: "Text", icon: Icon::Text, key: Key::T, tip: "Text (T)" },
    ToolInfo {
        id: "blur",
        name: "Blur and redact",
        icon: Icon::Blur,
        key: Key::B,
        tip: "Hide: pixelate, Gaussian blur or redact (B)",
    },
    ToolInfo { id: "step", name: "Numbered steps", icon: Icon::Step, key: Key::N, tip: "Numbered step (N)" },
    ToolInfo {
        id: "eyedropper",
        name: "Eyedropper",
        icon: Icon::Eyedropper,
        key: Key::I,
        tip: "Pick a colour from the screen (I)",
    },
    ToolInfo {
        id: "read-text",
        name: "Copy text (OCR)",
        icon: Icon::ReadText,
        key: Key::O,
        tip: "Read the text in the region (O)",
    },
    ToolInfo { id: "read-qr", name: "Read QR code", icon: Icon::ReadQr, key: Key::Q, tip: "Read a QR code in the region (Q)" },
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
        }
    }
}
