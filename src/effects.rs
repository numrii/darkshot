//! Pixel effects: pixelate and blur for hiding parts of a capture, and the border and
//! drop shadow that can be added around the finished image.

use eframe::egui::{Color32, Pos2, Vec2};
use image::RgbaImage;

use crate::annotate::{Annotation, Dash, ShapeKind};

/// Replace each `block` x `block` square (aligned to the image's top-left) with its
/// average colour.
pub fn pixelate(img: &mut RgbaImage, block: u32) {
    let block = block.max(2);
    let (w, h) = img.dimensions();
    for by in (0..h).step_by(block as usize) {
        for bx in (0..w).step_by(block as usize) {
            let (x1, y1) = ((bx + block).min(w), (by + block).min(h));
            let mut sum = [0u64; 4];
            for y in by..y1 {
                for x in bx..x1 {
                    for (s, c) in sum.iter_mut().zip(img.get_pixel(x, y).0) {
                        *s += u64::from(c);
                    }
                }
            }
            let n = u64::from((x1 - bx) * (y1 - by));
            let avg = image::Rgba(sum.map(|s| ((s + n / 2) / n) as u8));
            for y in by..y1 {
                for x in bx..x1 {
                    img.put_pixel(x, y, avg);
                }
            }
        }
    }
}

/// Approximate a Gaussian blur of the given radius with three box blurs per axis.
/// Edges are clamped, so colours from outside the image do not bleed in.
pub fn blur(img: &mut RgbaImage, radius: u32) {
    let radius = radius.max(1) as usize;
    let (w, h) = (img.width() as usize, img.height() as usize);
    if w == 0 || h == 0 {
        return;
    }
    // Three passes of a box of this half-width give roughly a Gaussian of `radius`.
    let half = ((radius as f32) / 1.7).round().max(1.0) as usize;
    let mut buf: Vec<[f32; 4]> =
        img.pixels().map(|p| p.0.map(f32::from)).collect();
    let mut tmp = buf.clone();
    for _ in 0..3 {
        box_pass(&buf, &mut tmp, w, h, half, true);
        box_pass(&tmp, &mut buf, w, h, half, false);
    }
    for (p, v) in img.pixels_mut().zip(buf) {
        p.0 = v.map(|c| c.round().clamp(0.0, 255.0) as u8);
    }
}

/// One running-sum box blur along rows (`horizontal`) or columns.
fn box_pass(src: &[[f32; 4]], dst: &mut [[f32; 4]], w: usize, h: usize, half: usize, horizontal: bool) {
    let (lines, len) = if horizontal { (h, w) } else { (w, h) };
    let at = |line: usize, i: usize| if horizontal { line * w + i } else { i * w + line };
    let norm = 1.0 / (2 * half + 1) as f32;
    for line in 0..lines {
        let clamp = |i: isize| i.clamp(0, len as isize - 1) as usize;
        let mut sum = [0.0f32; 4];
        for i in -(half as isize)..=(half as isize) {
            let p = src[at(line, clamp(i))];
            for c in 0..4 {
                sum[c] += p[c];
            }
        }
        for i in 0..len {
            dst[at(line, i)] = sum.map(|s| s * norm);
            let add = src[at(line, clamp(i as isize + half as isize + 1))];
            let sub = src[at(line, clamp(i as isize - half as isize))];
            for c in 0..4 {
                sum[c] += add[c] - sub[c];
            }
        }
    }
}

/// A border around the image, drawn like a rectangle annotation.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Border {
    pub color: Color32,
    /// Thickness in pixels.
    pub width: f32,
    pub dash: Dash,
}

/// What to add around an image when it is copied or saved.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct Frame {
    pub border: Option<Border>,
    /// A soft drop shadow, in a margin around the image.
    pub shadow: bool,
}

impl Frame {
    /// Nothing is added.
    pub fn is_empty(&self) -> bool {
        self.border.is_none() && !self.shadow
    }
}

