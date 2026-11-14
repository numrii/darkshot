//! Reading what is in a capture: its text (OCR) and QR codes. Both run on this
//! computer only. Text recognition is the one built into Windows
//! (`Windows.Media.Ocr`), in the languages the user has installed; QR codes are
//! decoded here, by `rqrr`. Nothing found is ever opened or followed: a link in a QR
//! code is only shown and copied as text.

use eframe::egui::{Pos2, Rect};
use image::RgbaImage;

/// What to read from the region.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Kind {
    Text,
    Qr,
}

/// One line of text, or one QR code, and where it is.
#[derive(Clone, Debug, PartialEq)]
pub struct Line {
    /// The whole line, spaced as Windows reads it (no spaces between words in Chinese
    /// or Japanese, for example).
    pub text: String,
    /// Its words, each with its outline in the image's pixels. A QR code is one "word".
    pub words: Vec<Word>,
    /// The outline round all its words, turned with them.
    pub quad: Quad,
}

#[derive(Clone, Debug, PartialEq)]
pub struct Word {
    pub text: String,
    pub quad: Quad,
}

impl Line {
    /// The upright box around it.
    pub fn rect(&self) -> Rect {
        self.quad.bounds()
    }
}

#[cfg(test)]
impl Word {
    /// The upright box around it.
    pub fn rect(&self) -> Rect {
        self.quad.bounds()
    }
}

/// An outline with four corners, in order round it: top left, top right, bottom right
/// and bottom left as the text reads. Tilted text has tilted outlines.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Quad(pub [Pos2; 4]);

impl Quad {
    pub fn from_rect(r: Rect) -> Self {
        Self([r.left_top(), r.right_top(), r.right_bottom(), r.left_bottom()])
    }

    pub fn map(self, f: impl Fn(Pos2) -> Pos2) -> Self {
        Self(self.0.map(f))
    }

    pub fn bounds(&self) -> Rect {
        Rect::from_points(&self.0)
    }

    /// Its size across the text: the height of a line.
    pub fn height(&self) -> f32 {
        self.0[0].distance(self.0[3])
    }

    /// Grown by `along` at each end along the text, and by `across` on each side.
    pub fn expand(&self, along: f32, across: f32) -> Self {
        let [a, b, c, d] = self.0;
        let unit = |v: eframe::egui::Vec2| if v.length() > 0.0 { v.normalized() } else { v };
        let (u, v) = (unit(b - a) * along, unit(d - a) * across);
        Self([a - u - v, b + u - v, c + u + v, d - u + v])
    }

    /// Whether `p` is inside (or on the edge).
    pub fn contains(&self, p: Pos2) -> bool {
        let cross = |i: usize| {
            let (a, b) = (self.0[i], self.0[(i + 1) % 4]);
            (b - a).x * (p - a).y - (b - a).y * (p - a).x
        };
        let sides = [cross(0), cross(1), cross(2), cross(3)];
        sides.iter().all(|&s| s >= 0.0) || sides.iter().all(|&s| s <= 0.0)
    }
}

/// What reading found, in reading order, or why there is nothing.
pub type Outcome = Result<Vec<Line>, String>;

/// The text to copy for `lines`: each line on its own.
pub fn joined(lines: &[Line]) -> String {
    lines.iter().map(|l| l.text.as_str()).collect::<Vec<_>>().join("\n")
}

/// Read `img` for `kind`. Can take a moment: call it off the UI thread. With `as_is`
/// (the user turned the capture by hand), text is taken to be the right way up: only
/// read as it is, never turned to find a better reading.
pub fn read(kind: Kind, img: &RgbaImage, as_is: bool) -> Outcome {
    match kind {
        Kind::Text if as_is => text_as_is(img),
        Kind::Text => text(img),
        Kind::Qr => qr(img),
    }
}

/// The text in `img`, read the way up it is.
pub fn text_as_is(img: &RgbaImage) -> Outcome {
    outcome(ocr::recognize(img))
}

/// The text in `img`, one line per line of text, or why there is none. Text that is
/// tilted is straightened by Windows itself (tried up to about 40 degrees); text that
/// is sideways or upside down is found by turning the region, see [`any_way_up`].
pub fn text(img: &RgbaImage) -> Outcome {
    outcome(any_way_up(img))
}

