//! The app icon, drawn in code: four white selection corners on a dark tile. The same
//! design is in assets/darkshot.ico (the exe's icon), generated from this drawing.

/// The icon as RGBA pixels, `size` x `size`. The design lives on a 32x32 grid; other
/// sizes sample it (nearest neighbour), which keeps the lines crisp at multiples of 32.
pub fn rgba(size: u32) -> Vec<u8> {
    let near_edge = |v: u32| (7..14).contains(&v) || (18..25).contains(&v);
    let on_line = |v: u32| (7..10).contains(&v) || (22..25).contains(&v);
    let outer = |v: u32| v < 2 || v >= 30;

    let mut rgba = Vec::with_capacity((size * size * 4) as usize);
    for y in 0..size {
        for x in 0..size {
            let (gx, gy) = (x * 32 / size, y * 32 / size);
            let pixel = if outer(gx) && outer(gy) {
                [0, 0, 0, 0] // clip the tile's corners
            } else if (on_line(gx) && near_edge(gy)) || (on_line(gy) && near_edge(gx)) {
                [255, 255, 255, 255]
            } else {
                [30, 30, 34, 255]
            };
            rgba.extend_from_slice(&pixel);
        }
    }
    rgba
}

/// The icon for an eframe window's title bar and taskbar button.
pub fn window_icon() -> std::sync::Arc<eframe::egui::IconData> {
    std::sync::Arc::new(eframe::egui::IconData { rgba: rgba(32), width: 32, height: 32 })
}

#[cfg(test)]
mod tests {
    #[test]
    fn matches_the_ico_asset() {
        // The 32x32 image in the .ico was generated from this drawing.
        let ico = image::load_from_memory_with_format(include_bytes!("../assets/darkshot.ico"), image::ImageFormat::Ico)
            .unwrap()
            .to_rgba8();
        let at32 = image::imageops::resize(&ico, 32, 32, image::imageops::FilterType::Nearest);
        assert_eq!(at32.as_raw(), &super::rgba(32));
    }
}
