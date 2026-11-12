//! Saving a capture to a file: file names, formats and JPEG quality.

use std::path::{Path, PathBuf};

use image::RgbaImage;

use crate::settings::{expand, Settings};
use crate::AnyResult;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Format {
    /// Lossless.
    Png,
    /// Lossy, with a quality setting. Smaller files for photos and gradients.
    Jpeg,
}

impl Format {
    pub fn extension(self) -> &'static str {
        match self {
            Format::Png => "png",
            Format::Jpeg => "jpg",
        }
    }

    fn filter_name(self) -> &'static str {
        match self {
            Format::Png => "PNG image",
            Format::Jpeg => "JPEG image",
        }
    }
}

/// JPEG quality presets.
pub const QUALITY_HIGH: u8 = 90;
pub const QUALITY_MEDIUM: u8 = 75;

#[derive(Clone, Debug, PartialEq)]
pub struct SaveOptions {
    /// The file name is `prefix` + `suffix` + extension; both may contain `{date}` and
    /// `{time}`.
    pub prefix: String,
    pub suffix: String,
    pub format: Format,
    /// JPEG quality, 1-100.
    pub quality: u8,
    /// The folder the save dialog opens in.
    pub folder: Option<PathBuf>,
}

impl Default for SaveOptions {
    fn default() -> Self {
        Self::from_settings(&Settings::default())
    }
}

impl SaveOptions {
    pub fn from_settings(settings: &Settings) -> Self {
        Self {
            prefix: settings.save.prefix.clone(),
            suffix: settings.save.suffix.clone(),
            format: settings.save.format.into(),
            quality: settings.save.jpeg_quality,
            folder: settings.save_folder(),
        }
    }