/// What Windows read, or why there is nothing to show.
fn outcome(read: windows::core::Result<Option<Vec<Line>>>) -> Outcome {
    match read {
        Err(e) => Err(format!("Text recognition failed: {}", e.message())),
        Ok(None) => Err("Text recognition is not installed for your languages. Windows adds it with a \
                         language under Settings > Time & language > Language & region."
            .to_owned()),
        Ok(Some(lines)) if lines.iter().all(|l| l.text.trim().is_empty()) => Err("No text found in the region.".to_owned()),
        Ok(Some(lines)) => Ok(lines),
    }
}

/// One way of looking at the region for QR codes: enlarged `scale` times, softened by
/// a blur of `blur` pixels, then perhaps made pure black and white at `threshold`.
struct QrPass {
    scale: u32,
    blur: f32,
    threshold: Option<u8>,
}

/// The ways tried, in order, until every code found is read. The first is the region
/// as it is, which reads ordinary codes. The rest are for styled codes, whose modules
/// are drawn as ornaments, dots or in colour: hard black and white makes coloured
/// modules count as dark; enlarging and softening fills in the gaps between ornaments.
/// (A decorated code that only read this way: a rabbit in blue lace, read at 1x with a
/// threshold, and at 2x or 3x with a slight blur.)
const QR_PASSES: [QrPass; 9] = [
    QrPass { scale: 1, blur: 0.0, threshold: None },
    QrPass { scale: 1, blur: 0.0, threshold: Some(100) },
    QrPass { scale: 1, blur: 0.0, threshold: Some(160) },
    QrPass { scale: 2, blur: 1.0, threshold: None },
    QrPass { scale: 2, blur: 0.0, threshold: Some(128) },
    QrPass { scale: 3, blur: 1.0, threshold: None },
    QrPass { scale: 1, blur: 1.0, threshold: Some(200) },
    QrPass { scale: 2, blur: 1.5, threshold: Some(200) },
    QrPass { scale: 2, blur: 2.0, threshold: Some(220) },
];

/// Regions are only enlarged up to this many pixels (before enlarging), which keeps a
/// whole screen quick. Styled codes are usually small in the region anyway.
const QR_ENLARGE_UP_TO: u32 = 1_000_000;

/// The contents of every QR code in `img`, top to bottom, or why there are none.
pub fn qr(img: &RgbaImage) -> Outcome {
    let (w, h) = img.dimensions();
    // Transparent areas (off every monitor) count as white, like paper.
    let grey = image::GrayImage::from_fn(w, h, |x, y| {
        let [r, g, b, a] = img.get_pixel(x, y).0;
        let grey = (u32::from(r) * 299 + u32::from(g) * 587 + u32::from(b) * 114) / 1000;
        image::Luma([((grey * u32::from(a) + 255 * (255 - u32::from(a))) / 255) as u8])
    });
    let mut found: Vec<Line> = Vec::new();
    let mut seen = false;
    for pass in QR_PASSES.iter().filter(|p| p.scale == 1 || w * h <= QR_ENLARGE_UP_TO) {
        let mut look = if pass.scale == 1 {
            grey.clone()
        } else {
            image::imageops::resize(&grey, w * pass.scale, h * pass.scale, image::imageops::FilterType::Triangle)
        };
        if pass.blur > 0.0 {
            look = image::imageops::blur(&look, pass.blur);
        }
        let (lw, lh) = look.dimensions();
        let mut prepared = rqrr::PreparedImage::prepare_from_greyscale(lw as usize, lh as usize, |x, y| {
            let v = look.get_pixel(x as u32, y as u32).0[0];
            match pass.threshold {
                Some(t) if v < t => 0,
                Some(_) => 255,
                None => v,
            }
        });
        let grids = prepared.detect_grids();
        seen |= !grids.is_empty();
        let mut all_read = !grids.is_empty();
        for grid in &grids {
            let Ok((_, content)) = grid.decode() else {
                all_read = false;
                continue;
            };
            if found.iter().any(|l| l.text == content) {
                continue;
            }
            // Its four corners, back in the region's own pixels.
            let quad = Quad(grid.bounds.map(|p| Pos2::new(p.x as f32, p.y as f32) / pass.scale as f32));
            found.push(Line { text: content.clone(), words: vec![Word { text: content, quad }], quad });
        }
        if all_read {
            break;
        }
    }
    if found.is_empty() {
        return Err(if seen {
            "Found a QR code, but could not read it. Try selecting just the code."
        } else {
            "No QR code found in the region."
        }
        .to_owned());
    }
    found.sort_by(|a, b| (a.rect().top(), a.rect().left()).partial_cmp(&(b.rect().top(), b.rect().left())).unwrap());
    Ok(found)
}

