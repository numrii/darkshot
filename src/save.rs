//! Saving a capture to a file: file names, formats and JPEG quality.

use std::path::{Path, PathBuf};

use image::RgbaImage;

use eframe::egui;

use crate::settings::Settings;
use crate::AnyResult;

/// Characters Windows does not allow in file names.
pub const INVALID_CHARS: &str = r#"<>:"/\|?*"#;

/// The name macros, with an example of what each becomes.
const MACROS: [(&str, &str); 9] = [
    ("{date}", "2026-11-13"),
    ("{time}", "16-24-03"),
    ("{year}", "2026"),
    ("{month}", "11"),
    ("{day}", "13"),
    ("{hour}", "16"),
    ("{minute}", "24"),
    ("{second}", "03"),
    ("{counter}", "0001, 0002, 0003 ..."),
];

/// The tooltip next to a name field: each macro in code style with an example.
fn name_help(ui: &mut egui::Ui) {
    egui::Grid::new("name-macros").num_columns(2).spacing([14.0, 4.0]).show(ui, |ui| {
        for (name, example) in MACROS {
            ui.label(egui::RichText::new(name).code());
            ui.label(example);
            ui.end_row();
        }
    });
    ui.add_space(6.0);
    ui.label(
        "The counter takes the lowest number no file in your save folder has yet: \
         shot-0003 if shot-0001 and shot-0002 are already there.",
    );
    ui.add_space(4.0);
    ui.label("File names can't contain < > : \" / \\ | ? *");
}

/// The moment a capture is named after, in local time.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Stamp {
    pub year: u16,
    pub month: u16,
    pub day: u16,
    pub hour: u16,
    pub minute: u16,
    pub second: u16,
}

impl Stamp {
    pub fn now() -> Self {
        let mut t: windows_sys::Win32::Foundation::SYSTEMTIME = unsafe { std::mem::zeroed() };
        unsafe { windows_sys::Win32::System::SystemInformation::GetLocalTime(&mut t) };
        Self { year: t.wYear, month: t.wMonth, day: t.wDay, hour: t.wHour, minute: t.wMinute, second: t.wSecond }
    }

    /// `YYYY-MM-DD`.
    pub fn date(&self) -> String {
        format!("{:04}-{:02}-{:02}", self.year, self.month, self.day)
    }

    /// `HH-MM-SS`.
    pub fn time(&self) -> String {
        format!("{:02}-{:02}-{:02}", self.hour, self.minute, self.second)
    }
}

/// Fill in the name macros (see [`MACROS`]).
pub fn expand(template: &str, stamp: &Stamp, counter: u32) -> String {
    let two = |n: u16| format!("{n:02}");
    template
        .replace("{date}", &stamp.date())
        .replace("{time}", &stamp.time())
        .replace("{year}", &format!("{:04}", stamp.year))
        .replace("{month}", &two(stamp.month))
        .replace("{day}", &two(stamp.day))
        .replace("{hour}", &two(stamp.hour))
        .replace("{minute}", &two(stamp.minute))
        .replace("{second}", &two(stamp.second))
        .replace("{counter}", &format!("{counter:04}"))
}

/// Make `name` (without extension) a valid Windows file name: forbidden and control
/// characters become `_`, surrounding spaces and trailing dots go, names Windows
/// reserves for devices (`CON`, `NUL`, `COM1`...) get a `_` in front, and very long
/// names are shortened. May return an empty string.
pub fn clean_file_name(name: &str) -> String {
    let name: String =
        name.chars().map(|c| if INVALID_CHARS.contains(c) || c.is_control() { '_' } else { c }).collect();
    let mut name: String = name.trim().trim_end_matches(['.', ' ']).chars().take(200).collect();
    name = name.trim_end_matches(['.', ' ']).to_owned();
    let base = name.split('.').next().unwrap_or("").trim_end().to_ascii_uppercase();
    let reserved = matches!(base.as_str(), "CON" | "PRN" | "AUX" | "NUL")
        || (base.len() == 4
            && (base.starts_with("COM") || base.starts_with("LPT"))
            && base.as_bytes()[3].is_ascii_digit());
    if reserved {
        name.insert(0, '_');
    }
    name
}

