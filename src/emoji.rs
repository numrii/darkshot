//! Emoji stamps, drawn from Segoe UI Emoji, the colour emoji font that comes with
//! Windows. Its emoji are vector shapes (OpenType `COLR` layers), so a stamp is drawn
//! sharp at any size, on screen and in the copied or saved image alike. Sequences such
//! as skin tones, flags and families are put together by harfrust, the text shaper egui
//! already uses. Kaomoji and symbols, such as (●'◡'●) or ©, are drawn from other fonts
//! that come with Windows, in the stamp's colour.

use std::sync::OnceLock;

use eframe::egui::{Color32, Pos2, Rect, Vec2};
use tiny_skia::{
    BlendMode, Color, FillRule, GradientStop, Mask, Paint, Path, PathBuilder, Pixmap, PixmapPaint, Point, Shader,
    SpreadMode, Transform,
};
use ttf_parser::colr::{self, CompositeMode, GradientExtend};
use ttf_parser::{Face, GlyphId, RgbaColor};

/// The emoji the picker offers, a handful of the most useful for pointing things out
/// in a screenshot. Any other, or a kaomoji or symbol, can be picked with Windows'
/// emoji panel (Win+.).
pub const FAVOURITES: [&str; 40] = [
    "👍", "👎", "👌", "👉", "👈", "👆", "👇", "👀", "✅", "❌", "⚠️", "❗", "❓", "💡", "🔥", "⭐",
    "❤️", "💯", "🎉", "🚀", "🐛", "🔒", "🔑", "📌", "📎", "🗑️", "⏰", "🛑", "🆕", "🆗", "😀", "😂",
    "😍", "🤔", "😕", "😬", "😱", "🙏", "👏", "🤷",
];
pub const DEFAULT: &str = "👍";


/// Every emoji, by group, with its name: Unicode's list, built into the program and
/// read only when the picker shows a group or searches. See `emoji_list.txt`.
const LIST: &str = include_str!("emoji_list.txt");

/// The picker's groups, as `LIST` names them, and the emoji on each tab.
pub const GROUPS: [(&str, &str); 9] = [
    ("Smileys & Emotion", "😀"),
    ("People & Body", "👋"),
    ("Animals & Nature", "🐱"),
    ("Food & Drink", "🍕"),
    ("Travel & Places", "🚗"),
    ("Activities", "⚽"),
    ("Objects", "💡"),
    ("Symbols", "❤️"),
    ("Flags", "🏁"),
];

/// One emoji in `LIST`.
struct Entry {
    /// Index into `GROUPS`.
    group: usize,
    emoji: &'static str,
    /// Unicode's name, such as `face holding back tears`.
    name: &'static str,
    /// Shortcodes as in GitHub, Slack and Discord (`face_holding_back_tears`, `+1`),
    /// space-separated.
    codes: &'static str,
    /// Extra words to find it by, space-separated.
    tags: &'static str,
}

/// Every entry in `LIST`, read straight from the text built into the program.
fn entries() -> impl Iterator<Item = Entry> {
    let mut group = None;
    LIST.lines().filter_map(move |line| {
        if let Some(name) = line.strip_prefix('@') {
            group = GROUPS.iter().position(|g| g.0 == name);
            return None;
        }
        if line.starts_with('#') {
            return None;
        }
        let (emoji, rest) = line.split_once(' ')?;
        let mut fields = rest.split('|');
        let name = fields.next()?;
        let (codes, tags) = (fields.next().unwrap_or(""), fields.next().unwrap_or(""));
        Some(Entry { group: group?, emoji, name, codes, tags })
    })
}

/// The emoji in group `index` that this computer's emoji font draws (it has no
/// country flags, for one, and older Windows lacks the newest emoji).
pub fn group(index: usize) -> Vec<&'static str> {
    entries().filter(|e| e.group == index && is_emoji(e.emoji)).map(|e| e.emoji).collect()
}