/// A quarter, half or three-quarter turn of the region, clockwise.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Turn {
    Quarter,
    Half,
    ThreeQuarter,
}

impl Turn {
    const ALL: [Turn; 3] = [Turn::Quarter, Turn::Half, Turn::ThreeQuarter];

    fn apply(self, img: &RgbaImage) -> RgbaImage {
        match self {
            Turn::Quarter => image::imageops::rotate90(img),
            Turn::Half => image::imageops::rotate180(img),
            Turn::ThreeQuarter => image::imageops::rotate270(img),
        }
    }

    /// A point in the turned image back in the unturned one, `w` by `h` pixels.
    fn unturn(self, p: Pos2, w: f32, h: f32) -> Pos2 {
        match self {
            Turn::Quarter => Pos2::new(p.y, h - p.x),
            Turn::Half => Pos2::new(w - p.x, h - p.y),
            Turn::ThreeQuarter => Pos2::new(w - p.y, p.x),
        }
    }

    /// `lines` read from the turned image, with their boxes back on the unturned one.
    /// They stay in the order they were read in, which is the text's own order.
    fn unturn_lines(self, mut lines: Vec<Line>, w: f32, h: f32) -> Vec<Line> {
        let unturn = |q: Quad| q.map(|p| self.unturn(p, w, h));
        for line in &mut lines {
            line.quad = unturn(line.quad);
            for word in &mut line.words {
                word.quad = unturn(word.quad);
            }
        }
        lines
    }
}

/// Read `img` the way up its text is. The region is read as it is first; only when
/// that finds nothing, or mostly gibberish (which is what Windows makes of text
/// upside down), is it also read turned a quarter, half and three-quarter turn.
/// "Mostly" is strict, 85% of at least 20 characters: upside-down u, n, o, d and q
/// make believable letters, so gibberish can be 60% plausible ("8t78ZV Old SL
/// euoqd!" was "iPhone 15 Pro A2848"), and a short reading can be all plausible but
/// only part of the text ("N/S", where "S/N 2KR192C22FG" was upside down). Short text
/// is quick to read again.
///
/// The reading as it is wins unless a turned one is clearly better: at least 60%
/// plausible itself, and more of it. Text the rule below does not know (a lowercase code such
/// as a7f3k9q2, say) is then still read as it is, not swapped for gibberish that
/// happens to look like words.
fn any_way_up(img: &RgbaImage) -> windows::core::Result<Option<Vec<Line>>> {
    let Some(upright) = ocr::recognize(img)? else { return Ok(None) };
    let score = plausibility(&upright);
    if plausible_at_least(score, 85) && score.1 >= 20 {
        return Ok(Some(upright));
    }
    let (w, h) = (img.width() as f32, img.height() as f32);
    let mut best = (score.0, upright);
    for turn in Turn::ALL {
        let Some(lines) = ocr::recognize(&turn.apply(img))? else { continue };
        let score = plausibility(&lines);
        if plausible_at_least(score, 60) && score.0 > best.0 {
            best = (score.0, turn.unturn_lines(lines, w, h));
        }
    }
    Ok(Some(best.1))
}

/// At least `percent` of a reading's characters are in plausible words.
fn plausible_at_least((plausible, all): (usize, usize), percent: usize) -> bool {
    all > 0 && plausible * 100 >= all * percent
}

