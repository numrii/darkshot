//! Drawing the mouse cursor into a capture (the "Include the mouse cursor" setting).
//!
//! Screen captures do not contain the cursor: Windows draws it separately. So the
//! cursor's picture and position are read when the capture starts, and Windows then
//! draws that picture onto the captured pixels, exactly as it does on screen (including
//! cursors that invert what is under them, such as the text cursor).

use image::RgbaImage;
use windows_sys::Win32::Graphics::Gdi::{
    CreateCompatibleDC, CreateDIBSection, DeleteDC, DeleteObject, GetObjectW, SelectObject, BITMAP,
    BITMAPINFO, BITMAPINFOHEADER, BI_RGB, DIB_RGB_COLORS,
};
use windows_sys::Win32::UI::WindowsAndMessaging::{
    DrawIconEx, GetCursorInfo, GetIconInfo, LoadCursorW, CURSORINFO, CURSOR_SHOWING, DI_NORMAL,
    HCURSOR, ICONINFO, IDC_APPSTARTING, IDC_ARROW,
};

/// The cursor as it was on screen at one moment.
pub struct Cursor {
    handle: HCURSOR,
    /// The top-left corner of its picture, in screen coordinates.
    left: i32,
    top: i32,
    width: u32,
    height: u32,
}

impl Cursor {
    /// The cursor on screen right now, or `None` if it is hidden.
    pub fn current() -> Option<Self> {
        unsafe {
            let mut info: CURSORINFO = std::mem::zeroed();
            info.cbSize = std::mem::size_of::<CURSORINFO>() as u32;
            if GetCursorInfo(&mut info) == 0 || info.flags & CURSOR_SHOWING == 0 || info.hCursor.is_null() {
                return None;
            }
            // Starting the capture process makes Windows show the arrow with a busy ring
            // for a moment. That is Darkshot's doing, not what was on screen: draw the
            // plain arrow instead.
            let mut handle = info.hCursor;
            if handle == LoadCursorW(std::ptr::null_mut(), IDC_APPSTARTING) {
                handle = LoadCursorW(std::ptr::null_mut(), IDC_ARROW);
            }
            let (width, height, hot_x, hot_y) = shape(handle)?;
            Some(Self {
                handle,
                left: info.ptScreenPos.x - hot_x,
                top: info.ptScreenPos.y - hot_y,
                width,
                height,
            })
        }
    }

    /// Draw the cursor onto the monitor captures it touches. Each capture comes with the
    /// screen position of its top-left corner.
    pub fn draw(&self, shots: &mut [(i32, i32, RgbaImage)]) {
        // The part of a capture under the cursor: (x, y) in the capture and in the patch.
        let overlap = |x0: i32, y0: i32, shot: &RgbaImage| {
            let (w, h) = (self.width as i32, self.height as i32);
            let xs = (self.left.max(x0), (self.left + w).min(x0 + shot.width() as i32));
            let ys = (self.top.max(y0), (self.top + h).min(y0 + shot.height() as i32));
            (ys.0..ys.1).flat_map(move |y| (xs.0..xs.1).map(move |x| (x, y)))
        };
        let mut patch = RgbaImage::new(self.width, self.height);
        for (x0, y0, shot) in shots.iter() {
            for (x, y) in overlap(*x0, *y0, shot) {
                let pixel = *shot.get_pixel((x - x0) as u32, (y - y0) as u32);
                patch.put_pixel((x - self.left) as u32, (y - self.top) as u32, pixel);
            }
        }
        if !paint(self.handle, &mut patch) {
            return;
        }
        for (x0, y0, shot) in shots.iter_mut() {
            for (x, y) in overlap(*x0, *y0, shot) {
                let pixel = *patch.get_pixel((x - self.left) as u32, (y - self.top) as u32);
                shot.put_pixel((x - *x0) as u32, (y - *y0) as u32, pixel);
            }
        }
    }
}

/// A cursor's picture size and hot spot (the point of the picture that sits at the
/// cursor's position): (width, height, hot x, hot y).
unsafe fn shape(handle: HCURSOR) -> Option<(u32, u32, i32, i32)> {
    unsafe {
        let mut icon: ICONINFO = std::mem::zeroed();
        if GetIconInfo(handle, &mut icon) == 0 {
            return None;
        }
        // A cursor without a colour picture keeps its two masks stacked in one bitmap
        // of twice the height.
        let (bitmap, halve) = if icon.hbmColor.is_null() { (icon.hbmMask, true) } else { (icon.hbmColor, false) };
        let mut bm: BITMAP = std::mem::zeroed();
        let got = GetObjectW(bitmap, std::mem::size_of::<BITMAP>() as i32, std::ptr::from_mut(&mut bm).cast());
        for owned in [icon.hbmMask, icon.hbmColor] {
            if !owned.is_null() {
                DeleteObject(owned);
            }
        }
        let height = if halve { bm.bmHeight / 2 } else { bm.bmHeight };
        if got == 0 || bm.bmWidth <= 0 || height <= 0 || bm.bmWidth > 1024 || height > 1024 {
            return None;
        }
        Some((bm.bmWidth as u32, height as u32, icon.xHotspot as i32, icon.yHotspot as i32))
    }
}