/// Up to `limit` emoji for `query`, best first: a shortcode typed whole (`joy`,
/// `:face_holding_back_tears:`), then shortcodes that start with it, then emoji whose
/// name, shortcodes or tags hold every word of it (`back tears`, `holding_back`).
pub fn search(query: &str, limit: usize) -> Vec<&'static str> {
    let query = query.trim().trim_matches(':').to_lowercase();
    let words: Vec<&str> = query.split(|c: char| c.is_whitespace() || c == '_' || c == '-').filter(|w| !w.is_empty()).collect();
    if words.is_empty() {
        return Vec::new();
    }
    let single = !query.contains(char::is_whitespace);
    let rank = |e: &Entry| -> Option<u8> {
        let mut codes = e.codes.split(' ');
        if single && codes.clone().any(|c| c == query) {
            return Some(0);
        }
        if single && codes.any(|c| c.starts_with(query.as_str())) {
            return Some(1);
        }
        let found = |w: &&str| e.name.contains(*w) || e.codes.contains(*w) || e.tags.contains(*w);
        words.iter().all(found).then_some(2)
    };
    let mut found: Vec<(u8, usize, &'static str)> =
        entries().enumerate().filter_map(|(i, e)| rank(&e).map(|r| (r, i, e.emoji))).collect();
    found.sort_unstable();
    found.into_iter().map(|(_, _, e)| e).filter(|e| is_emoji(e)).take(limit).collect()
}

/// The kaomoji the kaomoji picker offers.
pub const KAOMOJI: [&str; 36] = [
    "(●'◡'●)", "(^_^)", "(^▽^)", "(≧▽≦)", "(◕‿◕)", "(｡♥‿♥｡)",
    "ヽ(•‿•)ノ", "\\(^o^)/", "(*^‿^*)", "(⌐■_■)", "(￣ー￣)", "(￢‿￢ )",
    "(•_•)", "(¬_¬)", "ಠ_ಠ", "(◔_◔)", "(⊙_⊙)", "(°ロ°)",
    "(・_・;)", "(；一_一)", "(T_T)", "(╥﹏╥)", "(；´д｀)", "( •́ ̯•̀ )",
    "¯\\_(ツ)_/¯", "(╯°□°）╯︵ ┻━┻", "┬─┬ノ( º _ ºノ)", "(ง •̀_•́)ง", "(づ｡◕‿‿◕｡)づ", "ʕ•ᴥ•ʔ",
    "(=^･ω･^=)", "(っ˘ڡ˘ς)", "(～￣▽￣)～", "o(〃＾▽＾〃)o", "(._.)", "(ﾉ◕ヮ◕)ﾉ*:･ﾟ✧",
];

/// The symbols the symbols picker offers: arrows, marks, shapes, maths, currency.
/// Those that are emoji too are asked for as text (U+FE0E), in the stamp's colour.
pub const SYMBOLS: [&str; 60] = [
    "→", "←", "↑", "↓", "↗\u{FE0E}", "↘\u{FE0E}", "↙\u{FE0E}", "↖\u{FE0E}", "⇒", "⇔",
    "✓", "✗", "✔\u{FE0E}", "✘", "★", "☆", "●", "○", "■", "□",
    "▲", "▼", "◆", "◇", "♥\u{FE0E}", "♦\u{FE0E}", "♣\u{FE0E}", "♠\u{FE0E}", "•", "…",
    "©\u{FE0E}", "®\u{FE0E}", "™\u{FE0E}", "§", "¶", "°", "†", "‡", "«", "»",
    "±", "×", "÷", "≠", "≈", "≤", "≥", "∞", "√", "∑",
    "π", "Ω", "µ", "Δ", "€", "£", "$", "¥", "½", "¾",
];

