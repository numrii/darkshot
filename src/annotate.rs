//! Annotations drawn on top of a capture: the shape model, hit-testing, on-screen
//! painting (egui), rasterising into the output image (tiny-skia), and undo history.
//!
//! All geometry is in desktop pixels, the same space the selection uses, so shapes
//! stay put when the capture region is moved or resized.

use std::time::{Duration, Instant};

use eframe::egui::{self, epaint, vec2, Color32, Pos2, Rect, Stroke, StrokeKind, Vec2};
use image::RgbaImage;

pub const MIN_WIDTH: f32 = 1.0;
pub const MAX_WIDTH: f32 = 40.0;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ShapeKind {
    Rect,
    Ellipse,
    Line,
    Arrow,
}

/// One editable annotation. For boxes (`Rect`, `Ellipse`) `a` and `b` are opposite
/// corners; for `Line` and `Arrow` they are the start and the end (the arrow tip).
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Annotation {
    pub kind: ShapeKind,
    pub a: Pos2,
    pub b: Pos2,
    pub color: Color32,
    /// Stroke width in pixels.
    pub width: f32,
}

impl Annotation {
    pub fn is_box(&self) -> bool {
        matches!(self.kind, ShapeKind::Rect | ShapeKind::Ellipse)
    }

    pub fn rect(&self) -> Rect {
        Rect::from_two_pos(self.a, self.b)
    }

    pub fn set_rect(&mut self, r: Rect) {
        self.a = r.min;
        self.b = r.max;
    }

    pub fn translate(&mut self, d: Vec2) {
        self.a += d;
        self.b += d;
    }

    /// Too small to be a deliberate shape (a stray click rather than a drag).
    pub fn is_degenerate(&self) -> bool {
        if self.is_box() {
            self.rect().width() < 3.0 && self.rect().height() < 3.0
        } else {
            self.a.distance(self.b) < 3.0
        }
    }

    /// Whether `p` is on the drawn stroke, within `tol` pixels. Shapes are unfilled,
    /// so the inside of a box does not count: you can click through it.
    pub fn hit(&self, p: Pos2, tol: f32) -> bool {
        let reach = self.width / 2.0 + tol;
        match self.kind {
            ShapeKind::Rect => {
                let r = self.rect();
                let inner = r.shrink(reach);
                let hollow = inner.width() > 0.0 && inner.height() > 0.0 && inner.contains(p);
                r.expand(reach).contains(p) && !hollow
            }
            ShapeKind::Ellipse => {
                let r = self.rect();
                let (rx, ry) = (r.width() / 2.0, r.height() / 2.0);
                if rx < 1.0 || ry < 1.0 {
                    return r.expand(reach).contains(p);
                }
                let d = p - r.center();
                // k = 1 exactly on the ellipse; d / k is the boundary point along the ray.
                let k = ((d.x / rx).powi(2) + (d.y / ry).powi(2)).sqrt();
                if k < 1e-3 {
                    return rx.min(ry) <= reach;
                }
                d.length() * (1.0 - 1.0 / k).abs() <= reach
            }
            ShapeKind::Line | ShapeKind::Arrow => {
                let head = self.arrow_head().is_some_and(|(_, tri)| in_triangle(p, tri));
                head || distance_to_segment(p, self.a, self.b) <= reach
            }
        }
    }

    /// For arrows: where the shaft stops and the three corners of the head.
    pub fn arrow_head(&self) -> Option<(Pos2, [Pos2; 3])> {
        if self.kind != ShapeKind::Arrow {
            return None;
        }
        let v = self.b - self.a;
        let len = v.length();
        if len < 1.0 {
            return None;
        }
        let dir = v / len;
        let normal = vec2(-dir.y, dir.x);
        let head_len = (self.width * 4.0 + 6.0).min(len);
        let base = self.b - dir * head_len;
        let half = head_len * 0.45;
        // The shaft runs slightly into the head so no gap shows between them.
        let shaft_end = base + dir * head_len.min(1.0);
        Some((shaft_end, [self.b, base + normal * half, base - normal * half]))
    }