/// A file name field with a macro help mark, as two grid cells and one row. Characters
/// Windows does not allow are refused as they are typed, with a note under the field.
pub fn name_row(ui: &mut egui::Ui, label: &str, name: &mut String, width: f32) -> egui::Response {
    ui.label(label);
    let response = ui
        .horizontal(|ui| {
            let field = ui.add(egui::TextEdit::singleline(name).hint_text("date and time").desired_width(width));
            crate::icons::help_mark(ui).on_hover_ui(name_help);
            field
        })
        .inner;
    ui.end_row();
    let refused_id = response.id.with("refused");
    if response.changed() {
        let before = name.chars().count();
        name.retain(|c| !INVALID_CHARS.contains(c) && !c.is_control());
        let refused = name.chars().count() != before;
        ui.data_mut(|d| d.insert_temp(refused_id, refused));
    }
    if ui.data(|d| d.get_temp::<bool>(refused_id)).unwrap_or(false) {
        ui.label("");
        let text = "File names can't contain < > : \" / \\ | ? *";
        ui.label(egui::RichText::new(text).color(ui.visuals().warn_fg_color).small());
        ui.end_row();
    }
    response
}

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
    /// The file name without extension; may contain `{date}`, `{time}` and `{counter}`.
    pub name: String,
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
            name: settings.save.name.clone(),
            format: settings.save.format.into(),
            quality: settings.save.jpeg_quality,
            folder: settings.save_folder(),
        }
    }

    /// The suggested file name: the name with its macros filled in, made valid for
    /// Windows, plus the format's extension. If the name ends up empty, the date and
    /// time are used. `{counter}` is the lowest four-digit number whose file does not
    /// exist yet in the save folder.
    pub fn file_name(&self) -> String {
        self.file_name_at(&Stamp::now())
    }

    fn file_name_at(&self, stamp: &Stamp) -> String {
        let ext = self.format.extension();
        let build = |counter: u32| {
            let name = clean_file_name(&expand(&self.name, stamp, counter));
            if name.is_empty() {
                format!("{}_{}.{ext}", stamp.date(), stamp.time())
            } else {
                format!("{name}.{ext}")
            }
        };
        match self.folder.as_ref().filter(|_| self.name.contains("{counter}")) {
            Some(folder) => (1..=9999).map(build).find(|name| !folder.join(name).exists()).unwrap_or_else(|| build(10000)),
            None => build(1),
        }
    }

    /// Ask where to save, starting in the configured folder with the suggested name.
    /// Returns `None` if the user cancelled.
    pub fn ask_path(&self, parent: &eframe::Frame) -> Option<PathBuf> {
        // Test hook: save straight into this folder, without a dialog. Debug builds
        // only: in a release build Save always asks.
        #[cfg(debug_assertions)]
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
        let chosen = dialog.save_file()?;
        // The dialog only confirmed overwriting the name as typed. If the extension has
        // to be added, the file actually written is a different one: confirm that too.
        let path = final_path(&chosen, self.format);
        if path != chosen && path.exists() {
            let name = path.file_name().unwrap_or_default().to_string_lossy();
            let replace = rfd::MessageDialog::new()
                .set_parent(parent)
                .set_level(rfd::MessageLevel::Warning)
                .set_title("Darkshot")
                .set_description(format!("{name} already exists.\n\nDo you want to replace it?"))
                .set_buttons(rfd::MessageButtons::YesNo)
                .show();
            if replace != rfd::MessageDialogResult::Yes {
                return None;
            }
        }
        Some(path)
    }
}

/// `path` with the format's extension added if it does not already end in one that
/// matches, so a file never claims to be something it is not.
pub fn final_path(path: &Path, format: Format) -> PathBuf {
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
    path
}