/// Have Windows draw the cursor over `patch`, at its top-left corner. The result is
/// opaque. Returns false, leaving `patch` alone, if drawing was not possible.
fn paint(handle: HCURSOR, patch: &mut RgbaImage) -> bool {
    let (w, h) = patch.dimensions();
    unsafe {
        let mut info: BITMAPINFO = std::mem::zeroed();
        info.bmiHeader = BITMAPINFOHEADER {
            biSize: std::mem::size_of::<BITMAPINFOHEADER>() as u32,
            biWidth: w as i32,
            biHeight: -(h as i32), // top-down
            biPlanes: 1,
            biBitCount: 32,
            biCompression: BI_RGB,
            ..std::mem::zeroed()
        };
        let dc = CreateCompatibleDC(std::ptr::null_mut());
        if dc.is_null() {
            return false;
        }
        let mut bits: *mut core::ffi::c_void = std::ptr::null_mut();
        let dib = CreateDIBSection(dc, &info, DIB_RGB_COLORS, &mut bits, std::ptr::null_mut(), 0);
        if dib.is_null() || bits.is_null() {
            DeleteDC(dc);
            return false;
        }
        // The bitmap's pixels are blue, green, red, unused.
        let pixels = std::slice::from_raw_parts_mut(bits.cast::<[u8; 4]>(), (w * h) as usize);
        for (dst, src) in pixels.iter_mut().zip(patch.pixels()) {
            let [r, g, b, _] = src.0;
            *dst = [b, g, r, 255];
        }
        let previous = SelectObject(dc, dib);
        let drawn = DrawIconEx(dc, 0, 0, handle, w as i32, h as i32, 0, std::ptr::null_mut(), DI_NORMAL) != 0;
        if drawn {
            for (dst, src) in patch.pixels_mut().zip(pixels.iter()) {
                let [b, g, r, _] = *src;
                dst.0 = [r, g, b, 255];
            }
        }
        SelectObject(dc, previous);
        DeleteObject(dib);
        DeleteDC(dc);
        drawn
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The standard arrow, placed with its picture's corner at (`left`, `top`).
    fn arrow(left: i32, top: i32) -> Cursor {
        unsafe {
            let handle = LoadCursorW(std::ptr::null_mut(), IDC_ARROW);
            let (width, height, _, _) = shape(handle).expect("the arrow cursor has a shape");
            Cursor { handle, left, top, width, height }
        }
    }

    #[test]
    fn the_arrow_is_drawn_where_the_cursor_is() {
        let white = image::Rgba([255, 255, 255, 255]);
        // Two monitors side by side; the cursor straddles the line between them.
        let mut shots = vec![
            (0, 0, RgbaImage::from_pixel(200, 200, white)),
            (200, 0, RgbaImage::from_pixel(200, 200, white)),
        ];
        let cursor = arrow(198, 50);
        assert!(cursor.width >= 16 && cursor.height >= 16);
        cursor.draw(&mut shots);

        let changed = |shot: &RgbaImage| {
            let marks = shot.enumerate_pixels().filter(|(_, _, p)| **p != white);
            marks.map(|(x, y, _)| (x, y)).collect::<Vec<_>>()
        };
        let (left, right) = (changed(&shots[0].2), changed(&shots[1].2));
        // The arrow's tip is at its picture's top-left, so both monitors get part of it.
        assert!(!left.is_empty() && !right.is_empty(), "{} {}", left.len(), right.len());
        // Nothing outside the cursor's picture is touched, and everything stays opaque.
        assert!(left.iter().all(|&(x, y)| x >= 198 && (50..50 + cursor.height).contains(&y)));
        assert!(right.iter().all(|&(x, y)| x + 2 < cursor.width && (50..50 + cursor.height).contains(&y)));
        assert!(shots.iter().all(|s| s.2.pixels().all(|p| p.0[3] == 255)));

        // A cursor off every monitor changes nothing.
        let mut blank = vec![(0, 0, RgbaImage::from_pixel(50, 50, white))];
        arrow(500, 500).draw(&mut blank);
        assert!(blank[0].2.pixels().all(|p| *p == white));
    }
}