    /// Draw on screen. `to_pt` maps desktop pixels to egui points; `scale` is pixels
    /// per point.
    pub fn paint(&self, painter: &egui::Painter, to_pt: impl Fn(Pos2) -> Pos2, scale: f32) {
        let stroke = Stroke::new(self.width / scale, self.color);
        match self.kind {
            ShapeKind::Rect => {
                let r = self.rect();
                let r = Rect::from_min_max(to_pt(r.min), to_pt(r.max));
                painter.rect_stroke(r, 0.0, stroke, StrokeKind::Middle);
            }
            ShapeKind::Ellipse => {
                let r = self.rect();
                let r = Rect::from_min_max(to_pt(r.min), to_pt(r.max));
                painter.add(epaint::EllipseShape::stroke(r.center(), r.size() / 2.0, stroke));
            }
            ShapeKind::Line => {
                painter.line_segment([to_pt(self.a), to_pt(self.b)], stroke);
            }
            ShapeKind::Arrow => match self.arrow_head() {
                Some((shaft_end, tri)) => {
                    painter.line_segment([to_pt(self.a), to_pt(shaft_end)], stroke);
                    painter.add(egui::Shape::convex_polygon(
                        tri.map(&to_pt).to_vec(),
                        self.color,
                        Stroke::NONE,
                    ));
                }
                None => {}
            },
        }
    }

    /// Draw into `pixmap`, whose top-left pixel is desktop position `origin`.
    fn rasterize(&self, pixmap: &mut tiny_skia::PixmapMut<'_>, origin: Pos2) {
        use tiny_skia::{FillRule, LineCap, LineJoin, Paint, PathBuilder, Transform};

        let [r, g, b, a] = self.color.to_srgba_unmultiplied();
        let mut paint = Paint::default();
        paint.set_color_rgba8(r, g, b, a);
        paint.anti_alias = true;
        let stroke = tiny_skia::Stroke {
            width: self.width,
            line_cap: LineCap::Butt,
            line_join: LineJoin::Miter,
            ..Default::default()
        };
        let shift = Transform::from_translate(-origin.x, -origin.y);
        let segment = |from: Pos2, to: Pos2| {
            let mut pb = PathBuilder::new();
            pb.move_to(from.x, from.y);
            pb.line_to(to.x, to.y);
            pb.finish()
        };

        match self.kind {
            ShapeKind::Rect | ShapeKind::Ellipse => {
                let r = self.rect();
                let Some(bounds) = tiny_skia::Rect::from_ltrb(r.min.x, r.min.y, r.max.x, r.max.y)
                else {
                    return;
                };
                let path = if self.kind == ShapeKind::Rect {
                    Some(PathBuilder::from_rect(bounds))
                } else {
                    PathBuilder::from_oval(bounds)
                };
                if let Some(path) = path {
                    pixmap.stroke_path(&path, &paint, &stroke, shift, None);
                }
            }
            ShapeKind::Line => {
                if let Some(path) = segment(self.a, self.b) {
                    pixmap.stroke_path(&path, &paint, &stroke, shift, None);
                }
            }
            ShapeKind::Arrow => {
                let Some((shaft_end, tri)) = self.arrow_head() else { return };
                if let Some(path) = segment(self.a, shaft_end) {
                    pixmap.stroke_path(&path, &paint, &stroke, shift, None);
                }
                let mut pb = PathBuilder::new();
                pb.move_to(tri[0].x, tri[0].y);
                pb.line_to(tri[1].x, tri[1].y);
                pb.line_to(tri[2].x, tri[2].y);
                pb.close();
                if let Some(path) = pb.finish() {
                    pixmap.fill_path(&path, &paint, FillRule::Winding, shift, None);
                }
            }
        }
    }
}

/// Draw `annotations` onto `img`, whose top-left pixel is desktop position `origin`.
pub fn render_onto(img: &mut RgbaImage, origin: Pos2, annotations: &[Annotation]) {
    if annotations.is_empty() {
        return;
    }
    let (w, h) = img.dimensions();
    // tiny-skia works on premultiplied pixels. Screenshot pixels are either opaque or
    // fully transparent (areas off every monitor), and both are identical in the two
    // encodings, so the buffer can be drawn on in place...
    {
        let Some(mut pixmap) = tiny_skia::PixmapMut::from_bytes(img.as_mut(), w, h) else {
            return;
        };
        for annotation in annotations {
            annotation.rasterize(&mut pixmap, origin);
        }
    }
    // ...and only pixels left partly transparent by drawing need converting back.
    for px in img.pixels_mut() {
        let a = px.0[3];
        if a != 0 && a != 255 {
            for c in &mut px.0[..3] {
                *c = ((u32::from(*c) * 255 + u32::from(a) / 2) / u32::from(a)).min(255) as u8;
            }
        }
    }
}

