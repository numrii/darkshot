//! Reopen last capture: when a capture started from the tray closes, it writes what it
//! needs to come back (the screenshot, region, drawings, undo history, and the tool and
//! its style) to the tray through a pipe, and quits. The tray keeps those bytes in
//! memory only, never on disk, for the minutes set in the settings, and starts a new
//! capture window with them when asked. So nothing but the bytes stays behind: no
//! window, no graphics device.
//!
//! The bytes are a plain format of their own, read only by the same build that wrote
//! them (both ends are one exe): numbers little-endian, lists and text with their length
//! first.

use eframe::egui::{Color32, Pos2, Rect, Vec2};

use crate::annotate::{Annotation, ArrowHead, Dash, Effect, ShapeKind, TextStyle};

/// What a reader gives up with: the bytes are cut short or not what was written.
pub type Result<T> = std::result::Result<T, &'static str>;

/// Marks the start of a kept capture, with the version of its layout.
pub const MAGIC: &[u8; 4] = b"DSK1";

#[derive(Default)]
pub struct Writer(pub Vec<u8>);

impl Writer {
    pub fn u8(&mut self, v: u8) {
        self.0.push(v);
    }
    pub fn bool(&mut self, v: bool) {
        self.u8(u8::from(v));
    }
    pub fn u32(&mut self, v: u32) {
        self.0.extend_from_slice(&v.to_le_bytes());
    }
    pub fn i32(&mut self, v: i32) {
        self.0.extend_from_slice(&v.to_le_bytes());
    }
    pub fn len(&mut self, n: usize) {
        self.u32(u32::try_from(n).unwrap_or(u32::MAX));
    }
    pub fn f32(&mut self, v: f32) {
        self.0.extend_from_slice(&v.to_le_bytes());
    }
    pub fn bytes(&mut self, v: &[u8]) {
        self.len(v.len());
        self.0.extend_from_slice(v);
    }
    pub fn str(&mut self, v: &str) {
        self.bytes(v.as_bytes());
    }
    pub fn vec2(&mut self, v: Vec2) {
        self.f32(v.x);
        self.f32(v.y);
    }
    pub fn pos(&mut self, p: Pos2) {
        self.vec2(p.to_vec2());
    }
    pub fn rect(&mut self, r: Rect) {
        self.pos(r.min);
        self.pos(r.max);
    }
    pub fn color(&mut self, c: Color32) {
        self.0.extend_from_slice(&c.to_array());
    }
    pub fn option<T>(&mut self, v: Option<T>, mut put: impl FnMut(&mut Self, T)) {
        self.bool(v.is_some());
        if let Some(v) = v {
            put(self, v);
        }
    }
    pub fn list<T>(&mut self, items: &[T], mut put: impl FnMut(&mut Self, &T)) {
        self.len(items.len());
        for item in items {
            put(self, item);
        }
    }
    /// One of `all`, by its place there.
    pub fn choice<T: PartialEq>(&mut self, v: &T, all: &[T]) {
        self.u8(all.iter().position(|a| a == v).unwrap_or(0) as u8);
    }

    pub fn annotation(&mut self, a: &Annotation) {
        self.choice(&a.kind, &SHAPES);
        self.pos(a.a);
        self.pos(a.b);
        self.color(a.color);
        self.f32(a.width);
        self.choice(&a.dash, &Dash::ALL.map(|d| d.0));
        self.choice(&a.head, &ArrowHead::ALL.map(|h| h.0));
        self.bool(a.double_head);
        self.list(&a.points, |w, p| w.pos(*p));
        self.str(&a.text);
        self.choice(&a.text_style, &TextStyle::ALL.map(|t| t.0));
        self.choice(&a.effect, &Effect::ALL.map(|e| e.0));
    }

    pub fn annotations(&mut self, list: &[Annotation]) {
        self.list(list, |w, a| w.annotation(a));
    }
}

pub struct Reader<'a> {
    data: &'a [u8],
    at: usize,
}

