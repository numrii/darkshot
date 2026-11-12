// Darkshot: a privacy-first screenshot tool. No network code, ever:
// a capture can only leave the app via the clipboard or a file on disk.
#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

use std::borrow::Cow;
use std::time::{SystemTime, UNIX_EPOCH};

use eframe::egui::{self, Align2, Color32, FontId, Pos2, Rect, Sense, Stroke, StrokeKind};
use image::RgbaImage;

type AnyResult<T> = Result<T, Box<dyn std::error::Error>>;

fn main() -> AnyResult<()> {
    // Grab the screen before our own window exists so the overlay is never in the shot.
    let screenshot = capture_primary_monitor()?;

    let options = eframe::NativeOptions {
        viewport: egui::ViewportBuilder::default()
            .with_decorations(false)
            .with_fullscreen(true)
            .with_taskbar(false),
        ..Default::default()
    };

    eframe::run_native(
        "Darkshot",
        options,
        Box::new(move |cc| Ok(Box::new(Overlay::new(cc, screenshot)))),
    )?;
    Ok(())
}

fn capture_primary_monitor() -> AnyResult<RgbaImage> {
    let monitor = xcap::Monitor::all()?
        .into_iter()
        .find(|m| m.is_primary().unwrap_or(false))
        .ok_or("no primary monitor found")?;
    Ok(monitor.capture_image()?)
}

struct Overlay {
    /// The frozen full-screen capture, in physical pixels.
    image: RgbaImage,
    /// Created on the first frame: the GPU's real texture size limit is not known before then.
    texture: Option<egui::TextureHandle>,
    drag_start: Option<Pos2>,
    /// Selected region in egui points (screen space).
    selection: Option<Rect>,
    error: Option<String>,
}

impl Overlay {
    fn new(_cc: &eframe::CreationContext<'_>, image: RgbaImage) -> Self {
        Self {
            image,
            texture: None,
            drag_start: None,
            selection: None,
            error: None,
        }
    }

    /// Crop the capture to the current selection, converting points to pixels.
    fn cropped(&self, screen: Rect) -> Option<RgbaImage> {
        let sel = self.selection?;
        let sx = self.image.width() as f32 / screen.width();
        let sy = self.image.height() as f32 / screen.height();
        let x = (((sel.min.x - screen.min.x) * sx).round() as u32).min(self.image.width());
        let y = (((sel.min.y - screen.min.y) * sy).round() as u32).min(self.image.height());
        let w = ((sel.width() * sx).round() as u32).min(self.image.width() - x);
        let h = ((sel.height() * sy).round() as u32).min(self.image.height() - y);
        if w == 0 || h == 0 {
            return None;
        }
        Some(image::imageops::crop_imm(&self.image, x, y, w, h).to_image())
    }

    fn copy_to_clipboard(img: &RgbaImage) -> AnyResult<()> {
        arboard::Clipboard::new()?.set_image(arboard::ImageData {
            width: img.width() as usize,
            height: img.height() as usize,
            bytes: Cow::Borrowed(img.as_raw()),
        })?;
        Ok(())
    }

    /// Returns Ok(false) if the user cancelled the dialog.
    fn save_to_file(img: &RgbaImage) -> AnyResult<bool> {
        let secs = SystemTime::now().duration_since(UNIX_EPOCH)?.as_secs();
        let Some(path) = rfd::FileDialog::new()
            .set_file_name(format!("darkshot-{secs}.png"))
            .add_filter("PNG image", &["png"])
            .save_file()
        else {
            return Ok(false);
        };
        img.save(path)?;
        Ok(true)
    }
}