/// `p` moved onto the nearest 45-degree direction from `origin` (Shift while drawing lines).
pub fn snap_angle(origin: Pos2, p: Pos2) -> Pos2 {
    let v = p - origin;
    let step = std::f32::consts::FRAC_PI_4;
    let angle = (v.y.atan2(v.x) / step).round() * step;
    origin + vec2(angle.cos(), angle.sin()) * v.length()
}

fn distance_to_segment(p: Pos2, a: Pos2, b: Pos2) -> f32 {
    let ab = b - a;
    let len_sq = ab.length_sq();
    if len_sq < 1e-6 {
        return p.distance(a);
    }
    let t = ((p - a).dot(ab) / len_sq).clamp(0.0, 1.0);
    p.distance(a + ab * t)
}

fn in_triangle(p: Pos2, [a, b, c]: [Pos2; 3]) -> bool {
    let side = |u: Pos2, v: Pos2| (v.x - u.x) * (p.y - u.y) - (v.y - u.y) * (p.x - u.x);
    let (d1, d2, d3) = (side(a, b), side(b, c), side(c, a));
    let negative = d1 < 0.0 || d2 < 0.0 || d3 < 0.0;
    let positive = d1 > 0.0 || d2 > 0.0 || d3 > 0.0;
    !(negative && positive)
}

/// What kind of small, repeated edit a history entry came from. Consecutive edits of
/// the same kind in quick succession share one undo step (a wheel spin, held arrow key).
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Repeat {
    Style,
    Nudge,
}

const COALESCE_WINDOW: Duration = Duration::from_millis(700);

/// Snapshot-based undo/redo over the annotation list.
#[derive(Default)]
pub struct History {
    undo: Vec<Vec<Annotation>>,
    redo: Vec<Vec<Annotation>>,
    last_repeat: Option<(Repeat, Instant)>,
}

impl History {
    /// Call with the state *before* a change. `repeat` marks edits that may merge
    /// with the previous one.
    pub fn record(&mut self, before: &[Annotation], repeat: Option<Repeat>) {
        let now = Instant::now();
        let merge = match (repeat, self.last_repeat) {
            (Some(kind), Some((last, at))) => kind == last && now.duration_since(at) < COALESCE_WINDOW,
            _ => false,
        };
        self.last_repeat = repeat.map(|kind| (kind, now));
        if merge {
            return;
        }
        self.undo.push(before.to_vec());
        self.redo.clear();
    }

    /// Drop the most recent entry (for a change that was abandoned, e.g. a stray click).
    pub fn forget_last(&mut self) {
        self.undo.pop();
    }

    pub fn can_undo(&self) -> bool {
        !self.undo.is_empty()
    }

    pub fn can_redo(&self) -> bool {
        !self.redo.is_empty()
    }

    pub fn undo(&mut self, current: &mut Vec<Annotation>) -> bool {
        self.last_repeat = None;
        let Some(previous) = self.undo.pop() else { return false };
        self.redo.push(std::mem::replace(current, previous));
        true
    }