/// The kaomoji in `KAOMOJI` that this computer's fonts can draw, worked out once. A
/// Windows without some of its fonts (a server edition, say) misses a few.
pub fn kaomoji() -> &'static [&'static str] {
    static DRAWN: OnceLock<Vec<&'static str>> = OnceLock::new();
    DRAWN.get_or_init(|| KAOMOJI.into_iter().filter(|k| can_stamp(k)).collect())
}

/// The symbols in `SYMBOLS` that this computer's fonts can draw, worked out once.
pub fn symbols() -> &'static [&'static str] {
    static DRAWN: OnceLock<Vec<&'static str>> = OnceLock::new();
    DRAWN.get_or_init(|| SYMBOLS.into_iter().filter(|k| can_stamp(k)).collect())
}

/// Whether every font in `FONT_FILES` is on this computer, as on any Windows 10 or 11
/// desktop. For the tests, which can expect every listed stamp to draw only then.
#[cfg(test)]
fn every_font() -> bool {
    let missing: Vec<&str> = (0..FONT_FILES.len()).filter(|&i| font_at(i).is_none()).map(|i| FONT_FILES[i]).collect();
    if !missing.is_empty() {
        eprintln!("fonts missing here: {missing:?}");
    }
    missing.is_empty()
}

struct Font {
    face: Face<'static>,
    shaper: harfrust::ShaperData,
    data: &'static [u8],
}

impl Font {
    fn has(&self, c: char) -> bool {
        self.face.glyph_index(c).is_some_and(|g| g.0 != 0)
    }

    fn units(&self) -> f32 {
        f32::from(self.face.units_per_em())
    }

    /// Halfway between the font's ascent and descent, in ems: where its glyphs are
    /// vertically centred.
    fn middle(&self) -> f32 {
        (f32::from(self.face.ascender()) + f32::from(self.face.descender())) / 2.0 / self.units()
    }
}

/// Segoe UI Emoji, then the fonts tried in turn for characters it should not draw,
/// such as the kaomoji and symbols in Windows' emoji panel: Segoe UI, Segoe UI Symbol,
/// and fonts for Japanese, Indic, Thai and Lao, Korean, Chinese, Canadian syllabics,
/// Ethiopic, historic scripts and maths. All come with Windows; each is read only when
/// a character needs it.
const FONT_FILES: [&str; 13] = [
    "seguiemj.ttf",
    "segoeui.ttf",
    "seguisym.ttf",
    "YuGothR.ttc",
    "msgothic.ttc",
    "Nirmala.ttf",
    "LeelawUI.ttf",
    "malgun.ttf",
    "msyh.ttc",
    "gadugi.ttf",
    "ebrima.ttf",
    "seguihis.ttf",
    "cambria.ttc",
];
const EMOJI_FONT: usize = 0;
const TEXT_FONT: usize = 1;

/// The font `FONT_FILES[index]` from the local Windows font folder; `None` if missing.
fn font_at(index: usize) -> Option<&'static Font> {
    static FONTS: [OnceLock<Option<Font>>; FONT_FILES.len()] = [const { OnceLock::new() }; FONT_FILES.len()];
    FONTS[index]
        .get_or_init(|| {
            let path = std::path::PathBuf::from(std::env::var_os("WINDIR")?).join("Fonts").join(FONT_FILES[index]);
            let data: &'static [u8] = Box::leak(std::fs::read(path).ok()?.into_boxed_slice());
            let face = Face::parse(data, 0).ok()?;
            let shaper = harfrust::ShaperData::new(&harfrust::FontRef::from_index(data, 0).ok()?);
            Some(Font { face, shaper, data })
        })
        .as_ref()
}

/// Segoe UI Emoji; `None` if it is missing.
fn font() -> Option<&'static Font> {
    font_at(EMOJI_FONT)
}

/// Whether emoji can be drawn at all on this computer.
pub fn available() -> bool {
    font().is_some()
}

/// A glyph, the font it comes from, and where it goes, in ems from the start of the
/// baseline.
struct Placed {
    font: &'static Font,
    glyph: GlyphId,
    x: f32,
    y: f32,
}

