//! Emoji stamps, drawn from Segoe UI Emoji, the colour emoji font that comes with
//! Windows. Its emoji are vector shapes (OpenType `COLR` layers), so a stamp is drawn
//! sharp at any size, on screen and in the copied or saved image alike. Sequences such
//! as skin tones, flags and families are put together by harfrust, the text shaper egui
//! already uses.

use std::sync::OnceLock;

use eframe::egui::{Pos2, Rect, Vec2};
use tiny_skia::{
    BlendMode, Color, FillRule, GradientStop, Mask, Paint, Path, PathBuilder, Pixmap, PixmapPaint, Point, Shader,
    SpreadMode, Transform,
};
use ttf_parser::colr::{self, CompositeMode, GradientExtend};
use ttf_parser::{Face, GlyphId, RgbaColor};

/// The emoji the picker offers, a handful of the most useful for pointing things out
/// in a screenshot. Any other can be typed or picked with Windows' emoji panel (Win+.).
pub const FAVOURITES: [&str; 40] = [
    "👍", "👎", "👌", "👉", "👈", "👆", "👇", "👀", "✅", "❌", "⚠️", "❗", "❓", "💡", "🔥", "⭐",
    "❤️", "💯", "🎉", "🚀", "🐛", "🔒", "🔑", "📌", "📎", "🗑️", "⏰", "🛑", "🆕", "🆗", "😀", "😂",
    "😍", "🤔", "😕", "😬", "😱", "🙏", "👏", "🤷",
];
pub const DEFAULT: &str = "👍";

struct Font {
    face: Face<'static>,
    shaper: harfrust::ShaperData,
    data: &'static [u8],
}

/// Segoe UI Emoji from the local Windows font folder; `None` if it is missing.
fn font() -> Option<&'static Font> {
    static FONT: OnceLock<Option<Font>> = OnceLock::new();
    FONT.get_or_init(|| {
        let path = std::path::PathBuf::from(std::env::var_os("WINDIR")?).join("Fonts").join("seguiemj.ttf");
        let data: &'static [u8] = Box::leak(std::fs::read(path).ok()?.into_boxed_slice());
        let face = Face::parse(data, 0).ok()?;
        let shaper = harfrust::ShaperData::new(&harfrust::FontRef::new(data).ok()?);
        Some(Font { face, shaper, data })
    })
    .as_ref()
}

/// Whether emoji can be drawn at all on this computer.
pub fn available() -> bool {
    font().is_some()
}

/// The glyphs for `text` and where each goes, in font units along the baseline, and
/// the total advance.
fn shape(font: &Font, text: &str) -> (Vec<(GlyphId, f32, f32)>, f32) {
    let Ok(font_ref) = harfrust::FontRef::new(font.data) else { return (Vec::new(), 0.0) };
    let shaper = font.shaper.shaper(&font_ref).build();
    let mut buffer = harfrust::UnicodeBuffer::new();
    buffer.push_str(text);
    buffer.guess_segment_properties();
    let shaped = shaper.shape(buffer, harfrust::ShapeOptions::new());
    let mut x = 0.0;
    let mut glyphs = Vec::new();
    for (info, pos) in shaped.glyph_infos().iter().zip(shaped.glyph_positions()) {
        let Ok(id) = u16::try_from(info.glyph_id) else { continue };
        glyphs.push((GlyphId(id), x + pos.x_offset as f32, pos.y_offset as f32));
        x += pos.x_advance as f32;
    }
    (glyphs, x)
}

/// Whether `text` is one emoji (or a sequence drawn as one, such as a flag or a
/// thumbs-up with a skin tone) that the font draws in colour.
pub fn is_emoji(text: &str) -> bool {
    let Some(font) = font() else { return false };
    let (glyphs, _) = shape(font, text);
    glyphs.len() == 1 && font.face.is_color_glyph(glyphs[0].0)
}

/// The area `text` covers when drawn `size` pixels tall, centred on `centre`.
pub fn bounds(text: &str, size: f32, centre: Pos2) -> Rect {
    let width = match font() {
        Some(font) => shape(font, text).1 / f32::from(font.face.units_per_em()) * size,
        None => size,
    };
    Rect::from_center_size(centre, Vec2::new(width.max(size * 0.5), size))
}

