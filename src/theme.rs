//! Duskshot's colour themes. Light, Dark and System are egui's own; Twilight is a dark
//! theme of deep aubergine surfaces with lavender accents, and the default.

use eframe::egui::{self, Color32, Stroke, Theme, Visuals};

use crate::settings::ThemeSetting;

/// The six Twilight colours.
pub const BACKGROUND: Color32 = Color32::from_rgb(0x17, 0x15, 0x22);
pub const SURFACE: Color32 = Color32::from_rgb(0x25, 0x21, 0x37);
pub const ACCENT: Color32 = Color32::from_rgb(0xA9, 0x9B, 0xFF);
pub const TEXT: Color32 = Color32::from_rgb(0xF4, 0xF1, 0xFA);
pub const TEXT_WEAK: Color32 = Color32::from_rgb(0xB8, 0xB0, 0xCA);
/// What is drawn on the accent colour: selected buttons, selected text.
pub const ON_ACCENT: Color32 = BACKGROUND;

// Shades between them for borders and the states of a control, and two signal colours
// that sit well on the surfaces: a soft rose for errors and a warm sand for warnings.
const BORDER: Color32 = Color32::from_rgb(0x3A, 0x34, 0x54);
const BORDER_STRONG: Color32 = Color32::from_rgb(0x5B, 0x53, 0x80);
const HOVER: Color32 = Color32::from_rgb(0x32, 0x2C, 0x49);
const PRESSED: Color32 = Color32::from_rgb(0x3D, 0x36, 0x59);
const ERROR: Color32 = Color32::from_rgb(0xFF, 0x8F, 0xA3);
const WARNING: Color32 = Color32::from_rgb(0xF2, 0xC8, 0x79);

/// Use `setting` for every window of this egui context. Cheap to call every frame.
pub fn apply(ctx: &egui::Context, setting: ThemeSetting) {
    let id = egui::Id::unique("duskshot-theme");
    if ctx.data(|d| d.get_temp::<ThemeSetting>(id)) == Some(setting) {
        return;
    }
    // Twilight takes the place of the dark theme, so the others must put it back.
    let mut dark = Visuals::dark();
    dark.text_options = ctx.style_of(Theme::Dark).visuals.text_options;
    if setting == ThemeSetting::Twilight {
        twilight(&mut dark);
    }
    ctx.set_visuals_of(Theme::Dark, dark);
    ctx.set_theme(egui::ThemePreference::from(setting));
    ctx.data_mut(|d| d.insert_temp(id, setting));
}

fn twilight(v: &mut Visuals) {
    v.panel_fill = BACKGROUND;
    // Floating bars (the toolbar, popups) sit one step above the panels.
    v.window_fill = SURFACE;
    v.window_stroke = Stroke::new(1.0, BORDER);
    v.extreme_bg_color = BACKGROUND;
    v.faint_bg_color = SURFACE;
    v.code_bg_color = SURFACE;
    v.weak_text_color = Some(TEXT_WEAK);
    v.hyperlink_color = ACCENT;
    v.error_fg_color = ERROR;
    v.warn_fg_color = WARNING;
    v.selection.bg_fill = ACCENT;
    v.selection.stroke = Stroke::new(1.0, ON_ACCENT);

    let w = &mut v.widgets;
    w.noninteractive.bg_fill = BACKGROUND;
    w.noninteractive.weak_bg_fill = BACKGROUND;
    w.noninteractive.bg_stroke = Stroke::new(1.0, BORDER);
    w.noninteractive.fg_stroke = Stroke::new(1.0, TEXT);
    w.inactive.bg_fill = SURFACE;
    w.inactive.weak_bg_fill = SURFACE;
    w.inactive.bg_stroke = Stroke::new(1.0, BORDER);
    w.inactive.fg_stroke = Stroke::new(1.0, TEXT);
    w.hovered.bg_fill = HOVER;
    w.hovered.weak_bg_fill = HOVER;
    w.hovered.bg_stroke = Stroke::new(1.0, BORDER_STRONG);
    w.hovered.fg_stroke = Stroke::new(1.5, TEXT);
    w.active.bg_fill = PRESSED;
    w.active.weak_bg_fill = PRESSED;
    w.active.bg_stroke = Stroke::new(1.0, ACCENT);
    w.active.fg_stroke = Stroke::new(2.0, TEXT);
    w.open.bg_fill = SURFACE;
    w.open.weak_bg_fill = SURFACE;
    w.open.bg_stroke = Stroke::new(1.0, BORDER_STRONG);
    w.open.fg_stroke = Stroke::new(1.0, TEXT);
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn twilight_replaces_dark_and_the_others_put_it_back() {
        let ctx = egui::Context::default();
        let dark_fill = || ctx.style_of(Theme::Dark).visuals.panel_fill;
        let stock = dark_fill();
        apply(&ctx, ThemeSetting::Twilight);
        assert_eq!(dark_fill(), BACKGROUND);
        assert_eq!(ctx.options(|o| o.theme_preference), egui::ThemePreference::Dark);
        apply(&ctx, ThemeSetting::Dark);
        assert_eq!(dark_fill(), stock);
        apply(&ctx, ThemeSetting::Twilight);
        apply(&ctx, ThemeSetting::System);
        assert_eq!(dark_fill(), stock);
        assert_eq!(ctx.options(|o| o.theme_preference), egui::ThemePreference::System);
    }
}
