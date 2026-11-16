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
    /// A smiling face, for emoji stamps.
    Emoji,
    Eyedropper,
    /// An arrow with a head at both ends.
    DoubleHead,
    Undo,
    Redo,
    Decoration,
    ReadText,
    ReadQr,
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
        if let Some(option) = chosen
            && option != *value {
                *value = option;
                response.mark_changed();
            }
    }
    response
}

/// Preview of a stroke style: a short line drawn in it.
pub fn dash_preview(painter: &egui::Painter, area: Rect, dash: crate::annotate::Dash, color: Color32) {
    use crate::annotate::{Annotation, ShapeKind};
    Annotation::stroked(ShapeKind::Line, area.left_center(), area.right_center(), color, 2.0, dash)
        .paint(painter, |p| p, 1.0);
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
        let shades = [1.0, 0.5, 0.75, 0.5, 0.75, 1.0, 0.75, 1.0, 0.5];
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
    egui::Id::unique("duskshot-marker-colour")
}

/// The highlighter colour the highlighter icon shows.
/// Asks for another frame when it changed, so a colour picked this frame shows at once.
pub fn set_marker_color(ctx: &egui::Context, color: Color32) {
    let previous = ctx.data_mut(|d| d.get_temp::<Color32>(marker_color_id()));
    ctx.data_mut(|d| d.insert_temp(marker_color_id(), color));
    if previous.is_some() && previous != Some(color) {
        ctx.request_repaint();
    }
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
pub fn paint(
    painter: &egui::Painter,
    area: Rect,
    icon: Icon,
    color: Color32,
    _background: Color32,
) {
    let unit = area.width() / GRID;
    let p = |x: f32, y: f32| area.min + vec2(x, y) * unit;
    let stroke = Stroke::new(STROKE * unit, color);
    match icon {
        Icon::Select => {
            painter.add(Shape::line(
                vec![
                    p(5.0, 2.5),
                    p(5.0, 16.0),
                    p(8.5, 12.7),
                    p(11.0, 17.5),
                    p(13.0, 16.5),
                    p(10.6, 11.7),
                    p(15.5, 11.7),
                    p(5.0, 2.5),
                ],
                stroke,
            ));
        }
        Icon::Rect => {
            painter.rect_stroke(
                Rect::from_min_max(p(3.0, 5.0), p(17.0, 15.0)),
                1.2 * unit,
                stroke,
                StrokeKind::Middle,
            );
        }
        Icon::Ellipse => {
            painter.add(egui::epaint::EllipseShape::stroke(
                p(10.0, 10.0),
                vec2(7.5, 5.5) * unit,
                stroke,
            ));
        }
        Icon::Line => {
            painter.add(Shape::line(vec![p(3.5, 16.5), p(16.5, 3.5)], stroke));
        }
        Icon::Arrow => {
            painter.add(Shape::line(vec![p(3.5, 16.5), p(15.5, 4.5)], stroke));
            painter.add(Shape::line(
                vec![p(9.0, 4.0), p(16.0, 4.0), p(16.0, 11.0)],
                stroke,
            ));
        }
        Icon::Pen => {
            // A pen, tip at the bottom left: an outlined barrel with a band near the
            // top, and a filled tip.
            let tip = [p(9.92, 12.91), p(4.4, 15.6), p(7.09, 10.08)];
            painter.add(Shape::convex_polygon(tip.to_vec(), color, Stroke::NONE));
            painter.add(Shape::closed_line(
                vec![p(14.59, 2.59), p(17.41, 5.41), tip[0], tip[1], tip[2]],
                stroke,
            ));
            painter.add(Shape::line(vec![p(12.61, 4.57), p(15.43, 7.39)], stroke));
        }
        Icon::Marker => {
            let marker_ink = painter
                .ctx()
                .data(|d| d.get_temp::<Color32>(marker_color_id()))
                .unwrap_or(crate::settings::DEFAULT_MARKER_COLOR);
            painter.add(Shape::convex_polygon(
                vec![
                    p(7.2, 8.0),
                    p(13.0, 2.2),
                    p(14.0, 1.8),
                    p(15.0, 2.2),
                    p(18.0, 5.2),
                    p(18.4, 6.2),
                    p(18.0, 7.2),
                    p(12.2, 13.0),
                ],
                color,
                Stroke::NONE,
            ));
            painter.add(Shape::convex_polygon(
                vec![p(6.4, 8.8), p(11.4, 13.8), p(7.0, 15.0), p(3.8, 11.8)],
                color,
                Stroke::NONE,
            ));
            painter.add(Shape::convex_polygon(
                vec![p(3.4, 12.6), p(6.2, 15.4), p(4.5, 16.3), p(1.7, 15.5)],
                marker_ink,
                Stroke::NONE,
            ));
            painter.rect_filled(
                Rect::from_min_max(p(2.0, 18.0), p(18.0, 19.5)),
                0.75 * unit,
                marker_ink,
            );
        }
        Icon::Text => {
            // A "T" with serifs on both arms and the foot.
            painter.add(Shape::line(
                vec![p(3.5, 7.0), p(3.5, 4.0), p(16.5, 4.0), p(16.5, 7.0)],
                stroke,
            ));
            painter.add(Shape::line(vec![p(10.0, 4.0), p(10.0, 16.0)], stroke));
            painter.add(Shape::line(vec![p(7.0, 16.0), p(13.0, 16.0)], stroke));
        }
        Icon::Blur => {
            painter.rect_filled(
                Rect::from_min_max(p(3.0, 3.0), p(7.0, 7.0)),
                0.0 * unit,
                color,
            );
            painter.rect_filled(
                Rect::from_min_max(p(8.0, 3.0), p(12.0, 7.0)),
                0.0 * unit,
                color.gamma_multiply(0.5),
            );
            painter.rect_filled(
                Rect::from_min_max(p(13.0, 3.0), p(17.0, 7.0)),
                0.0 * unit,
                color.gamma_multiply(0.75),
            );
            painter.rect_filled(
                Rect::from_min_max(p(3.0, 8.0), p(7.0, 12.0)),
                0.0 * unit,
                color.gamma_multiply(0.5),
            );
            painter.rect_filled(
                Rect::from_min_max(p(8.0, 8.0), p(12.0, 12.0)),
                0.0 * unit,
                color.gamma_multiply(0.75),
            );
            painter.rect_filled(
                Rect::from_min_max(p(13.0, 8.0), p(17.0, 12.0)),
                0.0 * unit,
                color,
            );
            painter.rect_filled(
                Rect::from_min_max(p(3.0, 13.0), p(7.0, 17.0)),
                0.0 * unit,
                color.gamma_multiply(0.75),
            );
            painter.rect_filled(
                Rect::from_min_max(p(8.0, 13.0), p(12.0, 17.0)),
                0.0 * unit,
                color,
            );
            painter.rect_filled(
                Rect::from_min_max(p(13.0, 13.0), p(17.0, 17.0)),
                0.0 * unit,
                color.gamma_multiply(0.5),
            );
        }
        Icon::Step => {
            painter.circle_stroke(p(10.0, 10.0), 7.5 * unit, stroke);
            painter.add(Shape::line(
                vec![p(8.0, 7.0), p(10.0, 5.5), p(10.0, 14.5)],
                stroke,
            ));
            painter.add(Shape::line(vec![p(7.5, 14.5), p(12.5, 14.5)], stroke));
        }
        Icon::Emoji => {
            painter.circle_stroke(p(10.0, 10.0), 7.5 * unit, stroke);
            painter.circle_filled(p(7.4, 8.0), 1.0 * unit, color);
            painter.circle_filled(p(12.6, 8.0), 1.0 * unit, color);
            painter.add(Shape::line(
                vec![
                    p(13.80649, 11.775),
                    p(13.62194, 12.1264),
                    p(13.40504, 12.4588),
                    p(13.15773, 12.76925),
                    p(12.88221, 13.05497),
                    p(12.58096, 13.3134),
                    p(12.25666, 13.54224),
                    p(11.9122, 13.73945),
                    p(11.55067, 13.90326),
                    p(11.17528, 14.03221),
                    p(10.7894, 14.12515),
                    p(10.39647, 14.18125),
                    p(10.0, 14.2),
                    p(9.60353, 14.18125),
                    p(9.2106, 14.12515),
                    p(8.82472, 14.03221),
                    p(8.44933, 13.90326),
                    p(8.0878, 13.73945),
                    p(7.74334, 13.54224),
                    p(7.41904, 13.3134),
                    p(7.11779, 13.05497),
                    p(6.84227, 12.76925),
                    p(6.59496, 12.4588),
                    p(6.37806, 12.1264),
                    p(6.19351, 11.775),
                ],
                stroke,
            ));
        }
        Icon::Eyedropper => {
            painter.add(Shape::line(
                vec![
                    p(4.0, 13.0),
                    p(11.0, 6.0),
                    p(14.0, 9.0),
                    p(7.0, 16.0),
                    p(3.0, 17.0),
                    p(4.0, 13.0),
                ],
                stroke,
            ));
            painter.add(Shape::line(vec![p(9.0, 5.0), p(15.0, 11.0)], stroke));
            painter.add(Shape::convex_polygon(
                vec![
                    p(11.0, 5.0),
                    p(13.0, 3.0),
                    p(15.0, 2.5),
                    p(17.5, 5.0),
                    p(17.0, 7.0),
                    p(15.0, 9.0),
                ],
                color,
                Stroke::NONE,
            ));
        }
        Icon::DoubleHead => {
            painter.add(Shape::line(vec![p(3.0, 10.0), p(17.0, 10.0)], stroke));
            painter.add(Shape::line(
                vec![p(7.0, 6.0), p(3.0, 10.0), p(7.0, 14.0)],
                stroke,
            ));
            painter.add(Shape::line(
                vec![p(13.0, 6.0), p(17.0, 10.0), p(13.0, 14.0)],
                stroke,
            ));
        }
        Icon::Undo => {
            painter.add(Shape::line(
                vec![
                    p(6.0, 6.0),
                    p(11.5, 6.0),
                    p(12.12, 6.04064),
                    p(12.72939, 6.16185),
                    p(13.31775, 6.36157),
                    p(13.875, 6.63638),
                    p(14.39162, 6.98157),
                    p(14.85876, 7.39124),
                    p(15.26843, 7.85838),
                    p(15.61362, 8.375),
                    p(15.88843, 8.93225),
                    p(16.08815, 9.52061),
                    p(16.20936, 10.13),
                    p(16.25, 10.75),
                    p(16.20936, 11.37),
                    p(16.08815, 11.97939),
                    p(15.88843, 12.56775),
                    p(15.61362, 13.125),
                    p(15.26843, 13.64162),
                    p(14.85876, 14.10876),
                    p(14.39162, 14.51843),
                    p(13.875, 14.86362),
                    p(13.31775, 15.13843),
                    p(12.72939, 15.33815),
                    p(12.12, 15.45936),
                    p(11.5, 15.5),
                    p(6.0, 15.5),
                ],
                stroke,
            ));
            painter.add(Shape::line(
                vec![p(7.0, 2.5), p(3.5, 6.0), p(7.0, 9.5)],
                stroke,
            ));
        }
        Icon::Redo => {
            painter.add(Shape::line(
                vec![
                    p(14.0, 6.0),
                    p(8.5, 6.0),
                    p(7.88, 6.04064),
                    p(7.27061, 6.16185),
                    p(6.68225, 6.36157),
                    p(6.125, 6.63638),
                    p(5.60838, 6.98157),
                    p(5.14124, 7.39124),
                    p(4.73157, 7.85838),
                    p(4.38638, 8.375),
                    p(4.11157, 8.93225),
                    p(3.91185, 9.52061),
                    p(3.79064, 10.13),
                    p(3.75, 10.75),
                    p(3.79064, 11.37),
                    p(3.91185, 11.97939),
                    p(4.11157, 12.56775),
                    p(4.38638, 13.125),
                    p(4.73157, 13.64162),
                    p(5.14124, 14.10876),
                    p(5.60838, 14.51843),
                    p(6.125, 14.86362),
                    p(6.68225, 15.13843),
                    p(7.27061, 15.33815),
                    p(7.88, 15.45936),
                    p(8.5, 15.5),
                    p(14.0, 15.5),
                ],
                stroke,
            ));
            painter.add(Shape::line(
                vec![p(13.0, 2.5), p(16.5, 6.0), p(13.0, 9.5)],
                stroke,
            ));
        }
        Icon::Decoration => {
            painter.rect_stroke(
                Rect::from_min_max(p(2.5, 2.5), p(14.5, 14.5)),
                1.0 * unit,
                stroke,
                StrokeKind::Middle,
            );
            painter.add(Shape::line(
                vec![p(6.0, 17.5), p(17.5, 17.5), p(17.5, 6.0)],
                stroke,
            ));
            painter.add(Shape::line(
                vec![p(5.0, 12.0), p(8.0, 8.5), p(12.0, 12.0)],
                stroke,
            ));
            painter.circle_filled(p(11.0, 6.0), 1.0 * unit, color);
        }
        Icon::ReadText => {
            painter.add(Shape::line(
                vec![p(2.5, 6.5), p(2.5, 2.5), p(6.5, 2.5)],
                stroke,
            ));
            painter.add(Shape::line(
                vec![p(17.5, 6.5), p(17.5, 2.5), p(13.5, 2.5)],
                stroke,
            ));
            painter.add(Shape::line(
                vec![p(2.5, 13.5), p(2.5, 17.5), p(6.5, 17.5)],
                stroke,
            ));
            painter.add(Shape::line(
                vec![p(17.5, 13.5), p(17.5, 17.5), p(13.5, 17.5)],
                stroke,
            ));
            painter.add(Shape::line(vec![p(6.0, 7.0), p(14.0, 7.0)], stroke));
            painter.add(Shape::line(vec![p(6.0, 10.0), p(14.0, 10.0)], stroke));
            painter.add(Shape::line(vec![p(6.0, 13.0), p(11.0, 13.0)], stroke));
        }
        Icon::ReadQr => {
            painter.rect_stroke(
                Rect::from_min_max(p(3.0, 3.0), p(8.0, 8.0)),
                0.2 * unit,
                stroke,
                StrokeKind::Middle,
            );
            painter.rect_filled(
                Rect::from_min_max(p(4.75, 4.75), p(6.25, 6.25)),
                0.0 * unit,
                color,
            );
            painter.rect_stroke(
                Rect::from_min_max(p(12.0, 3.0), p(17.0, 8.0)),
                0.2 * unit,
                stroke,
                StrokeKind::Middle,
            );
            painter.rect_filled(
                Rect::from_min_max(p(13.75, 4.75), p(15.25, 6.25)),
                0.0 * unit,
                color,
            );
            painter.rect_stroke(
                Rect::from_min_max(p(3.0, 12.0), p(8.0, 17.0)),
                0.2 * unit,
                stroke,
                StrokeKind::Middle,
            );
            painter.rect_filled(
                Rect::from_min_max(p(4.75, 13.75), p(6.25, 15.25)),
                0.0 * unit,
                color,
            );
            painter.rect_filled(
                Rect::from_min_max(p(12.0, 12.0), p(14.0, 14.0)),
                0.0 * unit,
                color,
            );
            painter.rect_filled(
                Rect::from_min_max(p(15.0, 12.0), p(17.0, 14.0)),
                0.0 * unit,
                color,
            );
            painter.rect_filled(
                Rect::from_min_max(p(12.0, 15.0), p(14.0, 17.0)),
                0.0 * unit,
                color,
            );
            painter.rect_filled(
                Rect::from_min_max(p(15.0, 15.0), p(17.0, 17.0)),
                0.0 * unit,
                color,
            );
        }
        Icon::Copy => {
            painter.add(Shape::line(
                vec![
                    p(7.0, 5.0),
                    p(7.0, 3.0),
                    p(17.0, 3.0),
                    p(17.0, 13.0),
                    p(15.0, 13.0),
                ],
                stroke,
            ));
            painter.rect_stroke(
                Rect::from_min_max(p(3.0, 7.0), p(13.0, 17.0)),
                1.2 * unit,
                stroke,
                StrokeKind::Middle,
            );
        }
        Icon::Save => {
            painter.add(Shape::line(vec![p(10.0, 2.5), p(10.0, 12.0)], stroke));
            painter.add(Shape::line(
                vec![p(6.0, 8.0), p(10.0, 12.0), p(14.0, 8.0)],
                stroke,
            ));
            painter.add(Shape::line(
                vec![p(3.5, 12.5), p(3.5, 16.5), p(16.5, 16.5), p(16.5, 12.5)],
                stroke,
            ));
        }
        Icon::Close => {
            painter.add(Shape::line(vec![p(5.0, 5.0), p(15.0, 15.0)], stroke));
            painter.add(Shape::line(vec![p(15.0, 5.0), p(5.0, 15.0)], stroke));
        }
    }
}