    /// The suggested file name: prefix + suffix with `{date}` and `{time}` filled in,
    /// cleaned of characters Windows does not allow, plus the format's extension. If
    /// both are empty, the date and time are used.
    pub fn file_name(&self) -> String {
        let (date, time) = local_date_time();
        let name = expand(&format!("{}{}", self.prefix, self.suffix), &date, &time);
        let name: String = name
            .trim()
            .chars()
            .map(|c| if r#"<>:"/\|?*"#.contains(c) || c.is_control() { '_' } else { c })
            .collect();
        let name = name.trim_end_matches('.');
        let ext = self.format.extension();
        if name.is_empty() { format!("{date}_{time}.{ext}") } else { format!("{name}.{ext}") }
    }

    /// Ask where to save, starting in the configured folder with the suggested name.
    /// Returns `None` if the user cancelled.
    pub fn ask_path(&self, parent: &eframe::Frame) -> Option<PathBuf> {
        // Test hook: save straight into this folder, without a dialog.
        if let Some(dir) = std::env::var_os("DARKSHOT_SAVE_TO") {
            return Some(PathBuf::from(dir).join(self.file_name()));
        }
        let mut dialog = rfd::FileDialog::new()
            .set_parent(parent)
            .set_file_name(self.file_name())
            .add_filter(self.format.filter_name(), &[self.format.extension()]);
        if let Some(folder) = self.folder.as_ref().filter(|f| f.is_dir()) {
            dialog = dialog.set_directory(folder);
        }
        dialog.save_file()
    }
}

/// Write `img` to `path` in `format`. The extension is corrected if it does not
/// match the format, so a file never claims to be something it is not. Returns the
/// path actually written.
pub fn write(img: &RgbaImage, path: &Path, format: Format, quality: u8) -> AnyResult<PathBuf> {
    let mut path = path.to_path_buf();
    let ext_ok = path.extension().and_then(|e| e.to_str()).is_some_and(|e| match format {
        Format::Png => e.eq_ignore_ascii_case("png"),
        Format::Jpeg => e.eq_ignore_ascii_case("jpg") || e.eq_ignore_ascii_case("jpeg"),
    });
    if !ext_ok {
        let mut name = path.file_name().unwrap_or_default().to_os_string();
        name.push(".");
        name.push(format.extension());
        path.set_file_name(name);
    }
    let file = std::io::BufWriter::new(std::fs::File::create(&path)?);
    match format {
        Format::Png => img.write_with_encoder(image::codecs::png::PngEncoder::new(file))?,
        Format::Jpeg => {
            // JPEG has no transparency: areas outside every monitor become white.
            let rgb = image::RgbImage::from_fn(img.width(), img.height(), |x, y| {
                let [r, g, b, a] = img.get_pixel(x, y).0;
                let blend = |c: u8| ((u32::from(c) * u32::from(a) + 255 * (255 - u32::from(a))) / 255) as u8;
                image::Rgb([blend(r), blend(g), blend(b)])
            });
            let encoder = image::codecs::jpeg::JpegEncoder::new_with_quality(file, quality.clamp(1, 100));
            rgb.write_with_encoder(encoder)?;
        }
    }
    Ok(path)
}

/// The local date (`YYYY-MM-DD`) and time (`HH-MM-SS`).
pub fn local_date_time() -> (String, String) {
    let mut t: windows_sys::Win32::Foundation::SYSTEMTIME = unsafe { std::mem::zeroed() };
    unsafe { windows_sys::Win32::System::SystemInformation::GetLocalTime(&mut t) };
    (
        format!("{:04}-{:02}-{:02}", t.wYear, t.wMonth, t.wDay),
        format!("{:02}-{:02}-{:02}", t.wHour, t.wMinute, t.wSecond),
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn file_names() {
        let stamp_len = "2026-11-12_21-50-03".len();
        // Default: prefix + date and time.
        let name = SaveOptions::default().file_name();
        assert!(name.starts_with("darkshot-20") && name.ends_with(".png"), "{name}");
        assert_eq!(name.len(), "darkshot-".len() + stamp_len + ".png".len());

        // Characters Windows forbids are replaced.
        let mut options = SaveOptions { prefix: " my:shot?-".into(), ..Default::default() };
        assert!(options.file_name().starts_with("my_shot_-20"), "{}", options.file_name());

        // Fixed text only, placeholders in the prefix, and nothing at all.
        options.prefix = "bug-report".into();
        options.suffix = String::new();
        assert_eq!(options.file_name(), "bug-report.png");
        options.prefix = "{date} ".into();
        options.suffix = "notes".into();
        let name = options.file_name();
        assert!(name.starts_with("20") && name.ends_with(" notes.png") && !name.contains('{'), "{name}");
        options.prefix.clear();
        options.suffix.clear();
        options.format = Format::Jpeg;
        let name = options.file_name();
        assert!(name.starts_with("20") && name.len() == stamp_len + ".jpg".len(), "{name}");
    }

    #[test]
    fn writes_png_and_jpeg() {
        let dir = std::env::temp_dir().join(format!("darkshot-test-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        // Left half opaque red, right half transparent.
        let img = RgbaImage::from_fn(40, 20, |x, _| {
            if x < 20 { image::Rgba([255, 0, 0, 255]) } else { image::Rgba([0, 0, 0, 0]) }
        });

        let png = write(&img, &dir.join("a.png"), Format::Png, 90).unwrap();
        let back = image::open(&png).unwrap().to_rgba8();
        assert_eq!(back, img); // lossless, keeps transparency

        // Wrong extension is corrected; transparency becomes white.
        let jpg = write(&img, &dir.join("b.png"), Format::Jpeg, 90).unwrap();
        assert_eq!(jpg.file_name().unwrap(), "b.png.jpg");
        let back = image::open(&jpg).unwrap().to_rgb8();
        let [r, g, b] = back.get_pixel(5, 10).0;
        assert!(r > 230 && g < 30 && b < 30, "{r},{g},{b}");
        assert!(back.get_pixel(35, 10).0.iter().all(|&c| c > 240));

        // Lower quality means a smaller file.
        let noisy = RgbaImage::from_fn(200, 200, |x, y| image::Rgba([(x * 7 % 256) as u8, (y * 13 % 256) as u8, ((x ^ y) % 256) as u8, 255]));
        let high = write(&noisy, &dir.join("h.jpg"), Format::Jpeg, QUALITY_HIGH).unwrap();
        let medium = write(&noisy, &dir.join("m.jpg"), Format::Jpeg, QUALITY_MEDIUM).unwrap();
        let size = |p: &Path| std::fs::metadata(p).unwrap().len();
        assert!(size(&medium) < size(&high));

        std::fs::remove_dir_all(&dir).unwrap();
    }
}