/// Draw `text` `size` pixels tall, centred on desktop position `centre`, into `pixmap`
/// whose top-left pixel is desktop position `origin`.
pub fn rasterize(text: &str, size: f32, centre: Pos2, pixmap: &mut tiny_skia::PixmapMut<'_>, origin: Pos2) {
    let Some(font) = font() else { return };
    let (glyphs, advance) = shape(font, text);
    // Drawn into a layer of its own just big enough for the stamp (with a margin for
    // glyphs that reach outside their box), then onto the target.
    let area = bounds(text, size, centre).expand(size * 0.25);
    let area = Rect::from_min_max((area.min - origin.to_vec2()).floor(), (area.max - origin.to_vec2()).ceil());
    let Some(mut layer) = Pixmap::new(area.width().max(1.0) as u32, area.height().max(1.0) as u32) else { return };

    let face = &font.face;
    let scale = size / f32::from(face.units_per_em());
    // Vertically centred on the font's ascent and descent, where emoji are designed.
    let middle = (f32::from(face.ascender()) + f32::from(face.descender())) / 2.0;
    let local = centre - origin.to_vec2() - area.min.to_vec2();
    let base = Transform::from_translate(local.x, local.y)
        .pre_scale(scale, -scale)
        .pre_translate(-advance / 2.0, -middle);
    for (glyph, x, y) in glyphs {
        let transform = base.pre_translate(x, y);
        if face.is_color_glyph(glyph) {
            let mut painter = Painter::new(face, &mut layer, transform);
            let black = RgbaColor::new(0, 0, 0, 255);
            face.paint_color_glyph(glyph, 0, black, &mut painter);
            painter.finish();
        } else if let Some(path) = outline(face, glyph) {
            let mut paint = Paint::default();
            paint.set_color_rgba8(0, 0, 0, 255);
            paint.anti_alias = true;
            layer.fill_path(&path, &paint, FillRule::Winding, transform, None);
        }
    }
    let (x, y) = (area.min.x as i32, area.min.y as i32);
    pixmap.draw_pixmap(x, y, layer.as_ref(), &PixmapPaint::default(), Transform::identity(), None);
}

/// A glyph's outline in font units.
fn outline(face: &Face<'_>, glyph: GlyphId) -> Option<Path> {
    struct Builder(PathBuilder);
    impl ttf_parser::OutlineBuilder for Builder {
        fn move_to(&mut self, x: f32, y: f32) {
            self.0.move_to(x, y);
        }
        fn line_to(&mut self, x: f32, y: f32) {
            self.0.line_to(x, y);
        }
        fn quad_to(&mut self, x1: f32, y1: f32, x: f32, y: f32) {
            self.0.quad_to(x1, y1, x, y);
        }
        fn curve_to(&mut self, x1: f32, y1: f32, x2: f32, y2: f32, x: f32, y: f32) {
            self.0.cubic_to(x1, y1, x2, y2, x, y);
        }
        fn close(&mut self) {
            self.0.close();
        }
    }
    let mut builder = Builder(PathBuilder::new());
    face.outline_glyph(glyph, &mut builder)?;
    builder.0.finish()
}

/// Paints a colour glyph's layers (COLR version 0: flat colours; version 1: also
/// gradients, clips and blended layers) with tiny-skia.
struct Painter<'f, 'p> {
    face: &'f Face<'static>,
    /// The glyph's own target, and layers pushed on top of it with how each blends
    /// down when popped.
    target: &'p mut Pixmap,
    layers: Vec<(Pixmap, CompositeMode)>,
    transforms: Vec<Transform>,
    clips: Vec<Mask>,
    /// The outline of the glyph last named, in font units.
    outline: Option<Path>,
}

impl<'f, 'p> Painter<'f, 'p> {
    fn new(face: &'f Face<'static>, target: &'p mut Pixmap, transform: Transform) -> Self {
        Self { face, target, layers: Vec::new(), transforms: vec![transform], clips: Vec::new(), outline: None }
    }

    fn transform(&self) -> Transform {
        *self.transforms.last().unwrap_or(&Transform::identity())
    }

    fn canvas(&mut self) -> &mut Pixmap {
        match self.layers.last_mut() {
            Some((layer, _)) => layer,
            None => self.target,
        }
    }

    /// Merge any layers left open.
    fn finish(mut self) {
        while !self.layers.is_empty() {
            colr::Painter::pop_layer(&mut self);
        }
    }

    fn push_clip_path(&mut self, path: Option<Path>) {
        let (w, h) = (self.target.width(), self.target.height());
        let transform = self.transform();
        let Some(mut mask) = self.clips.last().cloned().or_else(|| Mask::new(w, h)) else { return };
        match (path, self.clips.is_empty()) {
            (Some(path), true) => mask.fill_path(&path, FillRule::Winding, true, transform),
            (Some(path), false) => mask.intersect_path(&path, FillRule::Winding, true, transform),
            // Nothing to clip to: everything is clipped away.
            (None, _) => mask.clear(),
        }
        self.clips.push(mask);
    }

