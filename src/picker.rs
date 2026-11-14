//! Darkshot's colour picker: one popup used for every colour in the app.
//!
//! A hue ring around a saturation and value square sits on top, the same in every
//! tab; three tabs (RGB, HSV, CMYK) hold sliders for the exact values, and an alpha
//! slider sits at the bottom. (Wheels drawn per model, an RGB hexagon at the current
//! brightness or a CMYK disc at the current black, went dark and grey along with the
//! colour, and took slider work to get out of.) The colour being edited is
//! kept as hue, saturation, value and a separate alpha, so lowering alpha, or passing
//! through black or grey, never loses the hue or brightness.

use eframe::egui::{
    self, Color32, FontId, Id, Key, Mesh, Modifiers, Pos2, Rect, Response, Sense, Stroke,
    StrokeKind, Vec2, text::CCursorRange, vec2,
};
use std::f32::consts::TAU;

use crate::icons::{self, Icon};

/// Size of the wheel area.
const WHEEL: f32 = 200.0;
/// The HSV tab's hue ring and the saturation/value square inside it.
const RING_OUTER: f32 = 90.0;
const RING_INNER: f32 = 72.0;
const SQUARE: f32 = 100.0;
const TRACK: Vec2 = vec2(200.0, 18.0);
const FIELD: Vec2 = vec2(54.0, 22.0);
const ROW: f32 = 30.0;
/// Height of the area below the tabs, fitting the tallest tab (CMYK, four sliders)
/// so switching tabs never resizes the popup.
const TAB_AREA: f32 = 4.0 * ROW;

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
enum Tab {
    #[default]
    Rgb,
    Hsv,
    Cmyk,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum WheelPart {
    Ring,
    Square,
}

/// What the picker remembers between frames, per colour it edits.
#[derive(Clone, Debug)]
struct State {
    h: f32,
    s: f32,
    v: f32,
    a: u8,
    tab: Tab,
    /// The colour last written out. If the colour changes elsewhere (a swatch is
    /// clicked), the picker starts again from the new colour.
    last: Color32,
    /// The hex field's text while it is being typed in.
    hex: String,
    drag: Option<WheelPart>,
}

impl State {
    fn from_color(color: Color32) -> Self {
        let mut state = Self {
            h: 0.0,
            s: 0.0,
            v: 0.0,
            a: 255,
            tab: Tab::Rgb,
            last: color,
            hex: String::new(),
            drag: None,
        };
        state.sync(color);
        state
    }

    /// Take over a colour set from outside the picker.
    fn sync(&mut self, color: Color32) {
        let [r, g, b, a] = color.to_srgba_unmultiplied();
        self.set_rgb([r, g, b].map(|c| f32::from(c) / 255.0));
        self.a = a;
        self.last = color;
    }

    /// Set the colour from RGB (0-1), keeping the hue when it is grey and the
    /// saturation when it is black, where RGB does not say what they are.
    fn set_rgb(&mut self, rgb: [f32; 3]) {
        let (h, s, v) = rgb_to_hsv(rgb);
        self.v = v;
        if v > 0.0 {
            self.s = s;
            if let Some(h) = h {
                self.h = h;
            }
        }
    }

    fn rgb(&self) -> [f32; 3] {
        hsv_to_rgb(self.h, self.s, self.v)
    }

    fn rgb8(&self) -> [u8; 3] {
        self.rgb().map(to_u8)
    }

    fn color(&self) -> Color32 {
        let [r, g, b] = self.rgb8();
        Color32::from_rgba_unmultiplied(r, g, b, self.a)
    }

