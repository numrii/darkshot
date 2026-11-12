//! Toolbar icons, drawn with vector shapes so they stay sharp and need no icon font
//! or image files.

use eframe::egui::{self, vec2, Color32, Pos2, Rect, Response, Sense, Shape, Stroke, StrokeKind, Vec2};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Icon {
    Select,
    Rect,
    Ellipse,
    Line,
    Arrow,
    /// An arrow with a head at both ends.
    DoubleHead,
    Undo,
    Redo,
    Copy,
    Save,
    Close,
}

const BUTTON: f32 = 28.0;
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
    let (rect, mut response) = ui.allocate_exact_size(vec2(44.0, BUTTON), sense);

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

/// A colour-wheel button that opens a full colour picker (RGB, HSV and hex). The
/// centre dot shows the current colour.
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

    egui::Popup::menu(&response)
        .id(popup_id)
        .close_behavior(egui::PopupCloseBehavior::CloseOnClickOutside)
        .show(|ui| {
            ui.spacing_mut().slider_width = 275.0;
            let alpha = egui::widgets::color_picker::Alpha::OnlyBlend;
            if egui::widgets::color_picker::color_picker_color32(ui, color, alpha) {
                response.mark_changed();
            }
        });
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
