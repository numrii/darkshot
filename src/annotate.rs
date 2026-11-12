//! Annotations drawn on top of a capture: the shape model, hit-testing, on-screen
//! painting (egui), rasterising into the output image (tiny-skia), and undo history.
//!
//! All geometry is in desktop pixels, the same space the selection uses, so shapes
//! stay put when the capture region is moved or resized. Each shape is reduced to a
//! [`Geometry`] (stroked polylines, filled triangles, dots) that both renderers draw
//! the same way, so what is on screen is what gets copied or saved.

use std::time::{Duration, Instant};

use eframe::egui::{self, vec2, Color32, Pos2, Rect, Stroke, Vec2};
use image::RgbaImage;

pub const MIN_WIDTH: f32 = 1.0;
pub const MAX_WIDTH: f32 = 40.0;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum ShapeKind {
    Rect,
    Ellipse,
    Line,
    Arrow,
    /// Freehand pen stroke.
    Pen,
    /// Freehand highlighter: wide and translucent.
    Marker,
}

/// How see-through the highlighter is (multiplies the colour's own opacity).
const MARKER_OPACITY: f32 = 0.4;

/// How a stroke is broken up. Dash lengths scale with the stroke width.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Default, Hash)]
pub enum Dash {
    #[default]
    Solid,
    Dash,
    LongDash,
    DashDot,
    Dot,
}