/// The glyphs for `text` in `font` and where each goes, and the total advance in ems.
fn shape(font: &'static Font, text: &str) -> (Vec<Placed>, f32) {
    let Ok(font_ref) = harfrust::FontRef::from_index(font.data, 0) else { return (Vec::new(), 0.0) };
    let shaper = font.shaper.shaper(&font_ref).build();
    let mut buffer = harfrust::UnicodeBuffer::new();
    buffer.push_str(text);
    buffer.guess_segment_properties();
    let shaped = shaper.shape(buffer, harfrust::ShapeOptions::new());
    let units = font.units();
    let mut x = 0.0;
    let mut glyphs = Vec::new();
    for (info, pos) in shaped.glyph_infos().iter().zip(shaped.glyph_positions()) {
        let Ok(id) = u16::try_from(info.glyph_id) else { continue };
        let at = (x + pos.x_offset as f32) / units;
        glyphs.push(Placed { font, glyph: GlyphId(id), x: at, y: pos.y_offset as f32 / units });
        x += pos.x_advance as f32;
    }
    (glyphs, x / units)
}

/// `text` laid out: its glyphs, its advance in ems, and its vertical middle in ems
/// above the baseline.
struct Line {
    glyphs: Vec<Placed>,
    advance: f32,
    middle: f32,
}

/// Asks for the character before it as text rather than as an emoji.
const TEXT_STYLE: char = '\u{FE0E}';

/// Characters that belong with the one before them and are drawn in its font: joiners,
/// variation selectors, skin tones, tags and combining marks.
fn joins_previous(c: char) -> bool {
    matches!(
        c as u32,
        0x200D | 0xFE00..=0xFE0F | 0x1F3FB..=0x1F3FF | 0xE0020..=0xE007F | 0x0300..=0x036F | 0x20D0..=0x20FF | 0xFE20..=0xFE2F
    )
}

/// The font for `c`: Segoe UI Emoji when it is asked for as an emoji (followed by
/// U+FE0F, or from the emoji blocks), otherwise the first text font that has it, with
/// Segoe UI Emoji and then Segoe UI as the last resorts.
fn font_for(c: char, next: Option<char>) -> usize {
    let has = |i: usize| font_at(i).is_some_and(|f| f.has(c));
    let wants_emoji = next == Some('\u{FE0F}') || (c as u32) >= 0x1F000;
    if wants_emoji && has(EMOJI_FONT) {
        return EMOJI_FONT;
    }
    (TEXT_FONT..FONT_FILES.len()).find(|&i| has(i)).unwrap_or(if has(EMOJI_FONT) { EMOJI_FONT } else { TEXT_FONT })
}

/// Lay out `text`. One emoji (or a sequence drawn as one) comes from Segoe UI Emoji
/// alone; anything else, such as a kaomoji, is split into runs by the font each
/// character needs.
fn layout(text: &str) -> Line {
    let empty = Line { glyphs: Vec::new(), advance: 0.0, middle: 0.0 };
    let Some(emoji) = font() else { return empty };
    if is_emoji(text) {
        let (glyphs, advance) = shape(emoji, text);
        return Line { glyphs, advance, middle: emoji.middle() };
    }
    // The text-style selector has done its job once the font is chosen below.
    let text: String = text.chars().filter(|&c| c != TEXT_STYLE).collect();
    let text = text.as_str();
    let mut runs: Vec<(usize, usize, usize)> = Vec::new(); // font, start, end
    let mut chars = text.char_indices().peekable();
    while let Some((at, c)) = chars.next() {
        let end = at + c.len_utf8();
        let next = chars.peek().map(|&(_, n)| n);
        if let Some(run) = runs.last_mut().filter(|_| joins_previous(c)) {
            run.2 = end;
            continue;
        }
        let font = font_for(c, next);
        match runs.last_mut() {
            Some(run) if run.0 == font => run.2 = end,
            _ => runs.push((font, at, end)),
        }
    }
    let mut line = empty;
    for (index, start, end) in runs {
        let Some(font) = font_at(index) else { continue };
        let (glyphs, advance) = shape(font, &text[start..end]);
        let offset = line.advance;
        line.glyphs.extend(glyphs.into_iter().map(|g| Placed { x: g.x + offset, ..g }));
        line.advance += advance;
    }
    // Lined up as text, on Segoe UI's middle, unless it is all emoji.
    let all_emoji = line.glyphs.iter().all(|g| std::ptr::eq(g.font, emoji));
    line.middle = if all_emoji { emoji.middle() } else { font_at(TEXT_FONT).unwrap_or(emoji).middle() };
    line
}

