//! Toolbar icons, drawn with vector shapes so they stay sharp and need no icon font
//! or image files.

use eframe::egui::{self, vec2, Color32, Pos2, Rect, Response, Sense, Shape, Stroke, StrokeKind, Vec2};

use crate::annotate::{Effect, TextStyle};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Icon {
    Select,
    Rect,
    Ellipse,
    Line,
    Arrow,
    Pen,
    Marker,
    Text,
    Blur,
    Step,
    Eyedropper,
    /// An arrow with a head at both ends.
    DoubleHead,
    Undo,
    Redo,
    Copy,
    Save,
    Close,
}

pub const BUTTON: f32 = 28.0;
/// Width of a dropdown picker button.
pub const PICKER_WIDTH: f32 = 44.0;
/// Icons are designed on a 20x20 grid.
const GRID: f32 = 20.0;
const STROKE: f32 = 1.6;

/// A square icon button. `selected` shows it as the active tool; a disabled button
/// still shows its tooltip but ignores clicks.
pub fn button(ui: &mut egui::Ui, icon: Icon, selected: bool, enabled: bool) -> Response {
    let sense = if enabled { Sense::click() } else { Sense::hover() };
    let (rect, response) = ui.allocate_exact_size(Vec2::splat(BUTTON), sense);
    if !ui.is_rect_visible(rect) {
        return response;
    }
    let visuals = ui.style().interact_selectable(&response, selected);
    let highlighted = selected || enabled && response.hovered();
    let background = if highlighted {
        ui.painter().rect_filled(rect, 4.0, visuals.bg_fill);
        visuals.bg_fill
    } else {
        ui.visuals().window_fill
    };
    let color = if enabled {
        visuals.fg_stroke.color
    } else {
        ui.visuals().weak_text_color().gamma_multiply(0.6)
    };
    let area = Rect::from_center_size(rect.center(), Vec2::splat(GRID));
    paint(ui.painter(), area, icon, color, background);
    response
}

/// A dropdown button for choosing one of `options`. The button and each menu row show
/// a preview drawn by `preview(painter, area, option, colour)`.
pub fn picker<T: Copy + PartialEq>(
    ui: &mut egui::Ui,
    value: &mut T,
    enabled: bool,
    options: &[(T, &str)],
    preview: impl Fn(&egui::Painter, Rect, T, Color32),
) -> Response {
    let popup_id = ui.auto_id_with("picker");
    let open = egui::Popup::is_id_open(ui.ctx(), popup_id);
    let sense = if enabled { Sense::click() } else { Sense::hover() };
    let (rect, mut response) = ui.allocate_exact_size(vec2(PICKER_WIDTH, BUTTON), sense);

    if ui.is_rect_visible(rect) {
        let visuals = ui.style().interact_selectable(&response, open);
        if open || enabled && response.hovered() {
            ui.painter().rect_filled(rect, 4.0, visuals.bg_fill);
        }
        let color = if enabled {
            visuals.fg_stroke.color
        } else {
            ui.visuals().weak_text_color().gamma_multiply(0.6)
        };
        let area = Rect::from_min_max(rect.min + vec2(5.0, 6.0), Pos2::new(rect.right() - 13.0, rect.bottom() - 6.0));
        preview(ui.painter(), area, *value, color);
        // Small "more" caret.
        let c = Pos2::new(rect.right() - 7.0, rect.center().y + 1.0);
        let caret = vec![c + vec2(-3.0, -1.5), c + vec2(3.0, -1.5), c + vec2(0.0, 1.8)];
        ui.painter().add(Shape::convex_polygon(caret, color, Stroke::NONE));
    }

    if enabled {
        let mut chosen = None;
        egui::Popup::menu(&response).id(popup_id).show(|ui| {
            for &(option, name) in options {
                let (row, row_response) = ui.allocate_exact_size(vec2(150.0, 26.0), Sense::click());
                let selected = option == *value;
                let visuals = ui.style().interact_selectable(&row_response, selected);
                if selected || row_response.hovered() {
                    ui.painter().rect_filled(row, 4.0, visuals.bg_fill);
                }
                let color = visuals.fg_stroke.color;
                let area = Rect::from_min_size(row.min + vec2(8.0, 5.0), vec2(44.0, 16.0));
                preview(ui.painter(), area, option, color);
                ui.painter().text(
                    Pos2::new(row.left() + 62.0, row.center().y),
                    egui::Align2::LEFT_CENTER,
                    name,
                    egui::FontId::proportional(13.0),
                    color,
                );
                if row_response.clicked() {
                    chosen = Some(option);
                }
            }
        });
        if let Some(option) = chosen {
            if option != *value {
                *value = option;
                response.mark_changed();
            }
        }
    }
    response
}

