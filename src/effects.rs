//! Pixel effects for hiding parts of a capture: pixelate and blur.

use image::RgbaImage;

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

#[cfg(test)]
mod tests {
    use super::*;

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