    fn hex(&self) -> String {
        let [r, g, b] = self.rgb8();
        format!("#{r:02X}{g:02X}{b:02X}")
    }
}

/// Show the picker for `color` in a popup under `button` while it is open. `alpha`
/// adds the opacity slider. Returns true when the colour changed.
pub fn popup(button: &Response, popup_id: Id, color: &mut Color32, alpha: bool) -> bool {
    let state_id = popup_id.with("state");
    if button.clicked() {
        // Each time the picker opens it starts on the RGB tab.
        button.ctx.data_mut(|d| {
            d.get_temp_mut_or_insert_with(state_id, || State::from_color(*color))
                .tab = Tab::Rgb
        });
    }
    egui::Popup::menu(button)
        .id(popup_id)
        .close_behavior(egui::PopupCloseBehavior::CloseOnClickOutside)
        .show(|ui| edit(ui, state_id, color, alpha))
        .is_some_and(|inner| inner.inner)
}

/// A colour swatch that opens the picker, for settings. Returns its response, marked
/// changed when the colour changed.
pub fn swatch_button(ui: &mut egui::Ui, color: &mut Color32, alpha: bool) -> Response {
    let (rect, mut response) = ui.allocate_exact_size(vec2(36.0, 22.0), Sense::click());
    let popup_id = response.id.with("picker");
    if ui.is_rect_visible(rect) {
        let open = egui::Popup::is_id_open(ui.ctx(), popup_id);
        let visuals = ui.style().interact_selectable(&response, open);
        checkerboard(ui.painter(), rect, 5.0);
        ui.painter().rect_filled(rect, 3.0, *color);
        ui.painter()
            .rect_stroke(rect, 3.0, visuals.bg_stroke, StrokeKind::Inside);
    }
    if popup(&response, popup_id, color, alpha) {
        response.mark_changed();
    }
    response
}

fn edit(ui: &mut egui::Ui, state_id: Id, color: &mut Color32, alpha: bool) -> bool {
    let mut state = ui
        .data_mut(|d| d.get_temp::<State>(state_id))
        .unwrap_or_else(|| State::from_color(*color));
    if state.last != *color {
        state.sync(*color);
    }
    if !alpha {
        state.a = 255;
    }
    ui.spacing_mut().item_spacing = vec2(8.0, 6.0);
    ui.set_width(290.0);

    ui.horizontal(|ui| {
        wheel(ui, &mut state);
        ui.vertical(|ui| {
            ui.add_space(WHEEL / 2.0 - 40.0);
            let (rect, _) = ui.allocate_exact_size(Vec2::splat(56.0), Sense::hover());
            checkerboard(ui.painter(), rect, 7.0);
            ui.painter().rect_filled(rect, 4.0, state.color());
            ui.painter().rect_stroke(
                rect,
                4.0,
                ui.visuals().widgets.noninteractive.bg_stroke,
                StrokeKind::Inside,
            );
            if alpha {
                let percent = format!("{}%", (f32::from(state.a) / 2.55).round());
                ui.add_sized(
                    [56.0, 16.0],
                    egui::Label::new(egui::RichText::new(percent).monospace().weak()),
                );
            }
        });
    });

    ui.add_space(2.0);
    ui.horizontal(|ui| {
        ui.spacing_mut().item_spacing.x = 4.0;
        let width = (ui.available_width() - 8.0) / 3.0;
        for (tab, name) in [(Tab::Rgb, "RGB"), (Tab::Hsv, "HSV"), (Tab::Cmyk, "CMYK")] {
            let button = egui::Button::new(name).selected(state.tab == tab);
            if ui.add_sized([width, 26.0], button).clicked() {
                state.tab = tab;
            }
        }
    });
    ui.add_space(2.0);

    let top = ui.cursor().top();
    match state.tab {
        Tab::Rgb => {
            // Hex on top, so R, G and B sit directly above A.
            hex_row(ui, state_id, &mut state, alpha);
            let rgb = state.rgb();
            for (i, name) in ["R", "G", "B"].into_iter().enumerate() {
                let mut value = rgb[i] * 255.0;
                let grad = |t: f32| {
                    let mut c = rgb;
                    c[i] = t;
                    rgb_color(c, 255)
                };
                if slider(ui, name, &mut value, 255.0, grad, false) {
                    let mut c = rgb;
                    c[i] = value.round() / 255.0;
                    state.set_rgb(c);
                }
            }
        }
        Tab::Hsv => {
            let (h, s, v) = (state.h, state.s, state.v);
            let mut hue = h * 359.0;
            if slider(
                ui,
                "H",
                &mut hue,
                359.0,
                |t| rgb_color(hsv_to_rgb(t, 1.0, 1.0), 255),
                false,
            ) {
                state.h = hue.round() / 359.0;
            }
            let mut sat = s * 100.0;
            if slider(
                ui,
                "S",
                &mut sat,
                100.0,
                |t| rgb_color(hsv_to_rgb(h, t, v), 255),
                false,
            ) {
                state.s = sat.round() / 100.0;
            }
            let mut val = v * 100.0;
            if slider(
                ui,
                "V",
                &mut val,
                100.0,
                |t| rgb_color(hsv_to_rgb(h, s, t), 255),
                false,
            ) {
                state.v = val.round() / 100.0;
            }
        }
        Tab::Cmyk => {
            let inks = rgb_to_cmyk(state.rgb());
            for (i, name) in ["C", "M", "Y", "K"].into_iter().enumerate() {
                let mut value = inks[i] * 255.0;
                let grad = |t: f32| {
                    let mut c = inks;
                    c[i] = t;
                    rgb_color(cmyk_to_rgb(c), 255)
                };
                if slider(ui, name, &mut value, 255.0, grad, false) {
                    let mut c = inks;
                    c[i] = value.round() / 255.0;
                    state.set_rgb(cmyk_to_rgb(c));
                }
            }
        }
    }
    let used = ui.cursor().top() - top;
    ui.add_space((TAB_AREA - used).max(0.0));

    if alpha {
        let rgb = state.rgb();
        let mut a = f32::from(state.a);
        if slider(ui, "A", &mut a, 255.0, |t| rgb_color(rgb, to_u8(t)), true) {
            state.a = a.round() as u8;
        }
    }

    let new = state.color();
    let changed = new != *color;
    *color = new;
    state.last = new;
    ui.data_mut(|d| d.insert_temp(state_id, state));
    changed
}

/// The hex field with a copy button. Clicking the field selects it all, so a colour
/// on the clipboard can be pasted straight over it; with or without `#` is fine.
fn hex_row(ui: &mut egui::Ui, state_id: Id, state: &mut State, alpha: bool) {
    ui.horizontal(|ui| {
        ui.set_height(ROW - 6.0);
        ui.add_sized(
            [22.0, FIELD.y],
            egui::Label::new(egui::RichText::new("Hex").weak()),
        );
        let id = state_id.with("hex");
        let focused = ui.memory(|m| m.has_focus(id));
        if !focused {
            state.hex = state.hex();
        }
        let width = ui.available_width() - 34.0;
        let output = egui::TextEdit::singleline(&mut state.hex)
            .id(id)
            .font(FontId::monospace(14.0))
            .desired_width(width)
            .show(ui);
        let field = &output.response.response;
        if field.gained_focus() {
            let mut text_state = output.state.clone();
            text_state
                .cursor
                .set_char_range(Some(CCursorRange::select_all_str(&state.hex)));
            text_state.store(ui.ctx(), id);
        }
        if field.changed() {
            if let Some((rgb, a)) = parse_hex(&state.hex) {
                state.set_rgb(rgb.map(|c| f32::from(c) / 255.0));
                if let (Some(a), true) = (a, alpha) {
                    state.a = a;
                }
            }
        }
        // Enter only finishes typing; it must not also reach the capture (copy and close).
        if field.lost_focus() {
            ui.input_mut(|i| i.consume_key(Modifiers::NONE, Key::Enter));
        }

        let (rect, copy) = ui.allocate_exact_size(vec2(26.0, FIELD.y), Sense::click());
        let visuals = ui.style().interact(&copy);
        ui.painter().rect(
            rect,
            3.0,
            visuals.bg_fill,
            visuals.bg_stroke,
            StrokeKind::Inside,
        );
        let icon = Rect::from_center_size(rect.center(), Vec2::splat(17.0));
        icons::paint(
            ui.painter(),
            icon,
            Icon::Copy,
            visuals.fg_stroke.color,
            visuals.bg_fill,
        );
        if copy.on_hover_text("Copy").clicked() {
            ui.ctx().copy_text(state.hex());
        }
    });
}

/// One labelled slider with a gradient track and a number field. `value` runs from 0
/// to `max`; `grad` gives the colour at each point of the track (0-1).
fn slider(
    ui: &mut egui::Ui,
    name: &str,
    value: &mut f32,
    max: f32,
    grad: impl Fn(f32) -> Color32,
    checker: bool,
) -> bool {
    let mut changed = false;
    ui.horizontal(|ui| {
        ui.set_height(ROW - 6.0);
        ui.add_sized(
            [14.0, FIELD.y],
            egui::Label::new(egui::RichText::new(name).weak()),
        );

        let (rect, response) = ui.allocate_exact_size(TRACK, Sense::click_and_drag());
        if response.is_pointer_button_down_on() {
            if let Some(p) = response.interact_pointer_pos() {
                let t = ((p.x - rect.left()) / rect.width()).clamp(0.0, 1.0);
                *value = (t * max).round();
                changed = true;
            }
        }
        if ui.is_rect_visible(rect) {
            let painter = ui.painter();
            if checker {
                checkerboard(painter, rect, 6.0);
            }
            const STEPS: usize = 24;
            let mut mesh = Mesh::default();
            for i in 0..=STEPS {
                let t = i as f32 / STEPS as f32;
                let x = rect.left() + t * rect.width();
                let c = grad(t);
                mesh.colored_vertex(Pos2::new(x, rect.top()), c);
                mesh.colored_vertex(Pos2::new(x, rect.bottom()), c);
                if i > 0 {
                    let n = (i * 2) as u32;
                    mesh.add_triangle(n - 2, n - 1, n);
                    mesh.add_triangle(n - 1, n, n + 1);
                }
            }
            painter.add(mesh);
            let x = rect.left() + (*value / max).clamp(0.0, 1.0) * rect.width();
            let handle =
                Rect::from_center_size(Pos2::new(x, rect.center().y), vec2(3.0, rect.height()));
            painter.rect_filled(handle, 0.0, Color32::WHITE);
            painter.rect_stroke(
                handle,
                0.0,
                Stroke::new(1.0, Color32::BLACK),
                StrokeKind::Outside,
            );
        }

        ui.style_mut().override_font_id = Some(FontId::monospace(14.0));
        let field = egui::DragValue::new(value)
            .range(0.0..=max)
            .max_decimals(0)
            .speed(1.0);
        if ui.add_sized(FIELD, field).changed() {
            changed = true;
        }
    });
    changed
}

/// The hue ring and saturation/value square, with markers on the current colour.
fn wheel(ui: &mut egui::Ui, state: &mut State) {
    let (rect, response) = ui.allocate_exact_size(Vec2::splat(WHEEL), Sense::click_and_drag());
    let c = rect.center();
    if response.is_pointer_button_down_on() {
        if let Some(p) = response.interact_pointer_pos() {
            let d = p - c;
            let (r, angle) = (d.length(), angle_of(d));
            let part = *state.drag.get_or_insert(if r >= RING_INNER - 4.0 {
                WheelPart::Ring
            } else {
                WheelPart::Square
            });
            match part {
                WheelPart::Ring => state.h = angle,
                WheelPart::Square => {
                    state.s = ((d.x + SQUARE / 2.0) / SQUARE).clamp(0.0, 1.0);
                    state.v = 1.0 - ((d.y + SQUARE / 2.0) / SQUARE).clamp(0.0, 1.0);
                }
            }
        }
    } else {
        state.drag = None;
    }
    if !ui.is_rect_visible(rect) {
        return;
    }

    let painter = ui.painter();
    let at = |angle: f32, r: f32| c + vec2((angle * TAU).sin(), -(angle * TAU).cos()) * r;
    let mut mesh = Mesh::default();
    // A hue ring around a square of saturation (across) and value (down).
    const SEGMENTS: u32 = 96;
    for seg in 0..SEGMENTS {
        let angle = seg as f32 / SEGMENTS as f32;
        let color = rgb_color(hsv_to_rgb(angle, 1.0, 1.0), 255);
        mesh.colored_vertex(at(angle, RING_INNER), color);
        mesh.colored_vertex(at(angle, RING_OUTER), color);
        let (n, next) = (seg * 2, ((seg + 1) % SEGMENTS) * 2);
        mesh.add_triangle(n, n + 1, next);
        mesh.add_triangle(n + 1, next + 1, next);
    }
    const CELLS: u32 = 12;
    let base = mesh.vertices.len() as u32;
    let corner = c - Vec2::splat(SQUARE / 2.0);
    for y in 0..=CELLS {
        for x in 0..=CELLS {
            let (s, t) = (x as f32 / CELLS as f32, y as f32 / CELLS as f32);
            mesh.colored_vertex(
                corner + vec2(s, t) * SQUARE,
                rgb_color(hsv_to_rgb(state.h, s, 1.0 - t), 255),
            );
            if x > 0 && y > 0 {
                let i = base + y * (CELLS + 1) + x;
                let (up, left) = (i - CELLS - 1, i - 1);
                mesh.add_triangle(up - 1, up, left);
                mesh.add_triangle(up, i, left);
            }
        }
    }
    painter.add(mesh);
    draw_marker(painter, at(state.h, (RING_INNER + RING_OUTER) / 2.0));
    draw_marker(painter, corner + vec2(state.s, 1.0 - state.v) * SQUARE);
}

fn draw_marker(painter: &egui::Painter, p: Pos2) {
    painter.circle_stroke(p, 6.0, Stroke::new(2.0, Color32::BLACK));
    painter.circle_stroke(p, 5.0, Stroke::new(1.5, Color32::WHITE));
}

/// Grey squares behind anything partly transparent.
fn checkerboard(painter: &egui::Painter, rect: Rect, cell: f32) {
    let painter = painter.with_clip_rect(rect.intersect(painter.clip_rect()));
    painter.rect_filled(rect, 0.0, Color32::from_gray(102));
    let (cols, rows) = (
        (rect.width() / cell).ceil() as usize,
        (rect.height() / cell).ceil() as usize,
    );
    for y in 0..rows {
        for x in (y % 2..cols).step_by(2) {
            let min = rect.min + vec2(x as f32, y as f32) * cell;
            painter.rect_filled(
                Rect::from_min_size(min, Vec2::splat(cell)),
                0.0,
                Color32::from_gray(153),
            );
        }
    }
}

/// The angle of `d` as a fraction of a turn, clockwise from straight up.
fn angle_of(d: Vec2) -> f32 {
    (d.x.atan2(-d.y) / TAU).rem_euclid(1.0)
}

fn to_u8(x: f32) -> u8 {
    (x.clamp(0.0, 1.0) * 255.0).round() as u8
}

fn rgb_color(rgb: [f32; 3], a: u8) -> Color32 {
    let [r, g, b] = rgb.map(to_u8);
    Color32::from_rgba_unmultiplied(r, g, b, a)
}

/// Hue, saturation and value (all 0-1) to RGB (0-1).
fn hsv_to_rgb(h: f32, s: f32, v: f32) -> [f32; 3] {
    let f = |n: f32| {
        let k = (n + h * 6.0) % 6.0;
        v - v * s * k.min(4.0 - k).clamp(0.0, 1.0)
    };
    [f(5.0), f(3.0), f(1.0)]
}

/// RGB (0-1) to hue, saturation and value. The hue is `None` for greys.
fn rgb_to_hsv([r, g, b]: [f32; 3]) -> (Option<f32>, f32, f32) {
    let max = r.max(g).max(b);
    let d = max - r.min(g).min(b);
    let s = if max > 0.0 { d / max } else { 0.0 };
    if d <= 0.0 {
        return (None, s, max);
    }
    let h = if max == r {
        (g - b) / d
    } else if max == g {
        (b - r) / d + 2.0
    } else {
        (r - g) / d + 4.0
    };
    (Some((h / 6.0).rem_euclid(1.0)), s, max)
}

fn rgb_to_cmyk([r, g, b]: [f32; 3]) -> [f32; 4] {
    let k = 1.0 - r.max(g).max(b);
    if k >= 1.0 {
        return [0.0, 0.0, 0.0, 1.0];
    }
    let ink = |x: f32| (1.0 - x - k) / (1.0 - k);
    [ink(r), ink(g), ink(b), k]
}

fn cmyk_to_rgb([c, m, y, k]: [f32; 4]) -> [f32; 3] {
    [c, m, y].map(|ink| (1.0 - ink) * (1.0 - k))
}

/// `#RRGGBB` or `#RRGGBBAA`, with or without the `#`, as RGB and the alpha if given.
fn parse_hex(text: &str) -> Option<([u8; 3], Option<u8>)> {
    let hex = text.trim().trim_start_matches('#');
    if !(hex.len() == 6 || hex.len() == 8) || !hex.is_ascii() {
        return None;
    }
    let byte = |i: usize| u8::from_str_radix(&hex[i..i + 2], 16).ok();
    let a = if hex.len() == 8 { Some(byte(6)?) } else { None };
    Some(([byte(0)?, byte(2)?, byte(4)?], a))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn conversions_round_trip() {
        for rgb in [
            [255u8, 0, 0],
            [0, 255, 0],
            [0x3E, 0x63, 0xDD],
            [0xFF, 0xE6, 0x00],
            [12, 34, 56],
            [255, 255, 255],
        ] {
            let f = rgb.map(|c| f32::from(c) / 255.0);
            let (h, s, v) = rgb_to_hsv(f);
            assert_eq!(hsv_to_rgb(h.unwrap_or(0.0), s, v).map(to_u8), rgb);
            assert_eq!(cmyk_to_rgb(rgb_to_cmyk(f)).map(to_u8), rgb);
        }
        assert_eq!(rgb_to_cmyk([0.0; 3]), [0.0, 0.0, 0.0, 1.0]);
        assert_eq!(rgb_to_hsv([0.5; 3]).0, None);
    }

    #[test]
    fn hue_survives_black_grey_and_no_alpha() {
        let blue = Color32::from_rgb(0x3E, 0x63, 0xDD);
        let mut state = State::from_color(blue);
        let hue = state.h;
        state.a = 0;
        let transparent = state.color();
        state.last = transparent;
        // Raising alpha again gives back the colour, not black.
        state.a = 255;
        assert_eq!(state.color(), blue);
        // Dragging value to 0 and back keeps hue and saturation.
        state.set_rgb([0.0; 3]);
        assert_eq!(state.h, hue);
        state.v = 1.0;
        assert!(state.s > 0.5);
        // A grey keeps the hue too.
        state.set_rgb([0.5; 3]);
        assert_eq!(state.h, hue);
    }

    #[test]
    fn hex_input() {
        assert_eq!(parse_hex("#3993E6"), Some(([0x39, 0x93, 0xE6], None)));
        assert_eq!(parse_hex(" 3993e6 "), Some(([0x39, 0x93, 0xE6], None)));
        assert_eq!(
            parse_hex("3993E680"),
            Some(([0x39, 0x93, 0xE6], Some(0x80)))
        );
        assert_eq!(parse_hex("#393E6"), None);
        assert_eq!(parse_hex("#GG93E6"), None);
        assert_eq!(parse_hex("#ÅÅÅ"), None);
    }

    #[test]
    fn wheel_angles() {
        assert!((angle_of(vec2(0.0, -1.0))).abs() < 1e-6);
        assert!((angle_of(vec2(1.0, 0.0)) - 0.25).abs() < 1e-6);
    }
}
