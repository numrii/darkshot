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
    /// Its words, each with its box in the image's pixels. A QR code is one "word".
    pub words: Vec<Word>,
}

#[derive(Clone, Debug, PartialEq)]
pub struct Word {
    pub text: String,
    pub rect: Rect,
}

impl Line {
    /// The box around all its words.
    pub fn rect(&self) -> Rect {
        self.words.iter().fold(Rect::NOTHING, |r, w| r.union(w.rect))
    }
}

/// What reading found, in reading order, or why there is nothing.
pub type Outcome = Result<Vec<Line>, String>;

/// The text to copy for `lines`: each line on its own.
pub fn joined(lines: &[Line]) -> String {
    lines.iter().map(|l| l.text.as_str()).collect::<Vec<_>>().join("\n")
}

/// Read `img` for `kind`. Can take a moment: call it off the UI thread.
pub fn read(kind: Kind, img: &RgbaImage) -> Outcome {
    match kind {
        Kind::Text => text(img),
        Kind::Qr => qr(img),
    }
}

/// The text in `img`, one line per line of text, or why there is none.
pub fn text(img: &RgbaImage) -> Outcome {
    match ocr::recognize(img) {
        Err(e) => Err(format!("Text recognition failed: {}", e.message())),
        Ok(None) => Err("Text recognition is not installed for your languages. Windows adds it with a \
                         language under Settings > Time & language > Language & region."
            .to_owned()),
        Ok(Some(lines)) if lines.iter().all(|l| l.text.trim().is_empty()) => Err("No text found in the region.".to_owned()),
        Ok(Some(lines)) => Ok(lines),
    }
}

/// The contents of every QR code in `img`, top to bottom, or why there are none.
pub fn qr(img: &RgbaImage) -> Outcome {
    let (w, h) = img.dimensions();
    let luma = |x: usize, y: usize| {
        let [r, g, b, a] = img.get_pixel(x as u32, y as u32).0;
        // Transparent areas (off every monitor) count as white, like paper.
        let grey = (u32::from(r) * 299 + u32::from(g) * 587 + u32::from(b) * 114) / 1000;
        ((grey * u32::from(a) + 255 * (255 - u32::from(a))) / 255) as u8
    };
    let mut prepared = rqrr::PreparedImage::prepare_from_greyscale(w as usize, h as usize, luma);
    let grids = prepared.detect_grids();
    if grids.is_empty() {
        return Err("No QR code found in the region.".to_owned());
    }
    let mut found: Vec<Line> = grids
        .iter()
        .filter_map(|grid| {
            let (_, content) = grid.decode().ok()?;
            let corners = grid.bounds.map(|p| Pos2::new(p.x as f32, p.y as f32));
            let rect = Rect::from_points(&corners);
            Some(Line { text: content.clone(), words: vec![Word { text: content, rect }] })
        })
        .collect();
    if found.is_empty() {
        return Err("Found a QR code, but could not read it. Try selecting just the code.".to_owned());
    }
    found.sort_by(|a, b| (a.rect().top(), a.rect().left()).partial_cmp(&(b.rect().top(), b.rect().left())).unwrap());
    Ok(found)
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
        let lines = result.Lines()?;
        let mut found = Vec::new();
        for i in 0..lines.Size()? {
            let line = lines.GetAt(i)?;
            let words = line.Words()?;
            let mut boxes = Vec::new();
            for j in 0..words.Size()? {
                let word = words.GetAt(j)?;
                let r = word.BoundingRect()?;
                // Back to the region's own pixels.
                let rect = Rect::from_min_size(Pos2::new(r.X, r.Y), eframe::egui::vec2(r.Width, r.Height));
                let rect = Rect::from_min_max((rect.min.to_vec2() / factor).to_pos2(), (rect.max.to_vec2() / factor).to_pos2());
                boxes.push(super::Word { text: word.Text()?.to_string_lossy(), rect });
            }
            found.push(super::Line { text: line.Text()?.to_string_lossy(), words: boxes });
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
                let (a, b) = (lines[0].words[0].rect, lines[0].words[1].rect);
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
}