impl eframe::App for Overlay {
    fn ui(&mut self, ui: &mut egui::Ui, _frame: &mut eframe::Frame) {
        let ctx = ui.ctx().clone();
        let screen = ctx.content_rect();
        let response = ui.allocate_rect(screen, Sense::drag());
        let painter = ui.painter_at(screen);
        ctx.set_cursor_icon(egui::CursorIcon::Crosshair);

        // Frozen screenshot as the backdrop.
        let uv = Rect::from_min_max(Pos2::ZERO, Pos2::new(1.0, 1.0));
        let texture = self.texture.get_or_insert_with(|| {
            let size = [self.image.width() as usize, self.image.height() as usize];
            ctx.load_texture(
                "screenshot",
                egui::ColorImage::from_rgba_unmultiplied(size, self.image.as_raw()),
                egui::TextureOptions::NEAREST,
            )
        });
        painter.image(texture.id(), screen, uv, Color32::WHITE);

        // Drag to (re)select a region.
        if response.drag_started() {
            self.drag_start = response.interact_pointer_pos();
            self.error = None;
        }
        if response.dragged() {
            if let (Some(a), Some(b)) = (self.drag_start, response.interact_pointer_pos()) {
                self.selection = Some(Rect::from_two_pos(a, b).intersect(screen));
            }
        }

        // Dim everything outside the selection.
        let dim = Color32::from_black_alpha(130);
        let font = FontId::proportional(14.0);
        match self.selection {
            Some(sel) => {
                let (l, r) = (sel.left(), sel.right());
                for rect in [
                    Rect::from_min_max(screen.min, Pos2::new(screen.right(), sel.top())),
                    Rect::from_min_max(Pos2::new(screen.left(), sel.bottom()), screen.max),
                    Rect::from_min_max(Pos2::new(screen.left(), sel.top()), Pos2::new(l, sel.bottom())),
                    Rect::from_min_max(Pos2::new(r, sel.top()), Pos2::new(screen.right(), sel.bottom())),
                ] {
                    painter.rect_filled(rect, 0.0, dim);
                }
                painter.rect_stroke(sel, 0.0, Stroke::new(1.0, Color32::WHITE), StrokeKind::Outside);

                let scale = self.image.width() as f32 / screen.width();
                let label = format!(
                    "{} x {}   Ctrl+C copy · Ctrl+S save · Esc cancel",
                    (sel.width() * scale).round(),
                    (sel.height() * scale).round()
                );
                // Put the label above the selection, or inside it when there is no room.
                let pos = if sel.top() - screen.top() > 22.0 {
                    sel.left_top() + egui::vec2(0.0, -20.0)
                } else {
                    sel.left_top() + egui::vec2(4.0, 4.0)
                };
                painter.text(pos, Align2::LEFT_TOP, label, font.clone(), Color32::WHITE);
            }
            None => {
                painter.rect_filled(screen, 0.0, dim);
                painter.text(
                    screen.center(),
                    Align2::CENTER_CENTER,
                    "Drag to select an area · Esc to cancel",
                    FontId::proportional(18.0),
                    Color32::WHITE,
                );
            }
        }

        if let Some(err) = &self.error {
            painter.text(
                screen.center_bottom() + egui::vec2(0.0, -40.0),
                Align2::CENTER_BOTTOM,
                err,
                font,
                Color32::LIGHT_RED,
            );
        }

        // Keyboard actions. egui turns Ctrl+C into Event::Copy rather than a key press.
        let (copy, save, cancel) = ctx.input(|i| {
            (
                i.events.iter().any(|e| matches!(e, egui::Event::Copy))
                    || i.key_pressed(egui::Key::Enter),
                i.modifiers.command && i.key_pressed(egui::Key::S),
                i.key_pressed(egui::Key::Escape),
            )
        });

        let close = || ctx.send_viewport_cmd(egui::ViewportCommand::Close);
        if cancel {
            close();
        } else if copy || save {
            if let Some(img) = self.cropped(screen) {
                let result = if copy {
                    Self::copy_to_clipboard(&img).map(|()| true)
                } else {
                    Self::save_to_file(&img)
                };
                match result {
                    Ok(true) => close(),
                    Ok(false) => {} // save dialog cancelled: stay in the overlay
                    Err(e) => self.error = Some(format!("Failed: {e}")),
                }
            }
        }
    }
}