/// How much of a reading looks like real words, as (characters in plausible words,
/// characters in all words that count). Windows gives no confidence, and text read
/// the wrong way up still comes out as letters ("Kneqen18", "6u!ppeds", "noÅ"), so
/// this judges each word's shape instead. Plausible, with a little punctuation
/// before or after allowed, are:
/// - words: only letters, each part (between - and ') all small, all capitals or
///   capitalised, perhaps after one small letter (iPhone, iOS, eBay);
/// - numbers: only digits, with . , : / - in between (9.75, 12:30, 12/03/2026), and
///   perhaps a short lowercase unit after them (500ml, 2nd);
/// - codes: capitals and digits, with - / . in between (2KR192C22FG, XG-4410B,
///   A2848, S/N). Gibberish almost always has a small letter in it.
///
/// Single letters and digits do not count either way: a sideways reading is mostly
/// those. A single letter of a script without capitals (a Chinese character) counts.
///
/// This only chooses which way up to read; it never changes the text that is read.
fn plausibility(lines: &[Line]) -> (usize, usize) {
    let mut plausible = 0;
    let mut all = 0;
    for word in lines.iter().flat_map(|l| l.words.iter()) {
        let core = word.text.trim_matches(|c: char| "\"'([{«“‘.,:;!?)]}»”’%*".contains(c));
        let chars: Vec<char> = core.chars().collect();
        let counts = match chars[..] {
            [] => false,
            [c] => c.is_alphabetic() && !c.is_lowercase() && !c.is_uppercase(),
            _ => true,
        };
        if !counts {
            continue;
        }
        let n = chars.len();
        all += n;
        let word = chars.iter().all(|c| c.is_alphabetic() || "'’-".contains(*c))
            && chars.split(|c| "'’-".contains(*c)).all(well_cased);
        let digits = chars.iter().take_while(|c| c.is_ascii_digit() || ".,:/-".contains(**c)).count();
        let unit = &chars[digits..];
        let number = chars[0].is_ascii_digit() && unit.len() <= 3 && unit.iter().all(|c| c.is_lowercase());
        let code = chars.iter().all(|c| c.is_ascii_uppercase() || c.is_ascii_digit() || "-/.".contains(*c))
            && chars.iter().any(char::is_ascii_uppercase);
        if word || number || code {
            plausible += n;
        }
    }
    (plausible, all)
}

/// Letters all small, all capitals, or capitalised, perhaps after one small letter
/// (iPhone). Letters of scripts without capitals fit any of these.
fn well_cased(part: &[char]) -> bool {
    let shaped = |p: &[char]| match p {
        [] => true,
        [first, rest @ ..] => {
            let small = |c: &char| !c.is_uppercase();
            let capital = |c: &char| !c.is_lowercase();
            rest.iter().all(small) || (capital(first) && rest.iter().all(capital))
        }
    };
    match part {
        [first, second, ..] if first.is_lowercase() && second.is_uppercase() => shaped(&part[1..]),
        _ => shaped(part),
    }
}

mod ocr {
    use eframe::egui::{Pos2, Rect};
    use image::RgbaImage;
    use windows::Graphics::Imaging::{BitmapPixelFormat, SoftwareBitmap};
    use windows::Media::Ocr::OcrEngine;
    use windows::Storage::Streams::DataWriter;

    /// Regions up to this many pixels are read at twice the size: Windows recognises
    /// small screen text much better that way. Larger ones are read as they are, which
    /// keeps a whole screen quick.
    const UPSCALE_UP_TO: u32 = 2_500_000;