/// Preview of a text style: "Ab" plain, with a halo, or in a box.
pub fn text_style_preview(painter: &egui::Painter, area: Rect, style: TextStyle, color: Color32) {
    let font = egui::FontId::proportional(area.height().min(16.0));
    let centre = area.center();
    let text = |at: Pos2, c: Color32| {
        painter.text(at, egui::Align2::CENTER_CENTER, "Ab", font.clone(), c);
    };
    match style {
        TextStyle::Plain => text(centre, color),
        TextStyle::Outline => {
            let halo = crate::annotate::contrast(color).gamma_multiply(0.9);
            for d in [vec2(-1.0, 0.0), vec2(1.0, 0.0), vec2(0.0, -1.0), vec2(0.0, 1.0)] {
                text(centre + d, halo);
            }
            text(centre, color);
        }
        TextStyle::Box => {
            let r = Rect::from_center_size(centre, vec2(area.width().min(30.0), area.height()));
            painter.rect_filled(r, 3.0, color);
            text(centre, crate::annotate::contrast(color));
        }
    }
}

/// Preview of a blur effect: a grid of blocks (pixelate) or soft rings (blur).
pub fn effect_preview(painter: &egui::Painter, area: Rect, effect: Effect, color: Color32) {
    let side = area.width().min(area.height());
    let square = Rect::from_center_size(area.center(), Vec2::splat(side));
    if effect == Effect::Redact {
        // A solid bar over two lines of "text".
        painter.rect_filled(square.shrink(side * 0.05), 1.5, color);
    } else if effect == Effect::Gaussian {
        for (i, alpha) in [0.15, 0.3, 0.5, 0.8].into_iter().enumerate() {
            let r = side / 2.0 * (1.0 - i as f32 * 0.22);
            painter.circle_filled(square.center(), r, color.gamma_multiply(alpha));
        }
    } else {
        // 3x3 blocks in a fixed pattern of strengths.
        let shades = [0.9, 0.35, 0.65, 0.3, 0.8, 0.45, 0.6, 0.25, 0.95];
        let cell = side / 3.0;
        for (i, alpha) in shades.into_iter().enumerate() {
            let min = square.min + vec2((i % 3) as f32, (i / 3) as f32) * cell;
            let r = Rect::from_min_size(min, Vec2::splat(cell)).shrink(0.5);
            painter.rect_filled(r, 0.0, color.gamma_multiply(alpha));
        }
    }
}

/// A small circled question mark; add its tooltip to the response.
pub fn help_mark(ui: &mut egui::Ui) -> Response {
    let (rect, response) = ui.allocate_exact_size(Vec2::splat(18.0), Sense::hover());
    if ui.is_rect_visible(rect) {
        let color = if response.hovered() {
            ui.visuals().strong_text_color()
        } else {
            ui.visuals().weak_text_color()
        };
        ui.painter().circle_stroke(rect.center(), 7.5, Stroke::new(1.2, color));
        ui.painter().text(
            rect.center() + vec2(0.0, 0.5),
            egui::Align2::CENTER_CENTER,
            "?",
            egui::FontId::proportional(11.0),
            color,
        );
    }
    response
}

fn marker_color_id() -> egui::Id {
    egui::Id::unique("darkshot-marker-colour")
}

/// The highlighter colour the highlighter icon shows.
pub fn set_marker_color(ctx: &egui::Context, color: Color32) {
    ctx.data_mut(|d| d.insert_temp(marker_color_id(), color));
}

