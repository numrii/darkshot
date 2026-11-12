//! Text for annotations: font loading, line layout and glyph outlines.
//!
//! Text is rasterised from glyph outlines (not with egui's text renderer) so the copied
//! or saved image looks exactly like the text on screen, which is drawn from the same
//! rasterised layer.

use std::sync::OnceLock;

use eframe::egui::{vec2, Pos2, Rect, Vec2};
use ttf_parser::{Face, GlyphId, OutlineBuilder};

/// The annotation font: Segoe UI from the local Windows font folder, or egui's own
/// built-in font if that is missing.
fn face() -> Option<&'static Face<'static>> {
    static DATA: OnceLock<Vec<u8>> = OnceLock::new();
    static FACE: OnceLock<Option<Face<'static>>> = OnceLock::new();
    FACE.get_or_init(|| {
        let data = DATA.get_or_init(|| {
            let windows = std::env::var_os("WINDIR")
                .map(|dir| std::path::PathBuf::from(dir).join("Fonts").join("segoeui.ttf"))
                .and_then(|path| std::fs::read(path).ok());
            windows.unwrap_or_else(|| {
                let fonts = eframe::egui::FontDefinitions::default();
                let fallback = fonts.font_data.values().next();
                fallback.map(|f| (*f.font).as_ref().to_vec()).unwrap_or_default()
            })
        });
        Face::parse(data, 0).ok()
    })
    .as_ref()
}

/// Positions of every character of a text at a given size, relative to its top-left.
pub struct Layout {
    /// Per line: the x position of each character boundary (one more than the
    /// number of characters), and the character index the line starts at.
    lines: Vec<(Vec<f32>, usize)>,
    pub line_height: f32,
    ascent: f32,
    pub size: Vec2,
}

impl Layout {
    pub fn new(text: &str, font_size: f32) -> Self {
        let mut lines = Vec::new();
        let (mut line_height, mut ascent) = (font_size * 1.3, font_size);
        let mut start = 0;
        if let Some(face) = face() {
            let scale = font_size / f32::from(face.units_per_em());
            ascent = f32::from(face.ascender()) * scale;
            let descent = -f32::from(face.descender()) * scale;
            line_height = ascent + descent + f32::from(face.line_gap()).max(0.0) * scale;
        }
        for line in text.split('\n') {
            let mut xs = vec![0.0];
            let mut x = 0.0;
            for ch in line.chars() {
                x += advance(ch, font_size);
                xs.push(x);
            }
            let count = xs.len() - 1;
            lines.push((xs, start));
            start += count + 1; // the newline
        }
        let width = lines.iter().map(|(xs, _)| *xs.last().unwrap()).fold(0.0, f32::max);
        let size = vec2(width, line_height * lines.len() as f32);
        Self { lines, line_height, ascent, size }
    }

    /// Where the caret goes before character `index`: its top, relative to the text's
    /// top-left. The caret is `line_height` tall.
    pub fn caret(&self, index: usize) -> Pos2 {
        for (row, (xs, start)) in self.lines.iter().enumerate() {
            let len = xs.len() - 1;
            if index <= start + len {
                let col = index.saturating_sub(*start);
                return Pos2::new(xs[col], row as f32 * self.line_height);
            }
        }
        Pos2::new(0.0, 0.0)
    }

    /// The character index nearest to `p` (relative to the text's top-left).
    pub fn index_at(&self, p: Pos2) -> usize {
        let row = ((p.y / self.line_height).floor().max(0.0) as usize).min(self.lines.len() - 1);
        let (xs, start) = &self.lines[row];
        let col = xs
            .iter()
            .enumerate()
            .min_by(|(_, a), (_, b)| (*a - p.x).abs().total_cmp(&(*b - p.x).abs()))
            .map_or(0, |(i, _)| i);
        start + col
    }

    /// Index of the line containing character `index`, and that line's first index and
    /// length.
    pub fn line_of(&self, index: usize) -> (usize, usize, usize) {
        for (row, (xs, start)) in self.lines.iter().enumerate() {
            let len = xs.len() - 1;
            if index <= start + len {
                return (row, *start, len);
            }
        }
        let (xs, start) = self.lines.last().unwrap();
        (self.lines.len() - 1, *start, xs.len() - 1)
    }

    pub fn line_count(&self) -> usize {
        self.lines.len()
    }
}