/// Whether `text` is one emoji (or a sequence drawn as one, such as a flag or a
/// thumbs-up with a skin tone) that the font draws in colour.
pub fn is_emoji(text: &str) -> bool {
    if text.contains(TEXT_STYLE) {
        return false;
    }
    let Some(font) = font() else { return false };
    let (glyphs, _) = shape(font, text);
    glyphs.len() == 1 && font.face.is_color_glyph(glyphs[0].glyph)
}

/// The longest text a stamp takes, in characters: enough for any kaomoji.
const MAX_STAMP_CHARS: usize = 48;

/// Whether `text` can be a stamp: an emoji, or a short line, such as a kaomoji or a
/// symbol, whose every character one of the fonts can draw.
pub fn can_stamp(text: &str) -> bool {
    if is_emoji(text) {
        return true;
    }
    if text.trim().is_empty() || text.chars().count() > MAX_STAMP_CHARS || text.chars().any(char::is_control) {
        return false;
    }
    let line = layout(text);
    !line.glyphs.is_empty() && line.glyphs.iter().all(|g| g.glyph.0 != 0)
}

/// The area `text` covers when drawn `size` pixels tall, centred on `centre`.
pub fn bounds(text: &str, size: f32, centre: Pos2) -> Rect {
    let width = if available() { layout(text).advance * size } else { size };
    Rect::from_center_size(centre, Vec2::new(width.max(size * 0.5), size))
}

/// Draw `text` `size` pixels tall, centred on desktop position `centre`, into `pixmap`
/// whose top-left pixel is desktop position `origin`. Emoji keep their own colours;
/// other characters, such as a kaomoji's, are drawn in `color`.
pub fn rasterize(text: &str, size: f32, centre: Pos2, color: Color32, pixmap: &mut tiny_skia::PixmapMut<'_>, origin: Pos2) {
    draw(text, size, centre, color, false, pixmap, origin);
}

/// As [`rasterize`], but every glyph in one colour, emoji too: the font's own
/// black-and-white emoji, for icons such as the emoji picker's tabs.
pub fn rasterize_plain(text: &str, size: f32, centre: Pos2, color: Color32, pixmap: &mut tiny_skia::PixmapMut<'_>, origin: Pos2) {
    draw(text, size, centre, color, true, pixmap, origin);
}