/// The margin on every side of an image with a shadow.
const SHADOW_MARGIN: u32 = 28;
/// How far the shadow sits below the image.
const SHADOW_OFFSET: u32 = 6;
/// The half-width of each of the three box blurs that soften the shadow's edge.
const SHADOW_SOFTNESS: usize = 5;
/// How dark the shadow is at its darkest, 0-1.
const SHADOW_OPACITY: f32 = 0.38;

/// `img` with the border and shadow of `frame` around it. Both are added outside the
/// image, so they never cover any of it. What they leave uncovered (the gaps of a
/// dashed border, the shadow's margin) is transparent; see [`flatten`].
pub fn frame(mut img: RgbaImage, frame: &Frame) -> RgbaImage {
    if let Some(border) = frame.border {
        let width = border.width.round().max(1.0);
        let pad = width as u32;
        let mut out = RgbaImage::new(img.width() + 2 * pad, img.height() + 2 * pad);
        image::imageops::replace(&mut out, &img, i64::from(pad), i64::from(pad));
        // A rectangle whose stroke, centred on its outline, exactly fills the band.
        let half = Vec2::splat(width / 2.0);
        let size = Vec2::new(out.width() as f32, out.height() as f32);
        let band =
            Annotation::stroked(ShapeKind::Rect, Pos2::ZERO + half, Pos2::ZERO + size - half, border.color, width, border.dash);
        crate::annotate::render_onto(&mut out, Pos2::ZERO, &[band]);
        img = out;
    }
    if frame.shadow {
        let (w, h) = img.dimensions();
        let (m, lift) = (SHADOW_MARGIN, SHADOW_OFFSET);
        // A blurred rectangle is the product of two blurred 1D boxes, so the shadow
        // needs no blur over the whole image.
        let cover_x = soft_box((w + 2 * m) as usize, m as usize, w as usize);
        let cover_y = soft_box((h + 2 * m) as usize, (m + lift) as usize, h as usize);
        let mut out = RgbaImage::from_fn(w + 2 * m, h + 2 * m, |x, y| {
            let dark = SHADOW_OPACITY * cover_x[x as usize] * cover_y[y as usize];
            image::Rgba([0, 0, 0, (255.0 * dark).round() as u8])
        });
        image::imageops::overlay(&mut out, &img, i64::from(m), i64::from(m));
        img = out;
    }
    img
}

/// Put `img` on white, leaving nothing transparent. For the clipboard: many programs
/// paste transparency as black, which would turn a shadow's margin into a black slab.
/// (Saving as JPEG does the same; a PNG file keeps the transparency.)
pub fn flatten(img: &mut RgbaImage) {
    for px in img.pixels_mut() {
        let a = u32::from(px.0[3]);
        if a != 255 {
            for c in &mut px.0[..3] {
                *c = ((u32::from(*c) * a + 255 * (255 - a) + 127) / 255) as u8;
            }
            px.0[3] = 255;
        }
    }
}