    fn shader(&self, paint: &colr::Paint<'_>) -> Option<Shader<'static>> {
        let coords = self.face.variation_coordinates();
        let color = |c: RgbaColor| Color::from_rgba8(c.red, c.green, c.blue, c.alpha);
        let stops = |stops: &mut dyn Iterator<Item = colr::ColorStop>| -> Vec<GradientStop> {
            stops.map(|s| GradientStop::new(s.stop_offset, color(s.color))).collect()
        };
        let spread = |extend: GradientExtend| match extend {
            GradientExtend::Pad => SpreadMode::Pad,
            GradientExtend::Repeat => SpreadMode::Repeat,
            GradientExtend::Reflect => SpreadMode::Reflect,
        };
        let transform = self.transform();
        match paint {
            colr::Paint::Solid(c) => Some(Shader::SolidColor(color(*c))),
            colr::Paint::LinearGradient(g) => {
                // The gradient runs from p0 towards p1, turned to be at right angles
                // to the line from p0 to p2.
                let (p0, p1, p2) = (Point::from_xy(g.x0, g.y0), Point::from_xy(g.x1, g.y1), Point::from_xy(g.x2, g.y2));
                let perp = Point::from_xy(p2.y - p0.y, p0.x - p2.x);
                let len = perp.x * perp.x + perp.y * perp.y;
                let end = if len > f32::EPSILON {
                    let t = ((p1.x - p0.x) * perp.x + (p1.y - p0.y) * perp.y) / len;
                    Point::from_xy(p0.x + perp.x * t, p0.y + perp.y * t)
                } else {
                    p1
                };
                let stops = stops(&mut g.stops(0, coords));
                tiny_skia::LinearGradient::new(p0, end, stops, spread(g.extend), transform)
            }
            colr::Paint::RadialGradient(g) => {
                let stops = stops(&mut g.stops(0, coords));
                tiny_skia::RadialGradient::new(
                    Point::from_xy(g.x0, g.y0),
                    g.r0.max(0.0),
                    Point::from_xy(g.x1, g.y1),
                    g.r1.max(0.0),
                    stops,
                    spread(g.extend),
                    transform,
                )
            }
            colr::Paint::SweepGradient(g) => {
                // Angles are stored in half turns, counter-clockwise in font space.
                let stops = stops(&mut g.stops(0, coords));
                let (start, end) = (g.start_angle * 180.0, g.end_angle * 180.0);
                tiny_skia::SweepGradient::new(
                    Point::from_xy(g.center_x, g.center_y),
                    start,
                    end,
                    stops,
                    spread(g.extend),
                    transform,
                )
            }
        }
    }
}

fn blend_mode(mode: CompositeMode) -> BlendMode {
    match mode {
        CompositeMode::Clear => BlendMode::Clear,
        CompositeMode::Source => BlendMode::Source,
        CompositeMode::Destination => BlendMode::Destination,
        CompositeMode::SourceOver => BlendMode::SourceOver,
        CompositeMode::DestinationOver => BlendMode::DestinationOver,
        CompositeMode::SourceIn => BlendMode::SourceIn,
        CompositeMode::DestinationIn => BlendMode::DestinationIn,
        CompositeMode::SourceOut => BlendMode::SourceOut,
        CompositeMode::DestinationOut => BlendMode::DestinationOut,
        CompositeMode::SourceAtop => BlendMode::SourceAtop,
        CompositeMode::DestinationAtop => BlendMode::DestinationAtop,
        CompositeMode::Xor => BlendMode::Xor,
        CompositeMode::Plus => BlendMode::Plus,
        CompositeMode::Screen => BlendMode::Screen,
        CompositeMode::Overlay => BlendMode::Overlay,
        CompositeMode::Darken => BlendMode::Darken,
        CompositeMode::Lighten => BlendMode::Lighten,
        CompositeMode::ColorDodge => BlendMode::ColorDodge,
        CompositeMode::ColorBurn => BlendMode::ColorBurn,
        CompositeMode::HardLight => BlendMode::HardLight,
        CompositeMode::SoftLight => BlendMode::SoftLight,
        CompositeMode::Difference => BlendMode::Difference,
        CompositeMode::Exclusion => BlendMode::Exclusion,
        CompositeMode::Multiply => BlendMode::Multiply,
        CompositeMode::Hue => BlendMode::Hue,
        CompositeMode::Saturation => BlendMode::Saturation,
        CompositeMode::Color => BlendMode::Color,
        CompositeMode::Luminosity => BlendMode::Luminosity,
    }
}

impl<'a> colr::Painter<'a> for Painter<'_, '_> {
    fn outline_glyph(&mut self, glyph: GlyphId) {
        self.outline = outline(self.face, glyph);
    }