fn draw(text: &str, size: f32, centre: Pos2, color: Color32, plain: bool, pixmap: &mut tiny_skia::PixmapMut<'_>, origin: Pos2) {
    if !available() {
        return;
    }
    let line = layout(text);
    // Drawn into a layer of its own just big enough for the stamp (with a margin for
    // glyphs that reach outside their box), then onto the target.
    let area = bounds(text, size, centre).expand(size * 0.25);
    let area = Rect::from_min_max((area.min - origin.to_vec2()).floor(), (area.max - origin.to_vec2()).ceil());
    let Some(mut layer) = Pixmap::new(area.width().max(1.0) as u32, area.height().max(1.0) as u32) else { return };

    // Centred on the advance, and vertically on the middle of the font's ascent and
    // descent, where emoji are designed.
    let local = centre - origin.to_vec2() - area.min.to_vec2();
    let base = Transform::from_translate(local.x, local.y)
        .pre_scale(size, -size)
        .pre_translate(-line.advance / 2.0, -line.middle);
    let [r, g, b, a] = color.to_srgba_unmultiplied();
    for Placed { font, glyph, x, y } in line.glyphs {
        let face = &font.face;
        let transform = base.pre_translate(x, y).pre_scale(1.0 / font.units(), 1.0 / font.units());
        if !plain && face.is_color_glyph(glyph) {
            let mut painter = Painter::new(face, &mut layer, transform);
            // Emoji that use the text colour are drawn black, as in Windows.
            face.paint_color_glyph(glyph, 0, RgbaColor::new(0, 0, 0, 255), &mut painter);
            painter.finish();
        } else if let Some(path) = outline(face, glyph) {
            let mut paint = Paint::default();
            paint.set_color_rgba8(r, g, b, a);
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
            rasterize("🔥", size, centre, Color32::BLACK, &mut pixmap.as_mut(), Pos2::ZERO);
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

    #[test]
    fn catalogue() {
        // Every line of the list belongs to a group, and every group is there.
        assert!(entries().count() > 1800);
        for (i, (name, icon)) in GROUPS.iter().enumerate() {
            assert!(entries().any(|e| e.group == i), "{name}");
            assert!(entries().any(|e| e.emoji == *icon), "{name}: {icon}");
        }
        assert_eq!(search("", 10), Vec::<&str>::new());
        if !available() {
            return;
        }
        let smileys = group(0);
        assert!(smileys.len() > 100 && smileys[0] == "😀", "{}", smileys.len());
        assert!(smileys.iter().all(|e| is_emoji(e)));
        // Words in any order, any case; only what the font draws.
        assert!(search("Thumbs UP", 50).contains(&"👍"));
        assert!(search("face grinning", 50).contains(&"😀"));
        assert!(search("fire", 50).contains(&"🔥"));
        assert!(search("zzzz-nothing", 50).is_empty());
        // Shortcodes as in Slack and Discord, with or without colons, best match first.
        assert_eq!(search(":slightly_smiling_face:", 5).first(), Some(&"🙂"));
        assert_eq!(search("slightly_smiling", 5).first(), Some(&"🙂"));
        // 🥹 is newer than Windows 10's emoji font: found only where it is drawn.
        assert_eq!(search("holding_back_tears", 5).first() == Some(&"🥹"), is_emoji("🥹"));
        assert_eq!(search("joy", 5).first(), Some(&"😂"));
        assert_eq!(search(":+1:", 5).first(), Some(&"👍"));
        assert_eq!(search("thumbsup", 5).first(), Some(&"👍"));
        assert!(search("tears_of_joy", 5).contains(&"😂"));
        assert!(search("face", 5).len() <= 5);
        // Country flags are letters in Windows' emoji font, so they are left out.
        assert!(!group(8).contains(&"🇸🇪"));
    }

    #[test]
    fn kaomoji_and_symbols() {
        if !available() {
            return;
        }
        // With every font Windows' desktop has, everything listed draws. Without some
        // (GitHub's Windows Server runners lack a few), the pickers leave out what
        // cannot be drawn, and the rest must still draw properly.
        let every_font = every_font();
        assert!(!every_font || (kaomoji().len() == KAOMOJI.len() && symbols().len() == SYMBOLS.len()));
        assert!(kaomoji().iter().all(|k| can_stamp(k)) && symbols().iter().all(|k| can_stamp(k)));
        // Basic Latin and the common symbols are in Segoe UI itself: always drawn.
        assert!(kaomoji().contains(&"(^_^)") && symbols().contains(&"→") && symbols().contains(&"€"));
        for k in PANEL_SAMPLES.iter().chain(&KAOMOJI).chain(&SYMBOLS) {
            assert!(!is_emoji(k), "{k}");
            if !every_font && !can_stamp(k) {
                eprintln!("{k}: not drawn with the fonts here");
                continue;
            }
            assert!(can_stamp(k), "{k}");
            // A kaomoji is wider than tall; each is drawn in the stamp's colour.
            let size = 40.0;
            let area = bounds(k, size, Pos2::ZERO);
            assert!(SYMBOLS.contains(k) || area.width() > size, "{k}: {area:?}");
            let mut pixmap = Pixmap::new(600, 120).unwrap();
            let green = Color32::from_rgb(0, 200, 0);
            rasterize(k, size, Pos2::new(300.0, 60.0), green, &mut pixmap.as_mut(), Pos2::ZERO);
            let inked = pixmap.pixels().iter().filter(|p| p.alpha() > 200 && p.green() > 150 && p.red() < 50).count();
            assert!(inked > 30, "{k}: {inked}");
        }
        assert!(can_stamp("👍🏽") && can_stamp("❗") && can_stamp("OK!") && can_stamp("(👍)"));
        assert!(!can_stamp("") && !can_stamp("   ") && !can_stamp("a\nb") && !can_stamp(&"x".repeat(60)));
        // An unassigned character, which no font draws.
        assert!(!can_stamp("\u{0378}"));
    }
}

/// Kaomoji and symbols from Windows' emoji panel, for the tests.
#[cfg(test)]
const PANEL_SAMPLES: [&str; 9] = [
    "(●'◡'●)",
    "¯\\_(ツ)_/¯",
    "(╯°□°）╯︵ ┻━┻",
    "ಠ_ಠ",
    "ʕ•ᴥ•ʔ",
    "(づ｡◕‿‿◕｡)づ",
    "ლ(ಠ益ಠლ)",
    "© ™ → ½ Ω ♥",
    "ヽ(•‿•)ノ",
];

#[cfg(test)]
#[test]
#[ignore]
fn emoji_sheet() {
    assert!(available());
    let mut pixmap = Pixmap::new(900, 880).unwrap();
    pixmap.fill(Color::WHITE);
    for (i, e) in FAVOURITES.iter().enumerate() {
        let (col, row) = ((i % 20) as f32, (i / 20) as f32);
        rasterize(e, 36.0, Pos2::new(25.0 + col * 44.0, 25.0 + row * 44.0), Color32::BLACK, &mut pixmap.as_mut(), Pos2::ZERO);
    }
    for (i, e) in ["👍🏽", "🇸🇪", "👨‍👩‍👧", "🏳️‍🌈"].iter().enumerate() {
        rasterize(e, 64.0, Pos2::new(50.0 + i as f32 * 90.0, 160.0), Color32::BLACK, &mut pixmap.as_mut(), Pos2::ZERO);
    }
    rasterize("🚀", 120.0, Pos2::new(700.0, 180.0), Color32::BLACK, &mut pixmap.as_mut(), Pos2::ZERO);
    let blue = Color32::from_rgb(0x11, 0x8A, 0xB2);
    for (i, k) in KAOMOJI.iter().chain(&SYMBOLS).enumerate() {
        let (col, row) = ((i % 6) as f32, (i / 6) as f32);
        rasterize(k, 22.0, Pos2::new(75.0 + col * 150.0, 250.0 + row * 34.0), blue, &mut pixmap.as_mut(), Pos2::ZERO);
    }
    // The picker's tabs: the font's black-and-white emoji.
    let tabs = std::iter::once("🕘").chain(GROUPS.iter().map(|g| g.1));
    for (i, icon) in tabs.enumerate() {
        rasterize_plain(icon, 28.0, Pos2::new(40.0 + i as f32 * 44.0, 845.0), Color32::from_gray(60), &mut pixmap.as_mut(), Pos2::ZERO);
    }
    let img = image::RgbaImage::from_raw(900, 880, pixmap.data().to_vec()).unwrap();
    img.save(std::env::var("EMOJI_SHEET").unwrap()).unwrap();
}
