//! Twilight app icon, shared by the window and tray.
//! Install the supplied PNG at assets/duskshot-256.png and ICO at assets/duskshot.ico.

pub fn rgba(size: u32) -> Vec<u8> {
    let image = image::load_from_memory_with_format(
        include_bytes!("../assets/duskshot-256.png"),
        image::ImageFormat::Png,
    )
    .expect("embedded Duskshot icon must be a valid PNG")
    .to_rgba8();
    image::imageops::resize(&image, size, size, image::imageops::FilterType::Lanczos3).into_raw()
}

pub fn window_icon() -> std::sync::Arc<eframe::egui::IconData> {
    std::sync::Arc::new(eframe::egui::IconData {
        rgba: rgba(32),
        width: 32,
        height: 32,
    })
}

#[cfg(test)]
mod tests {
    #[test]
    fn ico_and_png_share_the_master_image() {
        let ico = image::load_from_memory_with_format(
            include_bytes!("../assets/duskshot.ico"),
            image::ImageFormat::Ico,
        )
        .unwrap()
        .to_rgba8();
        assert_eq!(ico.dimensions(), (256, 256));
        assert_eq!(ico.as_raw(), &super::rgba(256));
    }
}