impl<'a> Reader<'a> {
    pub fn new(data: &'a [u8]) -> Self {
        Self { data, at: 0 }
    }
    pub fn take(&mut self, n: usize) -> Result<&'a [u8]> {
        let end = self.at.checked_add(n).filter(|&e| e <= self.data.len()).ok_or("cut short")?;
        let out = &self.data[self.at..end];
        self.at = end;
        Ok(out)
    }
    fn array<const N: usize>(&mut self) -> Result<[u8; N]> {
        Ok(self.take(N)?.try_into().expect("N bytes"))
    }
    pub fn u8(&mut self) -> Result<u8> {
        Ok(self.take(1)?[0])
    }
    pub fn bool(&mut self) -> Result<bool> {
        Ok(self.u8()? != 0)
    }
    pub fn u32(&mut self) -> Result<u32> {
        Ok(u32::from_le_bytes(self.array()?))
    }
    pub fn i32(&mut self) -> Result<i32> {
        Ok(i32::from_le_bytes(self.array()?))
    }
    /// A length, no longer than what is left (each item takes at least a byte).
    pub fn len(&mut self) -> Result<usize> {
        let n = self.u32()? as usize;
        if n > self.data.len() - self.at {
            return Err("length past the end");
        }
        Ok(n)
    }
    pub fn f32(&mut self) -> Result<f32> {
        Ok(f32::from_le_bytes(self.array()?))
    }
    pub fn bytes(&mut self) -> Result<&'a [u8]> {
        let n = self.len()?;
        self.take(n)
    }
    pub fn str(&mut self) -> Result<String> {
        String::from_utf8(self.bytes()?.to_vec()).map_err(|_| "not text")
    }
    pub fn vec2(&mut self) -> Result<Vec2> {
        Ok(Vec2::new(self.f32()?, self.f32()?))
    }
    pub fn pos(&mut self) -> Result<Pos2> {
        Ok(self.vec2()?.to_pos2())
    }
    pub fn rect(&mut self) -> Result<Rect> {
        Ok(Rect::from_min_max(self.pos()?, self.pos()?))
    }
    pub fn color(&mut self) -> Result<Color32> {
        let [r, g, b, a] = self.array()?;
        Ok(Color32::from_rgba_premultiplied(r, g, b, a))
    }
    pub fn option<T>(&mut self, get: impl FnOnce(&mut Self) -> Result<T>) -> Result<Option<T>> {
        if self.bool()? { get(self).map(Some) } else { Ok(None) }
    }
    pub fn list<T>(&mut self, mut get: impl FnMut(&mut Self) -> Result<T>) -> Result<Vec<T>> {
        let n = self.len()?;
        (0..n).map(|_| get(self)).collect()
    }
    pub fn choice<T: Copy>(&mut self, all: &[T]) -> Result<T> {
        all.get(self.u8()? as usize).copied().ok_or("unknown choice")
    }

    pub fn annotation(&mut self) -> Result<Annotation> {
        Ok(Annotation {
            kind: self.choice(&SHAPES)?,
            a: self.pos()?,
            b: self.pos()?,
            color: self.color()?,
            width: self.f32()?,
            dash: self.choice(&Dash::ALL.map(|d| d.0))?,
            head: self.choice(&ArrowHead::ALL.map(|h| h.0))?,
            double_head: self.bool()?,
            points: self.list(|r| r.pos())?,
            text: self.str()?,
            text_style: self.choice(&TextStyle::ALL.map(|t| t.0))?,
            effect: self.choice(&Effect::ALL.map(|e| e.0))?,
        })
    }

    pub fn annotations(&mut self) -> Result<Vec<Annotation>> {
        self.list(|r| r.annotation())
    }

    pub fn done(&self) -> Result<()> {
        if self.at == self.data.len() { Ok(()) } else { Err("bytes left over") }
    }
}

/// Every kind of annotation, in a fixed order for the bytes.
pub const SHAPES: [ShapeKind; 10] = [
    ShapeKind::Rect,
    ShapeKind::Ellipse,
    ShapeKind::Line,
    ShapeKind::Arrow,
    ShapeKind::Pen,
    ShapeKind::Marker,
    ShapeKind::Text,
    ShapeKind::Blur,
    ShapeKind::Step,
    ShapeKind::Emoji,
];

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn round_trip_and_damaged_bytes() {
        let mut shape = Annotation::stroked(ShapeKind::Arrow, Pos2::new(1.5, -2.0), Pos2::new(30.0, 40.25), Color32::from_rgba_premultiplied(10, 20, 30, 40), 3.0, Dash::DashDot);
        shape.points = vec![Pos2::new(0.1, 0.2), Pos2::new(7.0, 8.0)];
        shape.text = "Hej (●'◡'●) 👍🏽".into();
        shape.head = ArrowHead::Stealth;
        shape.double_head = true;
        shape.text_style = TextStyle::Box;
        shape.effect = Effect::Redact;
        let mut w = Writer::default();
        w.annotations(&[shape.clone(), Annotation::stroked(ShapeKind::Emoji, Pos2::ZERO, Pos2::ZERO, Color32::RED, 48.0, Dash::Solid)]);
        w.option(Some(Rect::from_min_max(Pos2::new(1.0, 2.0), Pos2::new(3.0, 4.0))), |w, r| w.rect(r));
        w.i32(-1920);
        let mut r = Reader::new(&w.0);
        let back = r.annotations().unwrap();
        assert_eq!(back[0], shape);
        assert_eq!(back[1].kind, ShapeKind::Emoji);
        assert_eq!(r.option(|r| r.rect()).unwrap().unwrap().max, Pos2::new(3.0, 4.0));
        assert_eq!(r.i32().unwrap(), -1920);
        r.done().unwrap();

        // Cut short anywhere, or with a length past the end: an error, never a panic.
        let read_all = |bytes: &[u8]| -> Result<()> {
            let mut r = Reader::new(bytes);
            r.annotations()?;
            r.option(|r| r.rect())?;
            r.i32()?;
            r.done()
        };
        assert!(read_all(&w.0).is_ok());
        for end in 0..w.0.len() {
            assert!(read_all(&w.0[..end]).is_err(), "{end}");
        }
        let mut huge = Writer::default();
        huge.u32(u32::MAX);
        assert!(Reader::new(&huge.0).annotations().is_err());
    }
}
