//! Putting a copied image on the clipboard, in two forms at once:
//!
//! - **PNG**, with transparency, for programs that read it (browsers, Office, chat
//!   apps, most image editors). A drop shadow keeps its soft, see-through edge there.
//! - **A plain bitmap on white** (`CF_DIB`), for programs that only read bitmaps (Paint,
//!   older software). Those would show transparent areas as black.
//!
//! Each program pastes the best form it understands. Both are kept out of Windows'
//! clipboard history (Win+V) and cloud clipboard sync, so a copy lives only until the
//! next one replaces it. (The clipboard library used for text does that for text, but
//! not for images, so images are written here, with the Windows API directly.)

use image::RgbaImage;
use windows_sys::Win32::Foundation::{GlobalFree, HANDLE};
use windows_sys::Win32::Graphics::Gdi::{BI_RGB, BITMAPINFOHEADER};
use windows_sys::Win32::System::DataExchange::{
    CloseClipboard, EmptyClipboard, OpenClipboard, RegisterClipboardFormatW, SetClipboardData,
};
use windows_sys::Win32::System::Memory::{GMEM_MOVEABLE, GlobalAlloc, GlobalLock, GlobalUnlock};
use windows_sys::Win32::System::Ole::CF_DIB;

use crate::AnyResult;

/// Put `img` on the clipboard, as PNG with its transparency and as a bitmap on white.
pub fn set_image(img: &RgbaImage) -> AnyResult<()> {
    let png = png(img)?;
    let mut flat = img.clone();
    crate::effects::flatten(&mut flat);
    let dib = dib(&flat);

    let _open = Open::new()?;
    if unsafe { EmptyClipboard() } == 0 {
        return Err("Could not empty the clipboard".into());
    }
    // PNG first: some programs take the first form they recognise.
    put(format("PNG")?, &png)?;
    put(u32::from(CF_DIB), &dib)?;
    // A DWORD of zero in these formats keeps this copy out of clipboard history and
    // cloud sync.
    for name in ["CanIncludeInClipboardHistory", "CanUploadToCloudClipboard"] {
        put(format(name)?, &0u32.to_ne_bytes())?;
    }
    Ok(())
}

/// `img` as a PNG file's bytes.
fn png(img: &RgbaImage) -> AnyResult<Vec<u8>> {
    let mut bytes = Vec::new();
    img.write_to(&mut std::io::Cursor::new(&mut bytes), image::ImageFormat::Png)?;
    Ok(bytes)
}

/// `img` as a device-independent bitmap: a header, then 32-bit blue-green-red pixels,
/// bottom row first.
fn dib(img: &RgbaImage) -> Vec<u8> {
    let (w, h) = img.dimensions();
    let header = BITMAPINFOHEADER {
        biSize: size_of::<BITMAPINFOHEADER>() as u32,
        biWidth: w as i32,
        biHeight: h as i32,
        biPlanes: 1,
        biBitCount: 32,
        biCompression: BI_RGB,
        biSizeImage: w * h * 4,
        biXPelsPerMeter: 0,
        biYPelsPerMeter: 0,
        biClrUsed: 0,
        biClrImportant: 0,
    };
    let mut out = Vec::with_capacity(size_of::<BITMAPINFOHEADER>() + (w * h * 4) as usize);
    // SAFETY: a plain struct of integers, read as its bytes.
    out.extend_from_slice(unsafe {
        std::slice::from_raw_parts((&raw const header).cast::<u8>(), size_of::<BITMAPINFOHEADER>())
    });
    for y in (0..h).rev() {
        for x in 0..w {
            let [r, g, b, a] = img.get_pixel(x, y).0;
            out.extend_from_slice(&[b, g, r, a]);
        }
    }
    out
}

/// The number of the clipboard format called `name`.
fn format(name: &str) -> AnyResult<u32> {
    let wide: Vec<u16> = name.encode_utf16().chain(Some(0)).collect();
    match unsafe { RegisterClipboardFormatW(wide.as_ptr()) } {
        0 => Err(format!("Could not register the {name} clipboard format").into()),
        id => Ok(id),
    }
}

/// Put `bytes` on the open clipboard as `format`. The clipboard owns the memory after.
fn put(format: u32, bytes: &[u8]) -> AnyResult<()> {
    unsafe {
        let memory = GlobalAlloc(GMEM_MOVEABLE, bytes.len().max(1));
        if memory.is_null() {
            return Err("Out of memory for the clipboard".into());
        }
        let target = GlobalLock(memory);
        if target.is_null() {
            GlobalFree(memory);
            return Err("Could not lock clipboard memory".into());
        }
        std::ptr::copy_nonoverlapping(bytes.as_ptr(), target.cast::<u8>(), bytes.len());
        GlobalUnlock(memory);
        if SetClipboardData(format, memory as HANDLE).is_null() {
            GlobalFree(memory);
            return Err("Could not put the image on the clipboard".into());
        }
    }
    Ok(())
}

/// The clipboard, open until dropped. Another program may hold it for a moment, so
/// opening is tried a few times.
struct Open;

impl Open {
    fn new() -> AnyResult<Self> {
        for _ in 0..20 {
            if unsafe { OpenClipboard(std::ptr::null_mut()) } != 0 {
                return Ok(Self);
            }
            std::thread::sleep(std::time::Duration::from_millis(10));
        }
        Err("The clipboard is in use by another program".into())
    }
}

impl Drop for Open {
    fn drop(&mut self) {
        unsafe { CloseClipboard() };
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn bitmap_is_bottom_up_bgra_after_its_header() {
        let img = RgbaImage::from_fn(2, 2, |x, y| image::Rgba([x as u8 * 10, y as u8 * 10, 7, 255]));
        let bytes = dib(&img);
        let header = size_of::<BITMAPINFOHEADER>();
        assert_eq!(header, 40);
        assert_eq!(bytes.len(), header + 16);
        assert_eq!(i32::from_le_bytes(bytes[4..8].try_into().unwrap()), 2);
        assert_eq!(u16::from_le_bytes(bytes[14..16].try_into().unwrap()), 32);
        // First stored row is the bottom one (y = 1): blue, green, red, alpha.
        assert_eq!(&bytes[header..header + 8], &[7, 10, 0, 255, 7, 10, 10, 255]);
        assert_eq!(&bytes[header + 8..header + 12], &[7, 0, 0, 255]);
    }

    /// Writes to the real clipboard, so only run on request:
    /// `cargo test clipboard -- --ignored`.
    #[test]
    #[ignore]
    fn puts_png_bitmap_and_exclusions_on_the_clipboard() {
        use windows_sys::Win32::System::DataExchange::IsClipboardFormatAvailable;
        let img = RgbaImage::from_pixel(8, 8, image::Rgba([200, 0, 0, 100]));
        set_image(&img).unwrap();
        for id in [format("PNG").unwrap(), u32::from(CF_DIB), format("CanIncludeInClipboardHistory").unwrap()] {
            assert_ne!(unsafe { IsClipboardFormatAvailable(id) }, 0, "format {id}");
        }
    }
}