/// A line of `len` values that is 1 on `start..start + size` and 0 elsewhere, with
/// its two steps softened.
fn soft_box(len: usize, start: usize, size: usize) -> Vec<f32> {
    let mut line: Vec<f32> = (0..len).map(|i| if (start..start + size).contains(&i) { 1.0 } else { 0.0 }).collect();
    let half = SHADOW_SOFTNESS as isize;
    for _ in 0..3 {
        let src = line.clone();
        for (i, v) in line.iter_mut().enumerate() {
            let window = (i as isize - half)..=(i as isize + half);
            let sum: f32 = window.clone().filter_map(|j| src.get(usize::try_from(j).ok()?)).sum();
            *v = sum / window.count() as f32;
        }
    }
    line
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn border_and_shadow_go_around_the_image() {
        let red = image::Rgba([255, 0, 0, 255]);
        let img = RgbaImage::from_pixel(40, 30, red);
        assert_eq!(frame(img.clone(), &Frame::default()), img);

        let solid = Border { color: Color32::from_rgb(1, 2, 3), width: 2.0, dash: Dash::Solid };
        let bordered = frame(img.clone(), &Frame { border: Some(solid), shadow: false });
        assert_eq!(bordered.dimensions(), (44, 34));
        // The band is filled edge to edge and corner to corner, and the image untouched.
        for (x, y) in [(0, 0), (1, 1), (43, 33), (42, 32), (20, 0), (0, 15), (43, 15), (20, 33)] {
            assert_eq!(bordered.get_pixel(x, y).0, [1, 2, 3, 255], "{x},{y}");
        }
        assert!((2..42).all(|x| (2..32).all(|y| bordered.get_pixel(x, y) == &red)));

        // A dashed border leaves gaps, and the gaps are transparent.
        let dashed = frame(img.clone(), &Frame { border: Some(Border { dash: Dash::Dash, ..solid }), shadow: false });
        let top: Vec<u8> = (0..44).map(|x| dashed.get_pixel(x, 0).0[3]).collect();
        assert!(top.contains(&255) && top.contains(&0), "{top:?}");
        assert!((2..42).all(|x| (2..32).all(|y| dashed.get_pixel(x, y) == &red)));

        let both = frame(img, &Frame { border: Some(solid), shadow: true });
        let m = SHADOW_MARGIN;
        assert_eq!(both.dimensions(), (44 + 2 * m, 34 + 2 * m));
        assert_eq!(both.get_pixel(0, 0).0[3], 0); // the margin's corner is clear
        assert_eq!(both.get_pixel(m, m).0, [1, 2, 3, 255]);
        assert_eq!(both.get_pixel(m + 2, m + 2), &red);
        // Black, darkest just under the image, lighter above it and further down.
        let shade = |y: u32| both.get_pixel(m + 22, y).0;
        let (under, above, far) = (shade(m + 34 + 2), shade(m - 3), shade(m + 34 + 16));
        assert_eq!(under[..3], [0, 0, 0]);
        assert!(under[3] > above[3] && under[3] > 60 && under[3] > far[3], "{under:?} {above:?} {far:?}");

        // On white for the clipboard: opaque everywhere, the corner white, the shadow grey.
        let mut flat = both.clone();
        flatten(&mut flat);
        assert!(flat.pixels().all(|p| p.0[3] == 255));
        assert_eq!(flat.get_pixel(0, 0).0, [255, 255, 255, 255]);
        let grey = flat.get_pixel(m + 22, m + 34 + 2).0;
        assert!(grey[0] < 200 && grey[0] == grey[1] && grey[1] == grey[2], "{grey:?}");
        assert_eq!(flat.get_pixel(m + 2, m + 2), &red);
    }

    fn stripes() -> RgbaImage {
        RgbaImage::from_fn(12, 6, |x, _| {
            if x % 2 == 0 { image::Rgba([0, 0, 0, 255]) } else { image::Rgba([200, 100, 50, 255]) }
        })
    }

    #[test]
    fn pixelate_averages_blocks() {
        let mut img = stripes();
        pixelate(&mut img, 4);
        // Every 4x4 block holds two black and two coloured columns: half of each.
        assert_eq!(img.get_pixel(0, 0).0, [100, 50, 25, 255]);
        assert_eq!(img.get_pixel(3, 3).0, img.get_pixel(0, 0).0);
        // The last column of blocks is clipped to the image (8..12) and still averaged.
        assert_eq!(img.get_pixel(11, 5).0, [100, 50, 25, 255]);
    }

    #[test]
    fn blur_smooths_and_keeps_flat_areas() {
        let mut img = stripes();
        blur(&mut img, 4);
        let p = img.get_pixel(6, 3).0;
        assert!((90..=110).contains(&p[0]) && (40..=60).contains(&p[1]), "{p:?}");
        let mut flat = RgbaImage::from_pixel(8, 8, image::Rgba([10, 20, 30, 255]));
        blur(&mut flat, 3);
        assert!(flat.pixels().all(|p| p.0 == [10, 20, 30, 255]));
    }
}