/// A colour-wheel button that opens the colour picker. The centre dot shows the
/// current colour.
pub fn color_picker_button(ui: &mut egui::Ui, color: &mut Color32) -> Response {
    let popup_id = ui.auto_id_with("color-picker");
    let open = egui::Popup::is_id_open(ui.ctx(), popup_id);
    let (rect, mut response) = ui.allocate_exact_size(Vec2::splat(BUTTON), Sense::click());

    if ui.is_rect_visible(rect) {
        let visuals = ui.style().interact_selectable(&response, open);
        let background = if open || response.hovered() {
            ui.painter().rect_filled(rect, 4.0, visuals.bg_fill);
            visuals.bg_fill
        } else {
            ui.visuals().window_fill
        };
        // A ring of hue segments, each slightly widened so no seams show between them.
        let center = rect.center();
        let (outer, inner) = (8.5, 4.5);
        const SEGMENTS: usize = 24;
        let step = std::f32::consts::TAU / SEGMENTS as f32;
        for i in 0..SEGMENTS {
            let (a0, a1) = (i as f32 * step - 0.02, (i + 1) as f32 * step + 0.02);
            let at = |a: f32, r: f32| center + vec2(a.cos(), a.sin()) * r;
            let hue = Color32::from(egui::ecolor::Hsva::new(i as f32 / SEGMENTS as f32, 0.85, 1.0, 1.0));
            let quad = vec![at(a0, inner), at(a0, outer), at(a1, outer), at(a1, inner)];
            ui.painter().add(Shape::convex_polygon(quad, hue, Stroke::NONE));
        }
        ui.painter().circle_filled(center, inner, background);
        ui.painter().circle_filled(center, 3.0, *color);
        let edge = ui.visuals().widgets.noninteractive.fg_stroke.color.gamma_multiply(0.5);
        ui.painter().circle_stroke(center, 3.0, Stroke::new(1.0, edge));
    }

    if crate::picker::popup(&response, popup_id, color, true) {
        response.mark_changed();
    }
    response
}