    fn paint(&mut self, paint: colr::Paint<'a>) {
        let Some(shader) = self.shader(&paint) else { return };
        let paint = Paint { shader, anti_alias: true, ..Paint::default() };
        let mask = self.clips.last().cloned();
        let transform = self.transform();
        let outline = self.outline.clone();
        let canvas = self.canvas();
        match mask {
            // Version 1: fill whatever the clips leave.
            Some(mask) => {
                let full = tiny_skia::Rect::from_xywh(0.0, 0.0, canvas.width() as f32, canvas.height() as f32);
                if let Some(full) = full {
                    canvas.fill_rect(full, &paint, Transform::identity(), Some(&mask));
                }
            }
            // Version 0: fill the glyph just named.
            None => {
                if let Some(path) = outline {
                    canvas.fill_path(&path, &paint, FillRule::Winding, transform, None);
                }
            }
        }
    }

    fn push_clip(&mut self) {
        let path = self.outline.clone();
        self.push_clip_path(path);
    }

    fn push_clip_box(&mut self, clip: colr::ClipBox) {
        let rect = tiny_skia::Rect::from_ltrb(clip.x_min, clip.y_min, clip.x_max, clip.y_max);
        self.push_clip_path(rect.map(PathBuilder::from_rect));
    }

    fn pop_clip(&mut self) {
        self.clips.pop();
    }

    fn push_layer(&mut self, mode: CompositeMode) {
        if let Some(layer) = Pixmap::new(self.target.width(), self.target.height()) {
            self.layers.push((layer, mode));
        }
    }

    fn pop_layer(&mut self) {
        let Some((layer, mode)) = self.layers.pop() else { return };
        let paint = PixmapPaint { blend_mode: blend_mode(mode), ..PixmapPaint::default() };
        self.canvas().draw_pixmap(0, 0, layer.as_ref(), &paint, Transform::identity(), None);
    }

    fn push_transform(&mut self, t: ttf_parser::Transform) {
        let next = self.transform().pre_concat(Transform::from_row(t.a, t.b, t.c, t.d, t.e, t.f));
        self.transforms.push(next);
    }

    fn pop_transform(&mut self) {
        if self.transforms.len() > 1 {
            self.transforms.pop();
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn draws_in_colour_and_scales() {
        if !available() {
            return; // no Segoe UI Emoji on this machine
        }
        for size in [16.0, 64.0, 200.0] {
            let n = (size * 2.0) as u32;
            let mut pixmap = Pixmap::new(n, n).unwrap();
            let centre = Pos2::new(size, size);
            rasterize("🔥", size, centre, &mut pixmap.as_mut(), Pos2::ZERO);
            let painted: Vec<_> = pixmap.pixels().iter().filter(|p| p.alpha() > 200).collect();
            // Covers a fair part of its box, and not in one flat colour.
            assert!(painted.len() as f32 > size * size * 0.2, "{size}: {}", painted.len());
            let reds = painted.iter().filter(|p| p.red() > p.blue().saturating_add(80)).count();
            assert!(reds > 0, "{size}: no colour");
        }
        // A skin-tone sequence comes out as one emoji, about as wide as a plain one.
        let one = bounds("👍", 64.0, Pos2::ZERO).width();
        let toned = bounds("👍🏽", 64.0, Pos2::ZERO).width();
        assert!((one - toned).abs() < 4.0, "{one} vs {toned}");
    }
}

#[cfg(test)]
#[test]
#[ignore]
fn emoji_sheet() {
    assert!(available());
    let mut pixmap = Pixmap::new(900, 260).unwrap();
    pixmap.fill(Color::WHITE);
    for (i, e) in FAVOURITES.iter().enumerate() {
        let (col, row) = ((i % 20) as f32, (i / 20) as f32);
        rasterize(e, 36.0, Pos2::new(25.0 + col * 44.0, 25.0 + row * 44.0), &mut pixmap.as_mut(), Pos2::ZERO);
    }
    for (i, e) in ["👍🏽", "🇸🇪", "👨‍👩‍👧", "🏳️‍🌈"].iter().enumerate() {
        rasterize(e, 64.0, Pos2::new(50.0 + i as f32 * 90.0, 160.0), &mut pixmap.as_mut(), Pos2::ZERO);
    }
    rasterize("🚀", 120.0, Pos2::new(700.0, 180.0), &mut pixmap.as_mut(), Pos2::ZERO);
    let img = image::RgbaImage::from_raw(900, 260, pixmap.data().to_vec()).unwrap();
    img.save(std::env::var("EMOJI_SHEET").unwrap()).unwrap();
}