    /// The recognised lines, or `None` when Windows has no recognition for any of the
    /// user's languages.
    pub fn recognize(img: &RgbaImage) -> windows::core::Result<Option<Vec<super::Line>>> {
        let Ok(engine) = OcrEngine::TryCreateFromUserProfileLanguages() else { return Ok(None) };
        let max = OcrEngine::MaxImageDimension()?;
        let (w, h) = img.dimensions();
        if w == 0 || h == 0 {
            return Ok(Some(Vec::new()));
        }
        // Shrunk only if Windows' size limit demands it.
        let factor = if w * h <= UPSCALE_UP_TO && w.max(h) * 2 <= max { 2.0 } else { (max as f32 / w.max(h) as f32).min(1.0) };
        let scaled;
        let img = if factor == 1.0 {
            img
        } else {
            let (sw, sh) = (((w as f32 * factor) as u32).max(1), ((h as f32 * factor) as u32).max(1));
            scaled = image::imageops::resize(img, sw, sh, image::imageops::FilterType::CatmullRom);
            &scaled
        };

        // Blue, green, red, alpha, on white where the capture is transparent.
        let mut bgra = Vec::with_capacity(img.as_raw().len());
        for px in img.pixels() {
            let [r, g, b, a] = px.0;
            let on_white = |c: u8| ((u32::from(c) * u32::from(a) + 255 * (255 - u32::from(a))) / 255) as u8;
            bgra.extend_from_slice(&[on_white(b), on_white(g), on_white(r), 255]);
        }
        let writer = DataWriter::new()?;
        writer.WriteBytes(&bgra)?;
        let buffer = writer.DetachBuffer()?;
        let bitmap = SoftwareBitmap::CreateCopyFromBuffer(&buffer, BitmapPixelFormat::Bgra8, img.width() as i32, img.height() as i32)?;
        let result = engine.RecognizeAsync(&bitmap)?.join()?;
        // Tilted text is straightened before it is read, and its boxes are given in the
        // straightened image: turned back by the text's angle round the image's centre.
        // Turned forward again, they sit on the text, at its angle.
        let angle = result.TextAngle().ok().and_then(|a| a.Value().ok()).unwrap_or(0.0) as f32;
        let (sin, cos) = angle.to_radians().sin_cos();
        let centre = Pos2::new(img.width() as f32 / 2.0, img.height() as f32 / 2.0);
        let place = |p: Pos2| {
            let v = p - centre;
            // And back to the region's own pixels.
            (centre + eframe::egui::vec2(cos * v.x - sin * v.y, sin * v.x + cos * v.y)) / factor
        };
        let lines = result.Lines()?;
        let mut found = Vec::new();
        for i in 0..lines.Size()? {
            let line = lines.GetAt(i)?;
            let words = line.Words()?;
            let mut boxes = Vec::new();
            let mut straight = Rect::NOTHING;
            for j in 0..words.Size()? {
                let word = words.GetAt(j)?;
                let r = word.BoundingRect()?;
                let rect = Rect::from_min_size(Pos2::new(r.X, r.Y), eframe::egui::vec2(r.Width, r.Height));
                straight = straight.union(rect);
                let quad = super::Quad::from_rect(rect).map(place);
                boxes.push(super::Word { text: word.Text()?.to_string_lossy(), quad });
            }
            let quad = super::Quad::from_rect(straight).map(place);
            found.push(super::Line { text: line.Text()?.to_string_lossy(), words: boxes, quad });
        }
        Ok(Some(found))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::annotate::{Annotation, ShapeKind};
    use eframe::egui::{Color32, Pos2};

    /// Black text on white, drawn the way a text annotation is.
    fn written(lines: &str, size: f32) -> RgbaImage {
        let mut img = RgbaImage::from_pixel(900, 40 + (size * 1.6) as u32 * lines.lines().count() as u32, image::Rgba([255; 4]));
        let mut text = Annotation::stroked(ShapeKind::Text, Pos2::new(20.0, 20.0), Pos2::new(20.0, 20.0), Color32::BLACK, size, Default::default());
        text.text = lines.to_owned();
        crate::annotate::render_onto(&mut img, Pos2::ZERO, &[text]);
        img
    }

    #[test]
    fn reads_text_line_by_line() {
        let img = written("Privacy first screenshots\nNothing is uploaded", 28.0);
        match text(&img) {
            Ok(lines) => {
                assert_eq!(lines.len(), 2, "{lines:?}");
                assert_eq!(lines[0].text, "Privacy first screenshots");
                assert_eq!(lines[1].text, "Nothing is uploaded");
                // Word boxes are in the image's own pixels, left to right, line under line.
                let words: Vec<&str> = lines[0].words.iter().map(|w| w.text.as_str()).collect();
                assert_eq!(words, ["Privacy", "first", "screenshots"]);
                let (a, b) = (lines[0].words[0].rect(), lines[0].words[1].rect());
                assert!(a.left() >= 15.0 && a.left() < 30.0 && a.top() >= 15.0, "{a:?}");
                assert!(a.right() < b.left() && (a.center().y - b.center().y).abs() < 4.0, "{a:?} {b:?}");
                assert!(lines[1].rect().top() > lines[0].rect().bottom(), "{lines:?}");
                assert!(lines[0].rect().right() < img.width() as f32);
                assert_eq!(joined(&lines), "Privacy first screenshots\nNothing is uploaded");
            }
            // A Windows without any OCR language cannot run this test.
            Err(e) if e.starts_with("Text recognition is not") => eprintln!("skipped: {e}"),
            Err(e) => panic!("{e}"),
        }
    }

    #[test]
    fn reads_text_sideways_and_upside_down() {
        let upright = written("Cappuccino large 4.50\nBlueberry muffin 3.25\nTotal 7.75", 22.0);
        let (w, h) = (upright.width() as f32, upright.height() as f32);
        for turn in Turn::ALL {
            let img = turn.apply(&upright);
            let lines = match text(&img) {
                Ok(lines) => lines,
                Err(e) if e.starts_with("Text recognition is not") => return eprintln!("skipped: {e}"),
                Err(e) => panic!("{turn:?}: {e}"),
            };
            assert_eq!(joined(&lines), "Cappuccino large 4.50\nBlueberry muffin 3.25\nTotal 7.75", "{turn:?}");
            // "Cappuccino" is where it is in the turned image: long along the turn.
            let word = lines[0].words[0].rect();
            let (iw, ih) = (img.width() as f32, img.height() as f32);
            assert!(word.min.x >= -1.0 && word.min.y >= -1.0 && word.max.x <= iw + 1.0 && word.max.y <= ih + 1.0, "{turn:?} {word:?}");
            match turn {
                Turn::Half => {
                    assert!(word.width() > word.height() && word.center().y > h / 2.0 && word.center().x > w / 2.0, "{word:?}")
                }
                Turn::Quarter => assert!(word.height() > word.width() && word.center().x > h / 2.0, "{word:?}"),
                Turn::ThreeQuarter => assert!(word.height() > word.width() && word.center().y > w / 2.0, "{word:?}"),
            }
            // And covers ink.
            let c = word.center();
            let dark = (-3..=3)
                .flat_map(|d| [(c.x + d as f32, c.y), (c.x, c.y + d as f32)])
                .any(|(x, y)| img.get_pixel(x as u32, y as u32).0[0] < 128);
            assert!(dark, "{turn:?} {word:?}");
        }
    }

    /// Text at an angle is outlined at that angle, on the words.
    #[test]
    fn tilted_text_is_outlined_at_its_angle() {
        let upright = written("COFFEE HOUSE Main\nCappuccino large", 30.0);
        let upright_lines = match text(&upright) {
            Ok(lines) => lines,
            Err(e) if e.starts_with("Text recognition is not") => return eprintln!("skipped: {e}"),
            Err(e) => panic!("{e}"),
        };
        let (w, h) = (upright.width() as f32, upright.height() as f32);
        let size = (w * w + h * h).sqrt().ceil() as u32 + 20;
        let (c, half) = (Pos2::new(w / 2.0, h / 2.0), size as f32 / 2.0);
        for degrees in [12.0f32, -20.0] {
            let (sin, cos) = degrees.to_radians().sin_cos();
            // Where an upright point lands, turned clockwise round the middle of a
            // larger white page.
            let turn = |p: Pos2| {
                let v = p - c;
                Pos2::new(half + cos * v.x - sin * v.y, half + sin * v.x + cos * v.y)
            };
            let img = RgbaImage::from_fn(size, size, |x, y| {
                let v = Pos2::new(x as f32 + 0.5 - half, y as f32 + 0.5 - half);
                let s = c + eframe::egui::vec2(cos * v.x + sin * v.y, -sin * v.x + cos * v.y);
                if s.x < 0.0 || s.y < 0.0 || s.x >= w || s.y >= h { image::Rgba([255; 4]) } else { *upright.get_pixel(s.x as u32, s.y as u32) }
            });
            let lines = text(&img).unwrap();
            assert_eq!(joined(&lines), joined(&upright_lines), "{degrees}");
            for (line, want) in lines.iter().zip(&upright_lines) {
                for (word, want) in line.words.iter().zip(&want.words) {
                    let [a, b, ..] = word.quad.0;
                    let tilt = (b - a).y.atan2((b - a).x).to_degrees();
                    assert!((tilt - degrees).abs() < 2.0, "{} tilted {tilt}, not {degrees}", word.text);
                    let centre = Rect::from_points(&word.quad.0).center();
                    let true_centre = turn(want.rect().center());
                    assert!(centre.distance(true_centre) < 6.0, "{} at {centre:?}, not {true_centre:?}", word.text);
                    assert!(word.quad.contains(true_centre));
                }
            }
        }
    }

    #[test]
    fn quads_contain_and_grow() {
        let q = Quad([Pos2::new(10.0, 0.0), Pos2::new(20.0, 10.0), Pos2::new(10.0, 20.0), Pos2::new(0.0, 10.0)]);
        assert!(q.contains(Pos2::new(10.0, 10.0)));
        assert!(!q.contains(Pos2::new(2.0, 2.0)));
        assert!(q.expand(3.0, 3.0).contains(Pos2::new(3.0, 5.0)));
        assert!((q.height() - 200f32.sqrt()).abs() < 1e-4);
    }

    /// After the user turned the capture by hand, text is read as it is, never turned.
    #[test]
    fn read_as_is_never_turns() {
        let upside_down = Turn::Half.apply(&written("Cappuccino large 4.50
Blueberry muffin 3.25", 22.0));
        match read(Kind::Text, &upside_down, false) {
            Ok(lines) => assert_eq!(joined(&lines), "Cappuccino large 4.50
Blueberry muffin 3.25"),
            Err(e) if e.starts_with("Text recognition is not") => return eprintln!("skipped: {e}"),
            Err(e) => panic!("{e}"),
        }
        let as_is = read(Kind::Text, &upside_down, true).map(|l| joined(&l)).unwrap_or_default();
        assert!(!as_is.contains("Cappuccino"), "{as_is}");
    }

    #[test]
    fn upside_down_readings_look_like_gibberish() {
        let lines = |s: &str| {
            s.lines()
                .map(|l| Line {
                    text: l.to_owned(),
                    words: l.split_whitespace().map(|w| Word { text: w.to_owned(), quad: Quad::from_rect(Rect::ZERO) }).collect(),
                    quad: Quad::from_rect(Rect::ZERO),
                })
                .collect::<Vec<_>>()
        };
        let ratio = |s: &str| {
            let (good, all) = plausibility(&lines(s));
            good as f32 / all.max(1) as f32
        };
        // What Windows really made of an upside-down receipt, and of a sideways one.
        let upside_down = "JO} noÅ >tueqL\nL 6u!pua S C6\n00'? 6u!lveds\nu!JJnw Kneqen18\ne6El oup•ndde)\nu!e1A 3SnOH 33430)";
        assert!(ratio(upside_down) < 0.3, "{}", ratio(upside_down));
        assert_eq!(plausibility(&lines("3\nC\no\nO\no\nO")), (0, 0));
        // Real text: words, prices, times, dates, and Chinese.
        let real = "COFFEE HOUSE 24 Main Street\nCappuccino large 4.50\nTotal: 9.75 (card)\nOpen 07:30-18:00\nDon't forget 12/03/2026";
        assert!(ratio(real) > 0.9, "{}", ratio(real));
        assert_eq!(plausibility(&lines("咖 啡")), (2, 2));
        // Serial and model numbers, product names and units.
        for code in ["2KR192C22FG", "S/N: 2KR192C22FG", "Model XG-4410B", "iPhone 15 Pro A2848", "SKU 88213-B4", "500ml 2nd eBay"] {
            assert_eq!(ratio(code), 1.0, "{code}");
        }
        // What Windows made of "iPhone 15 Pro A2848" upside down: mostly gibberish.
        assert!(ratio("Old SL öUOLld!") < 0.6);
        assert!(ratio("8t78ZV Old SL euoqd!") < 0.85);
        assert!(ratio("Zb6>lEJLe\nt8-ELZ88 rl>lS") < 0.3);
    }

    /// A serial number read upright stays upright, whatever the turned readings make
    /// of it, and one turned any way is turned back.
    #[test]
    fn reads_serial_numbers_any_way_up() {
        for s in ["iPhone 15 Pro A2848", "S/N 2KR192C22FG", "Model XG-4410B"] {
            for size in [16.0, 24.0] {
                let upright = written(s, size);
                let turned = Turn::ALL.map(|t| t.apply(&upright));
                for img in std::iter::once(&upright).chain(&turned) {
                    match text(&img) {
                        Ok(lines) => assert_eq!(joined(&lines), s, "{size}px"),
                        Err(e) if e.starts_with("Text recognition is not") => return eprintln!("skipped: {e}"),
                        Err(e) => panic!("{s} {size}px: {e}"),
                    }
                }
            }
        }
        // A lowercase code is not "plausible", but is still read as it is.
        let hash = written("REF a7f3k9q2", 24.0);
        let as_is = joined(&ocr::recognize(&hash).unwrap().unwrap());
        assert_eq!(text(&hash).map(|l| joined(&l)), Ok(as_is));
    }

    #[test]
    fn small_text_and_nothing_at_all() {
        let small = written("Small text at fourteen pixels", 14.0);
        if let Ok(lines) = text(&small) {
            assert_eq!(joined(&lines), "Small text at fourteen pixels");
        }
        let blank = RgbaImage::from_pixel(200, 80, image::Rgba([255; 4]));
        assert!(matches!(text(&blank), Err(e) if e.starts_with("No text") || e.starts_with("Text recognition is not")));
    }

    /// A QR code for `content`, `scale` pixels per module, with a quiet zone.
    fn qr_image(content: &str, scale: u32) -> RgbaImage {
        let code = qrcode::QrCode::new(content.as_bytes()).unwrap();
        let width = code.width() as u32;
        let colors = code.to_colors();
        let quiet = 4;
        let size = (width + 2 * quiet) * scale;
        RgbaImage::from_fn(size, size, |x, y| {
            let (mx, my) = ((x / scale) as i64 - i64::from(quiet), (y / scale) as i64 - i64::from(quiet));
            let inside = (0..i64::from(width)).contains(&mx) && (0..i64::from(width)).contains(&my);
            let dark = inside && colors[(my * i64::from(width) + mx) as usize] == qrcode::Color::Dark;
            image::Rgba(if dark { [0, 0, 0, 255] } else { [255, 255, 255, 255] })
        })
    }

    #[test]
    fn reads_qr_codes() {
        let url = "https://example.com/darkshot?q=1";
        let one = qr(&qr_image(url, 4)).unwrap();
        assert_eq!(joined(&one), url);
        // Its box covers the code, inside the quiet zone.
        let r = one[0].rect();
        assert!((r.left() - 16.0).abs() <= 2.0 && (r.width() - 4.0 * 29.0).abs() <= 6.0, "{r:?}");

        // Two codes side by side on a grey page: both are read.
        let (a, b) = (qr_image("first", 5), qr_image("second", 5));
        let mut page = RgbaImage::from_pixel(a.width() + b.width() + 60, a.height().max(b.height()) + 40, image::Rgba([200, 200, 200, 255]));
        image::imageops::replace(&mut page, &a, 20, 20);
        image::imageops::replace(&mut page, &b, i64::from(a.width()) + 40, 20);
        let found = qr(&page).unwrap();
        let texts: Vec<&str> = found.iter().map(|l| l.text.as_str()).collect();
        assert_eq!(texts, ["first", "second"]);

        // No code: says so.
        assert!(qr(&RgbaImage::from_pixel(100, 100, image::Rgba([255; 4]))).unwrap_err().starts_with("No QR code"));
    }

    /// A styled code: its data modules drawn as small blue dots, finder squares black.
    /// Read as it is, it does not decode; the extra passes read it.
    #[test]
    fn reads_styled_qr_codes() {
        let content = "https://example.com/styled";
        let code = qrcode::QrCode::new(content.as_bytes()).unwrap();
        let width = i64::from(code.width() as u32);
        let colors = code.to_colors();
        let (scale, quiet) = (4u32, 4i64);
        let finder = |mx: i64, my: i64| (mx < 7 || mx >= width - 7) && my < 7 || mx < 7 && my >= width - 7;
        let size = (width as u32 + 2 * quiet as u32) * scale;
        let img = RgbaImage::from_fn(size, size, |x, y| {
            let (mx, my) = ((x / scale) as i64 - quiet, (y / scale) as i64 - quiet);
            let inside = (0..width).contains(&mx) && (0..width).contains(&my);
            if !inside || colors[(my * width + mx) as usize] != qrcode::Color::Dark {
                return image::Rgba([255; 4]);
            }
            if finder(mx, my) {
                return image::Rgba([0, 0, 0, 255]);
            }
            let centre = scale as f32 / 2.0;
            let (dx, dy) = ((x % scale) as f32 + 0.5 - centre, (y % scale) as f32 + 0.5 - centre);
            let dot = (dx * dx + dy * dy).sqrt() <= 0.55 * centre;
            image::Rgba(if dot { [0x1f, 0x5f, 0xa0, 255] } else { [255; 4] })
        });
        let as_is = {
            let pass = &QR_PASSES[0];
            assert!(pass.scale == 1 && pass.threshold.is_none());
            let grey = image::imageops::grayscale(&img);
            let mut p = rqrr::PreparedImage::prepare_from_greyscale(size as usize, size as usize, |x, y| grey.get_pixel(x as u32, y as u32).0[0]);
            p.detect_grids().iter().any(|g| g.decode().is_ok())
        };
        assert!(!as_is, "the test code should need the extra passes");
        let found = qr(&img).unwrap();
        assert_eq!(joined(&found), content);
        // Its box is in the region's pixels, whichever pass read it.
        assert!(found[0].rect().right() <= size as f32 + 1.0, "{:?}", found[0].rect());
    }
}