/// Draw `icon` into `area` (any square). `background` is what the icon sits on, used
/// where one part of the icon must hide another.
pub fn paint(painter: &egui::Painter, area: Rect, icon: Icon, color: Color32, background: Color32) {
    let unit = area.width() / GRID;
    let p = |x: f32, y: f32| area.min + vec2(x, y) * unit;
    let stroke = Stroke::new(STROKE, color);
    let line = |points: &[(f32, f32)]| {
        painter.add(Shape::line(points.iter().map(|&(x, y)| p(x, y)).collect(), stroke));
    };
    let triangle = |points: [(f32, f32); 3]| {
        let points = points.iter().map(|&(x, y)| p(x, y)).collect();
        painter.add(Shape::convex_polygon(points, color, Stroke::NONE));
    };

    match icon {
        Icon::Select => {
            // A mouse pointer.
            let outline = [
                (5.0, 2.5),
                (5.0, 16.5),
                (8.6, 13.1),
                (11.0, 18.3),
                (13.3, 17.3),
                (10.9, 12.1),
                (15.6, 12.1),
            ];
            let points: Vec<Pos2> = outline.iter().map(|&(x, y)| p(x, y)).collect();
            painter.add(Shape::closed_line(points, stroke));
        }
        Icon::Rect => {
            let r = Rect::from_min_max(p(3.0, 5.0), p(17.0, 15.0));
            painter.rect_stroke(r, 1.0, stroke, StrokeKind::Middle);
        }
        Icon::Ellipse => {
            painter.add(egui::epaint::EllipseShape::stroke(p(10.0, 10.0), vec2(7.5, 5.5) * unit, stroke));
        }
        Icon::Line => line(&[(3.5, 16.5), (16.5, 3.5)]),
        Icon::Arrow => {
            line(&[(3.5, 16.5), (13.0, 7.0)]);
            triangle([(17.0, 3.0), (9.6, 5.4), (14.6, 10.4)]);
        }
        Icon::Pen => {
            // A pencil, tip at the bottom left.
            let body = [(13.0, 3.0), (17.0, 7.0), (7.0, 17.0), (3.0, 17.0), (3.0, 13.0)];
            painter.add(Shape::closed_line(body.iter().map(|&(x, y)| p(x, y)).collect(), stroke));
            line(&[(11.0, 5.0), (15.0, 9.0)]);
            line(&[(3.0, 13.0), (7.0, 17.0)]);
        }
        Icon::Marker => {
            // A highlighter with a chisel tip, over a translucent stroke.
            // The stripe shows the highlighter colour, so the icon reads as a highlighter.
            let swatch = Rect::from_min_max(p(2.0, 15.5), p(18.0, 19.0));
            let ink = painter
                .ctx()
                .data(|d| d.get_temp::<Color32>(marker_color_id()))
                .unwrap_or(crate::settings::DEFAULT_MARKER_COLOR);
            painter.rect_filled(swatch, 1.0, ink);
            let body = [(12.0, 1.5), (17.5, 7.0), (11.0, 13.5), (5.5, 8.0)];
            painter.add(Shape::closed_line(body.iter().map(|&(x, y)| p(x, y)).collect(), stroke));
            triangle([(5.5, 8.0), (11.0, 13.5), (4.0, 14.0)]);
        }
        Icon::Text => {
            // A serif-free "T".
            line(&[(4.0, 4.5), (16.0, 4.5)]);
            line(&[(10.0, 4.5), (10.0, 16.5)]);
            line(&[(7.5, 16.5), (12.5, 16.5)]);
        }
        Icon::Blur => effect_preview(painter, area.shrink(2.0 * unit), Effect::Pixelate, color),
        Icon::Step => {
            // A filled disc with "1" knocked out.
            painter.circle_filled(p(10.0, 10.0), 8.0 * unit, color);
            painter.text(
                p(10.0, 10.3),
                egui::Align2::CENTER_CENTER,
                "1",
                egui::FontId::proportional(11.0 * unit),
                background,
            );
        }
        Icon::Eyedropper => {
            // A pipette: bulb at the top right, tube running down to the tip.
            line(&[(4.0, 16.0), (12.0, 8.0)]);
            line(&[(9.5, 5.5), (14.5, 10.5)]);
            painter.circle_filled(p(14.5, 5.5), 3.2 * unit, color);
            painter.circle_filled(p(3.5, 16.5), 1.2 * unit, color);
        }
        Icon::DoubleHead => {
            line(&[(6.0, 10.0), (14.0, 10.0)]);
            triangle([(2.0, 10.0), (7.5, 6.0), (7.5, 14.0)]);
            triangle([(18.0, 10.0), (12.5, 6.0), (12.5, 14.0)]);
        }
        Icon::Undo | Icon::Redo => {
            // An arrow pointing left that loops back round on the right (mirrored for redo).
            let flip = |x: f32| if icon == Icon::Redo { GRID - x } else { x };
            let mut path = vec![(7.0, 6.0), (11.5, 6.0)];
            let (cx, cy, r) = (11.5, 10.75, 4.75);
            for i in 1..=12 {
                let a = -std::f32::consts::FRAC_PI_2 + std::f32::consts::PI * i as f32 / 12.0;
                path.push((cx + r * a.cos(), cy + r * a.sin()));
            }
            path.push((6.0, 15.5));
            let path: Vec<(f32, f32)> = path.into_iter().map(|(x, y)| (flip(x), y)).collect();
            line(&path);
            triangle([(flip(3.0), 6.0), (flip(7.5), 2.5), (flip(7.5), 9.5)]);
        }
        Icon::Copy => {
            // Two overlapping sheets; the front one hides the back one's corner.
            let back = Rect::from_min_max(p(7.0, 3.0), p(17.0, 13.0));
            let front = Rect::from_min_max(p(3.0, 7.0), p(13.0, 17.0));
            painter.rect_stroke(back, 1.5, stroke, StrokeKind::Middle);
            painter.rect_filled(front, 1.5, background);
            painter.rect_stroke(front, 1.5, stroke, StrokeKind::Middle);
        }
        Icon::Save => {
            // Download into a tray.
            line(&[(10.0, 2.5), (10.0, 12.0)]);
            line(&[(6.0, 8.5), (10.0, 12.5), (14.0, 8.5)]);
            line(&[(3.5, 12.5), (3.5, 16.5), (16.5, 16.5), (16.5, 12.5)]);
        }
        Icon::Close => {
            line(&[(5.0, 5.0), (15.0, 15.0)]);
            line(&[(15.0, 5.0), (5.0, 15.0)]);
        }
    }
}
