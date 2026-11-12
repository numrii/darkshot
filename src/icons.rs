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