fn advance(ch: char, font_size: f32) -> f32 {
    let Some(face) = face() else { return font_size * 0.55 };
    let scale = font_size / f32::from(face.units_per_em());
    if ch == '\t' {
        return advance(' ', font_size) * 4.0;
    }
    let glyph = face.glyph_index(ch).unwrap_or(GlyphId(0));
    f32::from(face.glyph_hor_advance(glyph).unwrap_or(0)) * scale
}

/// Glyph outlines of `text` at `font_size`, with its top-left at `origin`, as one path.
pub fn outline(text: &str, font_size: f32, origin: Pos2) -> Option<tiny_skia::Path> {
    let face = face()?;
    let layout = Layout::new(text, font_size);
    let scale = font_size / f32::from(face.units_per_em());
    let mut builder = PathSink { pb: tiny_skia::PathBuilder::new(), scale, x: 0.0, y: 0.0 };
    for (row, line) in text.split('\n').enumerate() {
        let baseline = origin.y + row as f32 * layout.line_height + layout.ascent;
        let mut x = origin.x;
        for ch in line.chars() {
            if !ch.is_whitespace() {
                if let Some(glyph) = face.glyph_index(ch) {
                    (builder.x, builder.y) = (x, baseline);
                    face.outline_glyph(glyph, &mut builder);
                }
            }
            x += advance(ch, font_size);
        }
    }
    builder.pb.finish()
}

/// Bounds of `text` at `font_size` with its top-left at `origin`.
pub fn bounds(text: &str, font_size: f32, origin: Pos2) -> Rect {
    let layout = Layout::new(text, font_size);
    // An empty text still has the height of one line and a sliver of width, so the
    // caret and the editing box have something to show.
    Rect::from_min_size(origin, vec2(layout.size.x.max(1.0), layout.size.y))
}

/// Receives glyph outlines in font units and writes them, scaled and flipped (fonts
/// have y pointing up), into a path.
struct PathSink {
    pb: tiny_skia::PathBuilder,
    scale: f32,
    x: f32,
    y: f32,
}

impl PathSink {
    fn map(&self, x: f32, y: f32) -> (f32, f32) {
        (self.x + x * self.scale, self.y - y * self.scale)
    }
}

impl OutlineBuilder for PathSink {
    fn move_to(&mut self, x: f32, y: f32) {
        let (x, y) = self.map(x, y);
        self.pb.move_to(x, y);
    }
    fn line_to(&mut self, x: f32, y: f32) {
        let (x, y) = self.map(x, y);
        self.pb.line_to(x, y);
    }
    fn quad_to(&mut self, x1: f32, y1: f32, x: f32, y: f32) {
        let ((x1, y1), (x, y)) = (self.map(x1, y1), self.map(x, y));
        self.pb.quad_to(x1, y1, x, y);
    }
    fn curve_to(&mut self, x1: f32, y1: f32, x2: f32, y2: f32, x: f32, y: f32) {
        let ((x1, y1), (x2, y2), (x, y)) = (self.map(x1, y1), self.map(x2, y2), self.map(x, y));
        self.pb.cubic_to(x1, y1, x2, y2, x, y);
    }
    fn close(&mut self) {
        self.pb.close();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn layout_and_carets() {
        let layout = Layout::new("ab\ncde", 20.0);
        assert_eq!(layout.line_count(), 2);
        assert!(layout.size.x > 0.0 && layout.size.y > 30.0);
        // Indices: a=0 b=1 (newline 2) c=3 d=4 e=5, end 6.
        assert_eq!(layout.caret(0), Pos2::new(0.0, 0.0));
        assert_eq!(layout.caret(3).y, layout.line_height);
        assert_eq!(layout.caret(3).x, 0.0);
        assert!(layout.caret(6).x > layout.caret(5).x);
        assert_eq!(layout.line_of(4), (1, 3, 3));
        // Clicking just right of "b" lands after it; on line two near the start, before "c".
        assert_eq!(layout.index_at(Pos2::new(layout.caret(2).x + 1.0, 5.0)), 2);
        assert_eq!(layout.index_at(Pos2::new(1.0, layout.line_height + 5.0)), 3);
    }

    #[test]
    fn outlines_exist_for_visible_glyphs() {
        assert!(outline("Hi", 24.0, Pos2::ZERO).is_some());
        assert!(outline("   ", 24.0, Pos2::ZERO).is_none());
        let path = outline("H", 100.0, Pos2::new(10.0, 10.0)).unwrap();
        let b = path.bounds();
        // An "H" at 100 px sits below the top and starts near the left edge.
        assert!(b.left() >= 10.0 && b.left() < 30.0 && b.top() > 10.0 && b.height() > 50.0);
    }
}