impl Dash {
    pub const ALL: [(Dash, &'static str); 5] = [
        (Dash::Solid, "Solid"),
        (Dash::Dash, "Dashed"),
        (Dash::LongDash, "Long dash"),
        (Dash::DashDot, "Dash dot"),
        (Dash::Dot, "Dotted"),
    ];

    /// Alternating on/off lengths in pixels for stroke width `w`, or `None` for solid
    /// lines and dots. Minimum lengths keep thin strokes readable.
    fn pattern(self, w: f32) -> Option<Vec<f32>> {
        match self {
            Dash::Solid | Dash::Dot => None,
            Dash::Dash => Some(vec![(4.0 * w).max(6.0), (2.5 * w).max(4.0)]),
            Dash::LongDash => Some(vec![(9.0 * w).max(14.0), (3.0 * w).max(5.0)]),
            Dash::DashDot => {
                let gap = (2.0 * w).max(3.5);
                Some(vec![(6.0 * w).max(9.0), gap, w.max(1.5), gap])
            }
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Default, Hash)]
pub enum ArrowHead {
    #[default]
    Triangle,
    Open,
    Stealth,
    Dot,
}

impl ArrowHead {
    pub const ALL: [(ArrowHead, &'static str); 4] = [
        (ArrowHead::Triangle, "Triangle"),
        (ArrowHead::Open, "Open"),
        (ArrowHead::Stealth, "Stealth"),
        (ArrowHead::Dot, "Dot"),
    ];
}

/// One editable annotation. For boxes (`Rect`, `Ellipse`) `a` and `b` are opposite
/// corners; for `Line` and `Arrow` they are the start and the end (the arrow tip).
/// `Pen` and `Marker` strokes are freehand when `points` holds their path, and straight
/// from `a` to `b` (drawn with Shift) when it is empty.
#[derive(Clone, Debug, PartialEq)]
pub struct Annotation {
    pub kind: ShapeKind,
    pub a: Pos2,
    pub b: Pos2,
    pub color: Color32,
    /// Stroke width in pixels.
    pub width: f32,
    pub dash: Dash,
    /// Arrows only: the head's style, and whether the start gets one too.
    pub head: ArrowHead,
    pub double_head: bool,
    /// Freehand path, for `Pen` and `Marker`.
    pub points: Vec<Pos2>,
}

/// An open or closed run of connected points.
#[derive(Clone, Debug, PartialEq)]
pub struct Polyline {
    pub points: Vec<Pos2>,
    pub closed: bool,
}

/// What a shape is made of, ready to draw.
#[derive(Default, Debug)]
pub struct Geometry {
    /// Stroked with the shape's width, butt caps.
    pub strokes: Vec<Polyline>,
    /// Filled triangles (arrowheads).
    pub fills: Vec<[Pos2; 3]>,
    /// Filled circles: centre and radius.
    pub dots: Vec<(Pos2, f32)>,
    /// Corners should be sharp (rectangles) rather than rounded.
    pub miter: bool,
    /// Round line ends (pen and marker strokes) rather than flat ones.
    pub round_caps: bool,
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
        for p in &mut self.points {
            *p += d;
        }
    }

    /// A freehand pen or marker stroke (as opposed to a straight one).
    pub fn is_freehand(&self) -> bool {
        self.points.len() >= 2
    }

    /// The colour actually drawn: the highlighter is translucent.
    pub fn ink(&self) -> Color32 {
        if self.kind == ShapeKind::Marker {
            let [r, g, b, a] = self.color.to_srgba_unmultiplied();
            Color32::from_rgba_unmultiplied(r, g, b, (f32::from(a) * MARKER_OPACITY).round() as u8)
        } else {
            self.color
        }
    }

    /// Bounding box of the freehand path (or of the two end points).
    pub fn path_bounds(&self) -> Rect {
        if self.is_freehand() {
            Rect::from_points(&self.points)
        } else {
            self.rect()
        }
    }

    /// Too small to be a deliberate shape (a stray click rather than a drag).
    pub fn is_degenerate(&self) -> bool {
        if self.is_freehand() {
            let r = self.path_bounds();
            r.width() < 3.0 && r.height() < 3.0
        } else if self.is_box() {
            self.rect().width() < 3.0 && self.rect().height() < 3.0
        } else {
            self.a.distance(self.b) < 3.0
        }
    }

    /// The full, undashed path of the shape (for lines: end to end, ignoring heads).
    fn outline(&self) -> Polyline {
        match self.kind {
            ShapeKind::Rect => {
                let r = self.rect();
                Polyline {
                    points: vec![r.left_top(), r.right_top(), r.right_bottom(), r.left_bottom()],
                    closed: true,
                }
            }
            ShapeKind::Ellipse => {
                let r = self.rect();
                let (rx, ry) = (r.width() / 2.0, r.height() / 2.0);
                // Roughly one point per 3 px of circumference.
                let perimeter = std::f32::consts::TAU * ((rx * rx + ry * ry) / 2.0).sqrt();
                let n = ((perimeter / 3.0) as usize).clamp(32, 720);
                let points = (0..n)
                    .map(|i| {
                        let t = std::f32::consts::TAU * i as f32 / n as f32;
                        r.center() + vec2(rx * t.cos(), ry * t.sin())
                    })
                    .collect();
                Polyline { points, closed: true }
            }
            _ if self.is_freehand() => Polyline { points: self.points.clone(), closed: false },
            ShapeKind::Line | ShapeKind::Arrow | ShapeKind::Pen | ShapeKind::Marker => {
                Polyline { points: vec![self.a, self.b], closed: false }
            }
        }
    }

    pub fn geometry(&self) -> Geometry {
        let mut geo = Geometry {
            miter: self.kind == ShapeKind::Rect,
            round_caps: matches!(self.kind, ShapeKind::Pen | ShapeKind::Marker),
            ..Default::default()
        };
        let mut body = self.outline();

        if self.kind == ShapeKind::Arrow {
            let v = self.b - self.a;
            let len = v.length();
            if len >= 1.0 {
                let dir = v / len;
                let room = if self.double_head { len * 0.45 } else { len };
                let head_len = (self.width * 4.0 + 6.0).min(room);
                body.points[1] = head(&mut geo, self.b, dir, self.width, head_len, self.head);
                if self.double_head {
                    body.points[0] = head(&mut geo, self.a, -dir, self.width, head_len, self.head);
                }
            }
        }

        match (self.dash, self.dash.pattern(self.width)) {
            (Dash::Dot, _) => geo.dots.extend(dots_along(&body, self.width)),
            (_, Some(pattern)) => geo.strokes.extend(dashes_along(&body, &pattern)),
            _ => geo.strokes.push(body),
        }
        geo
    }

    /// Whether `p` is on the drawn shape, within `tol` pixels. Shapes are unfilled, so
    /// the inside of a box does not count: you can click through it. Gaps in dashed
    /// strokes count as part of the stroke, so dashed shapes are as easy to grab.
    pub fn hit(&self, p: Pos2, tol: f32) -> bool {
        let reach = self.width / 2.0 + tol;
        if distance_to_polyline(p, &self.outline()) <= reach {
            return true;
        }
        let geo = self.geometry();
        geo.fills.iter().any(|tri| in_triangle(p, *tri))
            || geo.dots.iter().any(|(c, r)| p.distance(*c) <= r + tol)
            || geo.strokes.iter().any(|s| distance_to_polyline(p, s) <= reach)
    }

    /// Draw on screen. `to_pt` maps desktop pixels to egui points; `scale` is pixels
    /// per point.
    pub fn paint(&self, painter: &egui::Painter, to_pt: impl Fn(Pos2) -> Pos2, scale: f32) {
        let geo = self.geometry();
        let ink = self.ink();
        let stroke = Stroke::new(self.width / scale, ink);
        for line in &geo.strokes {
            let points: Vec<Pos2> = line.points.iter().map(|&p| to_pt(p)).collect();
            painter.add(if line.closed {
                egui::Shape::closed_line(points, stroke)
            } else {
                egui::Shape::line(points, stroke)
            });
        }
        for tri in &geo.fills {
            painter.add(egui::Shape::convex_polygon(tri.map(&to_pt).to_vec(), ink, Stroke::NONE));
        }
        for &(centre, radius) in &geo.dots {
            painter.circle_filled(to_pt(centre), radius / scale, ink);
        }
        // egui has no round line caps; add them for opaque strokes. (Translucent ones
        // would show the overlap; finished freehand strokes are drawn from a
        // rasterised layer instead, see `layer`.)
        if geo.round_caps && ink.a() == 255 {
            for line in &geo.strokes {
                for end in [line.points.first(), line.points.last()].into_iter().flatten() {
                    painter.circle_filled(to_pt(*end), self.width / scale / 2.0, ink);
                }
            }
        }
    }

    /// Rasterise this annotation on its own, for showing freehand strokes on screen
    /// exactly as they will be saved. Returns the image and the desktop-pixel area it
    /// covers, clipped to `clip`.
    pub fn layer(&self, clip: Rect) -> Option<(egui::ColorImage, Rect)> {
        let area = self.path_bounds().expand(self.width + 2.0).intersect(clip);
        let area = Rect::from_min_max(area.min.floor(), area.max.ceil());
        let (w, h) = (area.width() as u32, area.height() as u32);
        let mut pixmap = tiny_skia::Pixmap::new(w, h)?;
        self.rasterize(&mut pixmap.as_mut(), area.min);
        let image = egui::ColorImage::from_rgba_premultiplied([w as usize, h as usize], pixmap.data());
        Some((image, area))
    }

    /// Draw into `pixmap`, whose top-left pixel is desktop position `origin`.
    fn rasterize(&self, pixmap: &mut tiny_skia::PixmapMut<'_>, origin: Pos2) {
        use tiny_skia::{FillRule, LineCap, LineJoin, Paint, PathBuilder, Transform};

        let geo = self.geometry();
        let [r, g, b, a] = self.ink().to_srgba_unmultiplied();
        let mut paint = Paint::default();
        paint.set_color_rgba8(r, g, b, a);
        paint.anti_alias = true;
        let stroke = tiny_skia::Stroke {
            width: self.width,
            line_cap: if geo.round_caps { LineCap::Round } else { LineCap::Butt },
            line_join: if geo.miter { LineJoin::Miter } else { LineJoin::Round },
            ..Default::default()
        };
        let shift = Transform::from_translate(-origin.x, -origin.y);

        // Everything of one kind goes into one path, so overlapping parts of a
        // translucent shape are not painted twice.
        let mut lines = PathBuilder::new();
        for line in &geo.strokes {
            let Some((first, rest)) = line.points.split_first() else { continue };
            lines.move_to(first.x, first.y);
            for p in rest {
                lines.line_to(p.x, p.y);
            }
            if line.closed {
                lines.close();
            }
        }
        if let Some(path) = lines.finish() {
            pixmap.stroke_path(&path, &paint, &stroke, shift, None);
        }

        let mut fills = PathBuilder::new();
        for tri in &geo.fills {
            fills.move_to(tri[0].x, tri[0].y);
            fills.line_to(tri[1].x, tri[1].y);
            fills.line_to(tri[2].x, tri[2].y);
            fills.close();
        }
        for &(c, radius) in &geo.dots {
            fills.push_circle(c.x, c.y, radius);
        }
        if let Some(path) = fills.finish() {
            pixmap.fill_path(&path, &paint, FillRule::Winding, shift, None);
        }
    }
}

/// Add an arrowhead pointing along `dir` with its tip at `tip`, and return where the
/// shaft should stop so it joins the head cleanly.
fn head(geo: &mut Geometry, tip: Pos2, dir: Vec2, width: f32, len: f32, kind: ArrowHead) -> Pos2 {
    let normal = vec2(-dir.y, dir.x);
    let base = tip - dir * len;
    let half = len * 0.45;
    let (left, right) = (base + normal * half, base - normal * half);
    // Shafts run slightly into solid heads so no seam shows between them.
    let overlap = dir * len.min(1.0);
    match kind {
        ArrowHead::Triangle => {
            geo.fills.push([tip, left, right]);
            base + overlap
        }
        ArrowHead::Stealth => {
            // A shallow notch keeps it reading as a solid dart, not an open chevron.
            let notch = tip - dir * len * 0.75;
            geo.fills.push([tip, left, notch]);
            geo.fills.push([tip, notch, right]);
            notch + overlap
        }
        ArrowHead::Open => {
            // Pull the wings in so the outside of the stroke, not its centre, meets
            // the tip.
            let tip_in = tip - dir * (width * 0.9);
            let back = tip_in - dir * len;
            geo.strokes.push(Polyline {
                points: vec![back + normal * half, tip_in, back - normal * half],
                closed: false,
            });
            tip_in
        }
        ArrowHead::Dot => {
            let radius = (width * 1.6).max(3.0);
            geo.dots.push((tip, radius));
            tip
        }
    }
}

/// A polyline's segments, including the closing one, with the distance along the
/// path at which each starts.
fn segments(line: &Polyline) -> (Vec<(Pos2, Pos2)>, Vec<f32>, f32) {
    let mut segs: Vec<(Pos2, Pos2)> = line.points.windows(2).map(|w| (w[0], w[1])).collect();
    if line.closed && line.points.len() > 2 {
        segs.push((*line.points.last().unwrap(), line.points[0]));
    }
    let mut starts = Vec::with_capacity(segs.len());
    let mut total = 0.0;
    for (a, b) in &segs {
        starts.push(total);
        total += a.distance(*b);
    }
    (segs, starts, total)
}

/// Cut `line` into dashes following `pattern` (alternating on/off lengths). The
/// pattern is stretched slightly so it fits a whole number of times: closed paths
/// repeat seamlessly and open paths start and end with a dash.
fn dashes_along(line: &Polyline, pattern: &[f32]) -> Vec<Polyline> {
    let (segs, starts, total) = segments(line);
    if total <= 0.0 {
        return Vec::new();
    }
    let period: f32 = pattern.iter().sum();
    let last_gap = *pattern.last().unwrap();
    let fit = if line.closed { total } else { total + last_gap };
    let repeats = (fit / period).round().max(1.0);
    let k = fit / (repeats * period);

    let point_at = |d: f32| -> (usize, Pos2) {
        let i = starts.partition_point(|&s| s <= d).saturating_sub(1).min(segs.len() - 1);
        let (a, b) = segs[i];
        let len = a.distance(b);
        let t = if len > 0.0 { ((d - starts[i]) / len).clamp(0.0, 1.0) } else { 0.0 };
        (i, a + (b - a) * t)
    };

    let mut dashes = Vec::new();
    let mut d = 0.0;
    'outer: loop {
        for (j, &len) in pattern.iter().enumerate() {
            let len = len * k;
            if j % 2 == 0 {
                let end = (d + len).min(total);
                let (i0, p0) = point_at(d);
                let (i1, p1) = point_at(end);
                let mut points = vec![p0];
                // Keep the corners the dash passes round.
                points.extend((i0 + 1..=i1).map(|i| segs[i].0));
                points.push(p1);
                dashes.push(Polyline { points, closed: false });
            }
            d += len;
            if d >= total - 0.01 {
                break 'outer;
            }
        }
    }
    dashes
}

/// Evenly spaced dots along `line` for a dotted stroke of width `w`.
fn dots_along(line: &Polyline, w: f32) -> Vec<(Pos2, f32)> {
    let (segs, starts, total) = segments(line);
    let radius = (w / 2.0).max(0.75);
    let spacing = (w * 2.2).max(3.0);
    if total <= 0.0 {
        return vec![(line.points[0], radius)];
    }
    // Closed paths: n dots spaced all the way round. Open: n + 1, one at each end.
    let n = (total / spacing).round().max(1.0) as usize;
    let count = if line.closed { n } else { n + 1 };
    let step = total / n as f32;
    (0..count)
        .map(|j| {
            let d = j as f32 * step;
            let i = starts.partition_point(|&s| s <= d).saturating_sub(1).min(segs.len() - 1);
            let (a, b) = segs[i];
            let len = a.distance(b);
            let t = if len > 0.0 { ((d - starts[i]) / len).clamp(0.0, 1.0) } else { 0.0 };
            (a + (b - a) * t, radius)
        })
        .collect()
}

/// Thin out a freehand path: drop points that deviate less than `tolerance` pixels
/// from the line through their neighbours (Ramer-Douglas-Peucker).
pub fn simplify(points: &[Pos2], tolerance: f32) -> Vec<Pos2> {
    if points.len() < 3 {
        return points.to_vec();
    }
    let mut keep = vec![false; points.len()];
    keep[0] = true;
    keep[points.len() - 1] = true;
    let mut stack = vec![(0, points.len() - 1)];
    while let Some((first, last)) = stack.pop() {
        let (mut worst, mut worst_d) = (0, 0.0);
        for i in first + 1..last {
            let d = distance_to_segment(points[i], points[first], points[last]);
            if d > worst_d {
                (worst, worst_d) = (i, d);
            }
        }
        if worst_d > tolerance {
            keep[worst] = true;
            stack.push((first, worst));
            stack.push((worst, last));
        }
    }
    points.iter().zip(keep).filter(|(_, k)| *k).map(|(p, _)| *p).collect()
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

fn distance_to_polyline(p: Pos2, line: &Polyline) -> f32 {
    let (segs, _, _) = segments(line);
    segs.iter()
        .map(|&(a, b)| distance_to_segment(p, a, b))
        .fold(f32::INFINITY, f32::min)
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
            dash: Dash::Solid,
            head: ArrowHead::Triangle,
            double_head: false,
            points: Vec::new(),
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

        // Gaps in a dashed stroke are still grabbable.
        let mut dashed = shape(ShapeKind::Line, (0.0, 0.0), (200.0, 0.0));
        dashed.dash = Dash::LongDash;
        assert!((0..200).all(|x| dashed.hit(Pos2::new(x as f32, 0.0), 0.0)));
    }

    #[test]
    fn dashes_fit_the_path() {
        let mut line = shape(ShapeKind::Line, (0.0, 0.0), (300.0, 0.0));
        line.dash = Dash::Dash;
        let geo = line.geometry();
        assert!(geo.strokes.len() > 5);
        // Open paths start and end with a dash.
        assert_eq!(geo.strokes[0].points[0], Pos2::new(0.0, 0.0));
        let last = geo.strokes.last().unwrap().points.last().unwrap();
        assert!((last.x - 300.0).abs() < 0.1, "{last:?}");
        // Dashes are separated by gaps.
        for pair in geo.strokes.windows(2) {
            let gap = pair[1].points[0].x - pair[0].points.last().unwrap().x;
            assert!(gap > 5.0, "{gap}");
        }

        // A dashed rectangle keeps its corners inside dashes that wrap round them.
        let mut rect = shape(ShapeKind::Rect, (0.0, 0.0), (100.0, 60.0));
        rect.dash = Dash::LongDash;
        let geo = rect.geometry();
        let covered: f32 = geo
            .strokes
            .iter()
            .flat_map(|s| s.points.windows(2).map(|w| w[0].distance(w[1])))
            .sum();
        assert!(covered > 200.0 && covered < 320.0, "{covered}"); // perimeter 320 minus gaps

        // Dotted: evenly spaced, one at each end of an open line.
        let mut dotted = shape(ShapeKind::Line, (0.0, 0.0), (100.0, 0.0));
        dotted.dash = Dash::Dot;
        let geo = dotted.geometry();
        assert!(geo.strokes.is_empty());
        assert_eq!(geo.dots.first().unwrap().0, Pos2::new(0.0, 0.0));
        assert!((geo.dots.last().unwrap().0.x - 100.0).abs() < 0.01);
        let spacing: Vec<f32> = geo.dots.windows(2).map(|w| w[1].0.x - w[0].0.x).collect();
        assert!(spacing.iter().all(|s| (s - spacing[0]).abs() < 0.01));
    }

    #[test]
    fn arrowheads() {
        for (head, _) in ArrowHead::ALL {
            let mut arrow = shape(ShapeKind::Arrow, (0.0, 0.0), (100.0, 0.0));
            arrow.head = head;
            arrow.double_head = true;
            let geo = arrow.geometry();
            // Both ends are clickable, and so is the middle of the shaft.
            for x in [1.0, 50.0, 99.0] {
                assert!(arrow.hit(Pos2::new(x, 0.0), 1.0), "{head:?} at {x}");
            }
            match head {
                ArrowHead::Triangle => assert_eq!(geo.fills.len(), 2),
                ArrowHead::Stealth => assert_eq!(geo.fills.len(), 4),
                ArrowHead::Open => assert_eq!(geo.strokes.len(), 3), // two heads + shaft
                ArrowHead::Dot => assert_eq!(geo.dots.len(), 2),
            }
            // The shaft never pokes out past either tip.
            let shaft = geo.strokes.iter().find(|s| s.points.len() == 2).unwrap();
            assert!(shaft.points.iter().all(|p| (0.0..=100.0).contains(&p.x)));
        }
    }

    #[test]
    fn freehand_strokes() {
        // A wavy pen stroke: hit along its path, moved by translate, simplified.
        let wave: Vec<Pos2> =
            (0..=200).map(|x| Pos2::new(x as f32, 50.0 + 10.0 * (x as f32 / 20.0).sin())).collect();
        let mut pen = shape(ShapeKind::Pen, (0.0, 0.0), (0.0, 0.0));
        pen.points = simplify(&wave, 0.5);
        assert!(pen.points.len() < 80 && pen.points.len() > 10, "{}", pen.points.len());
        assert!(pen.hit(Pos2::new(100.0, 50.0 + 10.0 * 5.0f32.sin()), 1.0));
        assert!(!pen.hit(Pos2::new(100.0, 90.0), 1.0));
        pen.translate(vec2(10.0, 0.0));
        assert_eq!(pen.points[0], Pos2::new(10.0, 50.0));
        assert!(pen.geometry().round_caps);

        // The marker is translucent; a straight one (no points) runs from a to b.
        let mut marker = shape(ShapeKind::Marker, (0.0, 10.0), (100.0, 10.0));
        marker.width = 16.0;
        assert!(!marker.is_freehand());
        assert_eq!(marker.ink().a(), 102);
        assert!(marker.hit(Pos2::new(50.0, 16.0), 0.0));

        // Its on-screen layer matches the output: uniform translucency, even where a
        // freehand stroke crosses itself.
        marker.points = vec![
            Pos2::new(0.0, 10.0),
            Pos2::new(60.0, 10.0),
            Pos2::new(60.0, 40.0),
            Pos2::new(30.0, 40.0),
            Pos2::new(30.0, 0.0),
        ];
        let clip = Rect::from_min_size(Pos2::ZERO, vec2(500.0, 500.0));
        let (layer, area) = marker.layer(clip).unwrap();
        let at = |x: f32, y: f32| {
            layer.pixels[(y - area.min.y) as usize * layer.size[0] + (x - area.min.x) as usize]
        };
        assert_eq!(at(30.0, 10.0).a(), at(10.0, 10.0).a()); // crossing vs single pass
        assert_eq!(at(45.0, 25.0).a(), 0); // inside the loop, not covered
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
        list.push(a.clone());
        history.record(&list, None);
        list.push(b.clone());
        assert!(history.undo(&mut list));
        assert_eq!(list, vec![a.clone()]);
        assert!(history.redo(&mut list));
        assert_eq!(list, vec![a.clone(), b.clone()]);

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

        // A dashed line leaves gaps in the output.
        let mut img = RgbaImage::from_pixel(200, 10, white);
        let mut dashed = shape(ShapeKind::Line, (0.0, 5.0), (200.0, 5.0));
        dashed.dash = Dash::Dash;
        render_onto(&mut img, Pos2::ZERO, &[dashed]);
        let red = (0..200).filter(|&x| img.get_pixel(x, 5).0 == [255, 0, 0, 255]).count();
        assert!(red > 90 && red < 150, "{red}");
    }
}

/// Writes a sheet of every stroke style and arrowhead, drawn through the real output
/// path, for eyeballing: `DARKSHOT_PREVIEW=out.png cargo test style_sheet -- --ignored`.
#[cfg(test)]
#[test]
#[ignore]
fn style_sheet() {
    let Some(path) = std::env::var_os("DARKSHOT_PREVIEW") else { return };
    let mut img = RgbaImage::from_pixel(560, 400, image::Rgba([246, 246, 246, 255]));
    let mut shapes = Vec::new();
    let base = |kind, a: (f32, f32), b: (f32, f32), width| Annotation {
        kind,
        a: Pos2::new(a.0, a.1),
        b: Pos2::new(b.0, b.1),
        color: Color32::from_rgb(0xE5, 0x48, 0x4D),
        width,
        dash: Dash::Solid,
        head: ArrowHead::Triangle,
        double_head: false,
        points: Vec::new(),
    };
    for (row, (dash, _)) in Dash::ALL.iter().enumerate() {
        let y = 20.0 + row as f32 * 34.0;
        for (col, width) in [2.0, 5.0].into_iter().enumerate() {
            let mut line = base(ShapeKind::Line, (20.0 + col as f32 * 140.0, y), (140.0 + col as f32 * 140.0, y), width);
            line.dash = *dash;
            shapes.push(line);
        }
        let mut rect = base(ShapeKind::Rect, (310.0, y - 12.0), (400.0, y + 14.0), 3.0);
        rect.dash = *dash;
        rect.color = Color32::from_rgb(0x3E, 0x63, 0xDD);
        shapes.push(rect);
        let mut ellipse = base(ShapeKind::Ellipse, (420.0, y - 12.0), (540.0, y + 14.0), 3.0);
        ellipse.dash = *dash;
        ellipse.color = Color32::from_rgb(0x30, 0xA4, 0x6C);
        shapes.push(ellipse);
    }
    for (row, (head, _)) in ArrowHead::ALL.iter().enumerate() {
        let y = 210.0 + row as f32 * 48.0;
        for (col, (width, double)) in [(3.0, false), (6.0, false), (3.0, true)].into_iter().enumerate() {
            let x = 20.0 + col as f32 * 180.0;
            let mut arrow = base(ShapeKind::Arrow, (x, y + 18.0), (x + 150.0, y), width);
            arrow.head = *head;
            arrow.double_head = double;
            if col == 2 {
                arrow.dash = Dash::Dash;
            }
            shapes.push(arrow);
        }
    }
    render_onto(&mut img, Pos2::ZERO, &shapes);
    img.save(path).unwrap();
}