/// Write `img` to `path` in `format`, at [`final_path`]. Returns the path actually
/// written.
pub fn write(img: &RgbaImage, path: &Path, format: Format, quality: u8) -> AnyResult<PathBuf> {
    let path = final_path(path, format);
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


#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn file_names() {
        let stamp_len = "2026-11-12_21-50-03".len();
        // Default: darkshot- and the date and time.
        let name = SaveOptions::default().file_name();
        assert!(name.starts_with("darkshot-20") && name.ends_with(".png"), "{name}");
        assert_eq!(name.len(), "darkshot-".len() + stamp_len + ".png".len());

        // Characters Windows forbids are replaced.
        let mut options = SaveOptions { name: " my:shot?-{date}".into(), ..Default::default() };
        assert!(options.file_name().starts_with("my_shot_-20"), "{}", options.file_name());

        // Fixed text only, a macro in the middle, and nothing at all.
        options.name = "bug-report".into();
        assert_eq!(options.file_name(), "bug-report.png");
        options.name = "{date} notes".into();
        let name = options.file_name();
        assert!(name.starts_with("20") && name.ends_with(" notes.png") && !name.contains('{'), "{name}");
        options.name.clear();
        options.format = Format::Jpeg;
        let name = options.file_name();
        assert!(name.starts_with("20") && name.len() == stamp_len + ".jpg".len(), "{name}");
    }

    #[test]
    fn macros() {
        let stamp = Stamp { year: 2026, month: 3, day: 7, hour: 9, minute: 5, second: 1 };
        assert_eq!(expand(crate::settings::DEFAULT_NAME, &stamp, 1), "darkshot-2026-03-07_09-05-01");
        assert_eq!(
            expand("{year}/{month}/{day} {hour}.{minute}.{second} #{counter}", &stamp, 42),
            "2026/03/07 09.05.01 #0042"
        );
        assert_eq!(expand("plain {unknown}", &stamp, 1), "plain {unknown}");
        // Slashes from the template still end up valid.
        let options = SaveOptions { name: "{year}/{month}".into(), folder: None, ..Default::default() };
        assert_eq!(options.file_name_at(&stamp), "2026_03.png");
    }

    #[test]
    fn windows_file_names() {
        assert_eq!(clean_file_name(r#"a<b>c:d"e/f\g|h?i*j"#), "a_b_c_d_e_f_g_h_i_j");
        assert_eq!(clean_file_name("  tabs\tand\nlines  "), "tabs_and_lines");
        assert_eq!(clean_file_name("ends with dots... "), "ends with dots");
        assert_eq!(clean_file_name("con"), "_con");
        assert_eq!(clean_file_name("NUL.backup"), "_NUL.backup");
        assert_eq!(clean_file_name("com7"), "_com7");
        assert_eq!(clean_file_name("console"), "console");
        assert_eq!(clean_file_name("COMA"), "COMA");
        assert_eq!(clean_file_name(&"x".repeat(300)).len(), 200);
        assert_eq!(clean_file_name("..."), "");
    }

    #[test]
    fn counter_takes_the_next_free_number() {
        let dir = std::env::temp_dir().join(format!("darkshot-counter-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let options = SaveOptions { name: "shot {counter}".into(), folder: Some(dir.clone()), ..Default::default() };
        assert_eq!(options.file_name(), "shot 0001.png");
        std::fs::write(dir.join("shot 0001.png"), b"").unwrap();
        std::fs::write(dir.join("shot 0002.png"), b"").unwrap();
        assert_eq!(options.file_name(), "shot 0003.png");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn final_paths() {
        let p = |s: &str, f| final_path(Path::new(s), f).to_string_lossy().into_owned();
        assert_eq!(p(r"C:\a\shot.png", Format::Png), r"C:\a\shot.png");
        assert_eq!(p(r"C:\a\shot.PNG", Format::Png), r"C:\a\shot.PNG");
        assert_eq!(p(r"C:\a\shot.jpeg", Format::Jpeg), r"C:\a\shot.jpeg");
        // A different extension is kept and the right one added: the file written is
        // not the one the dialog confirmed, which `ask_path` checks for.
        assert_eq!(p(r"C:\a\notes.txt", Format::Png), r"C:\a\notes.txt.png");
        assert_eq!(p(r"C:\a\shot", Format::Jpeg), r"C:\a\shot.jpg");
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