    pub fn redo(&mut self, current: &mut Vec<Annotation>) -> bool {
        self.last_repeat = None;
        let Some(next) = self.redo.pop() else { return false };
        self.undo.push(std::mem::replace(current, next));
        true
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn shape(kind: ShapeKind, a: (f32, f32), b: (f32, f32)) -> Annotation {
        Annotation {
            kind,
            a: Pos2::new(a.0, a.1),
            b: Pos2::new(b.0, b.1),
            color: Color32::from_rgb(255, 0, 0),
            width: 4.0,
        }
    }

    #[test]
    fn hit_tests_follow_the_stroke() {
        let rect = shape(ShapeKind::Rect, (10.0, 10.0), (110.0, 60.0));
        assert!(rect.hit(Pos2::new(10.0, 30.0), 2.0)); // on the left edge
        assert!(rect.hit(Pos2::new(60.0, 62.0), 2.0)); // just outside the bottom edge
        assert!(!rect.hit(Pos2::new(60.0, 35.0), 2.0)); // hollow middle
        assert!(!rect.hit(Pos2::new(130.0, 35.0), 2.0)); // well outside

        let ellipse = shape(ShapeKind::Ellipse, (0.0, 0.0), (200.0, 100.0));
        assert!(ellipse.hit(Pos2::new(200.0, 50.0), 2.0)); // rightmost point
        assert!(ellipse.hit(Pos2::new(100.0, 1.0), 2.0)); // top
        assert!(!ellipse.hit(Pos2::new(100.0, 50.0), 2.0)); // centre
        assert!(!ellipse.hit(Pos2::new(2.0, 2.0), 2.0)); // bounding-box corner, off the curve

        let line = shape(ShapeKind::Line, (0.0, 0.0), (100.0, 100.0));
        assert!(line.hit(Pos2::new(50.0, 52.0), 2.0));
        assert!(!line.hit(Pos2::new(50.0, 70.0), 2.0));
        assert!(!line.hit(Pos2::new(120.0, 120.0), 2.0)); // beyond the end

        let arrow = shape(ShapeKind::Arrow, (0.0, 0.0), (100.0, 0.0));
        assert!(arrow.hit(Pos2::new(85.0, 6.0), 0.0)); // inside the head, off the shaft
    }

    #[test]
    fn shift_snaps_lines_to_45_degrees() {
        let o = Pos2::new(100.0, 100.0);
        let near_horizontal = snap_angle(o, Pos2::new(200.0, 110.0));
        assert!((near_horizontal.y - 100.0).abs() < 0.01 && near_horizontal.x > 199.0);
        let near_diagonal = snap_angle(o, Pos2::new(180.0, 30.0));
        let d = near_diagonal - o;
        assert!((d.x + d.y).abs() < 0.01 && d.x > 0.0); // up-right: dx == -dy
    }

    #[test]
    fn undo_redo_and_coalescing() {
        let a = shape(ShapeKind::Rect, (0.0, 0.0), (10.0, 10.0));
        let b = shape(ShapeKind::Line, (0.0, 0.0), (10.0, 10.0));
        let mut list = Vec::new();
        let mut history = History::default();

        history.record(&list, None);
        list.push(a);
        history.record(&list, None);
        list.push(b);
        assert!(history.undo(&mut list));
        assert_eq!(list, vec![a]);
        assert!(history.redo(&mut list));
        assert_eq!(list, vec![a, b]);

        // Three quick nudges collapse into one undo step.
        for _ in 0..3 {
            history.record(&list, Some(Repeat::Nudge));
            list[1].translate(vec2(1.0, 0.0));
        }
        assert!(history.undo(&mut list));
        assert_eq!(list, vec![a, b]);

        // A new edit discards the redo branch.
        history.record(&list, None);
        list.clear();
        assert!(!history.can_redo());
        assert!(history.undo(&mut list) && history.undo(&mut list) && history.undo(&mut list));
        assert!(list.is_empty() && !history.can_undo());
    }

    #[test]
    fn renders_into_the_output_image() {
        let white = image::Rgba([255, 255, 255, 255]);
        let mut img = RgbaImage::from_pixel(60, 40, white);
        // Image origin is desktop (100, 200); a red rectangle outline inside it.
        let rect = shape(ShapeKind::Rect, (110.0, 210.0), (150.0, 230.0));
        render_onto(&mut img, Pos2::new(100.0, 200.0), &[rect]);
        assert_eq!(img.get_pixel(10, 20).0, [255, 0, 0, 255]); // left edge of the outline
        assert_eq!(img.get_pixel(30, 20).0, [255, 255, 255, 255]); // hollow middle
        assert_eq!(img.get_pixel(2, 2).0, [255, 255, 255, 255]); // outside

        // Drawing over a transparent area yields straight (un-premultiplied) colour.
        let mut clear = RgbaImage::new(20, 20);
        let mut line = shape(ShapeKind::Line, (0.0, 10.0), (20.0, 10.0));
        line.color = Color32::from_rgba_unmultiplied(255, 0, 0, 128);
        render_onto(&mut clear, Pos2::ZERO, &[line]);
        let px = clear.get_pixel(10, 10).0;
        assert!(px[0] >= 250 && px[1] == 0 && (120..=136).contains(&px[3]), "{px:?}");
        assert_eq!(clear.get_pixel(10, 2).0, [0, 0, 0, 0]);
    }
}
