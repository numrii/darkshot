//! The capture overlay: freeze the monitor under the cursor, let the user pick and
//! adjust a region, then copy or save it.

use std::borrow::Cow;
use std::time::{SystemTime, UNIX_EPOCH};

use eframe::egui::{
    self, vec2, Align2, Color32, CursorIcon, FontId, Key, Modifiers, Pos2, Rect, Sense, Stroke,
    StrokeKind, Vec2,
};
use image::RgbaImage;
use raw_window_handle::{HasWindowHandle, RawWindowHandle};
use windows_sys::Win32::Foundation::{HWND, POINT, RECT};
use windows_sys::Win32::Graphics::Gdi::{
    GetMonitorInfoW, MonitorFromPoint, MONITORINFO, MONITOR_DEFAULTTONEAREST,
};
use windows_sys::Win32::UI::HiDpi::{
    SetProcessDpiAwarenessContext, DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE_V2,
};
use windows_sys::Win32::UI::WindowsAndMessaging::{
    GetCursorPos, GetWindowRect, SetForegroundWindow, SetWindowPos, HWND_TOPMOST,
};

use crate::AnyResult;

/// How close (in points) the pointer must be to an edge or handle to grab it.
const GRAB_TOLERANCE: f32 = 6.0;
const HANDLE_SIZE: f32 = 7.0;

/// Capture the monitor under the cursor and run the overlay on it until the user
/// copies, saves or cancels.
pub fn run() -> AnyResult<()> {
    // Must happen first: without it Windows reports scaled, not physical, coordinates.
    unsafe { SetProcessDpiAwarenessContext(DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE_V2) };

    let (cursor, monitor) = monitor_under_cursor()?;
    // Grab the screen before our own window exists so the overlay is never in the shot.
    let screenshot = xcap::Monitor::from_point(cursor.x, cursor.y)?.capture_image()?;

    let options = eframe::NativeOptions {
        viewport: egui::ViewportBuilder::default()
            .with_decorations(false)
            .with_taskbar(false)
            .with_active(true),
        ..Default::default()
    };

    eframe::run_native(
        "Darkshot",
        options,
        Box::new(move |cc| Ok(Box::new(Overlay::new(cc, screenshot, monitor)))),
    )?;
    Ok(())
}

/// The cursor position and the bounds of the monitor it is on, in physical pixels.
fn monitor_under_cursor() -> AnyResult<(POINT, RECT)> {
    unsafe {
        let mut cursor = POINT { x: 0, y: 0 };
        if GetCursorPos(&mut cursor) == 0 {
            return Err("could not read the cursor position".into());
        }
        let mut info: MONITORINFO = std::mem::zeroed();
        info.cbSize = std::mem::size_of::<MONITORINFO>() as u32;
        let handle = MonitorFromPoint(cursor, MONITOR_DEFAULTTONEAREST);
        if GetMonitorInfoW(handle, &mut info) == 0 {
            return Err("could not read the monitor bounds".into());
        }
        Ok((cursor, info.rcMonitor))
    }
}

fn hwnd_of(window: &impl HasWindowHandle) -> Option<HWND> {
    match window.window_handle().ok()?.as_raw() {
        RawWindowHandle::Win32(h) => Some(h.hwnd.get() as HWND),
        _ => None,
    }
}

/// An in-progress mouse drag on the selection. All coordinates are image pixels.
#[derive(Clone, Copy)]
enum Drag {
    Create { anchor: Pos2 },
    Move { start: Pos2, orig: Rect },
    /// `hx`/`hy` say which edges are held: -1 = left/top, 1 = right/bottom, 0 = neither.
    Resize { start: Pos2, orig: Rect, hx: i8, hy: i8 },
}

impl Drag {
    /// The selection this drag produces with the pointer at `p`.
    /// Shift constrains (square / aspect ratio / axis), Alt works from the centre.
    fn apply(self, p: Pos2, mods: Modifiers, bounds: Rect) -> Rect {
        let sign = |v: f32| if v < 0.0 { -1.0 } else { 1.0 };
        let rect = match self {
            Drag::Create { anchor } => {
                let mut d = p - anchor;
                if mods.shift {
                    let side = d.x.abs().max(d.y.abs());
                    d = vec2(side * sign(d.x), side * sign(d.y));
                }
                if mods.alt {
                    Rect::from_two_pos(anchor - d, anchor + d)
                } else {
                    Rect::from_two_pos(anchor, anchor + d)
                }
            }
            Drag::Move { start, orig } => {
                let mut d = p - start;
                if mods.shift {
                    if d.x.abs() > d.y.abs() {
                        d.y = 0.0;
                    } else {
                        d.x = 0.0;
                    }
                }
                return round(keep_inside(orig.translate(d), bounds));
            }
            Drag::Resize { start, orig, hx, hy } => {
                let d = p - start;
                let (mut min, mut max) = (orig.min, orig.max);
                if hx < 0 {
                    min.x += d.x;
                    if mods.alt {
                        max.x -= d.x;
                    }
                } else if hx > 0 {
                    max.x += d.x;
                    if mods.alt {
                        min.x -= d.x;
                    }
                }
                if hy < 0 {
                    min.y += d.y;
                    if mods.alt {
                        max.y -= d.y;
                    }
                } else if hy > 0 {
                    max.y += d.y;
                    if mods.alt {
                        min.y -= d.y;
                    }
                }

                if mods.shift && orig.height() > 0.0 && orig.width() > 0.0 {
                    let aspect = orig.width() / orig.height();
                    let (w, h) = (max.x - min.x, max.y - min.y);
                    let (aw, ah) = if hx != 0 && hy != 0 {
                        let aw = w.abs().max(h.abs() * aspect);
                        (aw, aw / aspect)
                    } else if hy == 0 {
                        (w.abs(), w.abs() / aspect)
                    } else {
                        (h.abs() * aspect, h.abs())
                    };
                    // Per axis: grow from the centre, or keep the edge opposite the handle fixed.
                    let fit = |lo: &mut f32, hi: &mut f32, handle: i8, signed: f32, size: f32| {
                        if handle == 0 || mods.alt {
                            let centre = (*lo + *hi) / 2.0;
                            *lo = centre - size / 2.0;
                            *hi = centre + size / 2.0;
                        } else if handle > 0 {
                            *hi = *lo + sign(signed) * size;
                        } else {
                            *lo = *hi - sign(signed) * size;
                        }
                    };
                    fit(&mut min.x, &mut max.x, hx, w, aw);
                    fit(&mut min.y, &mut max.y, hy, h, ah);
                }
                Rect::from_two_pos(min, max)
            }
        };
        round(rect.intersect(bounds))
    }
}

fn round(r: Rect) -> Rect {
    Rect::from_min_max(r.min.round(), r.max.round())
}

/// Translate `r` by the smallest amount that puts it back inside `bounds`.
fn keep_inside(r: Rect, bounds: Rect) -> Rect {
    let dx = (bounds.min.x - r.min.x).max(0.0) + (bounds.max.x - r.max.x).min(0.0);
    let dy = (bounds.min.y - r.min.y).max(0.0) + (bounds.max.y - r.max.y).min(0.0);
    r.translate(vec2(dx, dy))
}

/// Which edges of `sel` the point `p` is close enough to grab, if any.
fn edges_at(sel: Rect, p: Pos2, tol: f32) -> Option<(i8, i8)> {
    if !sel.expand(tol).contains(p) {
        return None;
    }
    let near = |v: f32, lo: f32, hi: f32| -> i8 {
        if (v - lo).abs() <= tol {
            -1
        } else if (v - hi).abs() <= tol {
            1
        } else {
            0
        }
    };
    let edges = (near(p.x, sel.min.x, sel.max.x), near(p.y, sel.min.y, sel.max.y));
    (edges != (0, 0)).then_some(edges)
}

fn resize_cursor(hx: i8, hy: i8) -> CursorIcon {
    match (hx, hy) {
        (0, _) => CursorIcon::ResizeVertical,
        (_, 0) => CursorIcon::ResizeHorizontal,
        _ if hx == hy => CursorIcon::ResizeNwSe,
        _ => CursorIcon::ResizeNeSw,
    }
}

struct Overlay {
    /// The frozen capture of one monitor, in physical pixels.
    image: RgbaImage,
    /// Created on the first frame: the GPU's real texture size limit is not known before then.
    texture: Option<egui::TextureHandle>,
    /// Where the overlay window must sit: the captured monitor's bounds.
    monitor: RECT,
    hwnd: Option<HWND>,
    /// Selected region in image pixels.
    selection: Option<Rect>,
    drag: Option<Drag>,
    error: Option<String>,
}

impl Overlay {
    fn new(cc: &eframe::CreationContext<'_>, image: RgbaImage, monitor: RECT) -> Self {
        let overlay = Self {
            image,
            texture: None,
            monitor,
            hwnd: hwnd_of(cc),
            selection: None,
            drag: None,
            error: None,
        };
        if overlay.cover_monitor() {
            if let Some(hwnd) = overlay.hwnd {
                unsafe { SetForegroundWindow(hwnd) };
            }
        }
        overlay
    }

    /// Make the window cover exactly the captured monitor. Returns true if it had to move.
    /// Called every frame because Windows may resize the window when it crosses onto a
    /// monitor with a different DPI scale.
    fn cover_monitor(&self) -> bool {
        let Some(hwnd) = self.hwnd else { return false };
        let m = self.monitor;
        unsafe {
            let mut now: RECT = std::mem::zeroed();
            if GetWindowRect(hwnd, &mut now) != 0
                && (now.left, now.top, now.right, now.bottom) == (m.left, m.top, m.right, m.bottom)
            {
                return false;
            }
            SetWindowPos(hwnd, HWND_TOPMOST, m.left, m.top, m.right - m.left, m.bottom - m.top, 0);
        }
        true
    }

    fn bounds(&self) -> Rect {
        Rect::from_min_size(
            Pos2::ZERO,
            vec2(self.image.width() as f32, self.image.height() as f32),
        )
    }

    fn begin_drag(&self, p: Pos2, tol: f32) -> Drag {
        match self.selection {
            Some(sel) => match edges_at(sel, p, tol) {
                Some((hx, hy)) => Drag::Resize { start: p, orig: sel, hx, hy },
                None if sel.contains(p) => Drag::Move { start: p, orig: sel },
                None => Drag::Create { anchor: p },
            },
            None => Drag::Create { anchor: p },
        }
    }

    /// Crop the capture to the current selection.
    fn cropped(&self) -> Option<RgbaImage> {
        let sel = self.selection?.intersect(self.bounds());
        let (w, h) = (sel.width() as u32, sel.height() as u32);
        if w == 0 || h == 0 {
            return None;
        }
        Some(image::imageops::crop_imm(&self.image, sel.min.x as u32, sel.min.y as u32, w, h).to_image())
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
    fn save_to_file(img: &RgbaImage, parent: &eframe::Frame) -> AnyResult<bool> {
        let secs = SystemTime::now().duration_since(UNIX_EPOCH)?.as_secs();
        // Parented to the overlay so the dialog opens above our always-on-top window.
        let Some(path) = rfd::FileDialog::new()
            .set_parent(parent)
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

/// Text on a dark pill so it stays readable over any screenshot.
fn label(painter: &egui::Painter, pos: Pos2, anchor: Align2, text: String, color: Color32) {
    let galley = painter.layout_no_wrap(text, FontId::proportional(13.0), color);
    let rect = anchor.anchor_size(pos, galley.size());
    painter.rect_filled(rect.expand2(vec2(6.0, 3.0)), 4.0, Color32::from_black_alpha(200));
    painter.galley(rect.min, galley, color);
}

impl eframe::App for Overlay {
    fn ui(&mut self, ui: &mut egui::Ui, frame: &mut eframe::Frame) {
        let ctx = ui.ctx().clone();
        if self.hwnd.is_none() {
            self.hwnd = hwnd_of(frame);
        }
        if self.cover_monitor() {
            ctx.request_repaint(); // check again next frame that the move stuck
        }

        let screen = ctx.content_rect();
        let response = ui.allocate_rect(screen, Sense::drag());
        let painter = ui.painter_at(screen);

        // Image pixels per egui point, and conversions between the two spaces.
        let scale = self.image.width() as f32 / screen.width();
        let bounds = self.bounds();
        let to_px = |p: Pos2| ((p - screen.min) * scale).to_pos2();
        let to_pt = |p: Pos2| screen.min + p.to_vec2() / scale;
        let tol = GRAB_TOLERANCE * scale;

        // Frozen screenshot as the backdrop.
        let texture = self.texture.get_or_insert_with(|| {
            let size = [self.image.width() as usize, self.image.height() as usize];
            ctx.load_texture(
                "screenshot",
                egui::ColorImage::from_rgba_unmultiplied(size, self.image.as_raw()),
                egui::TextureOptions::NEAREST,
            )
        });
        let uv = Rect::from_min_max(Pos2::ZERO, Pos2::new(1.0, 1.0));
        painter.image(texture.id(), screen, uv, Color32::WHITE);

        // --- Mouse: create, move or resize the selection ---------------------------
        let (hover, press_origin, mods) =
            ctx.input(|i| (i.pointer.latest_pos(), i.pointer.press_origin(), i.modifiers));

        if response.drag_started() {
            if let Some(origin) = press_origin.or(response.interact_pointer_pos()) {
                self.drag = Some(self.begin_drag(to_px(origin), tol));
                self.error = None;
            }
        }
        if response.dragged() {
            if let (Some(drag), Some(p)) = (self.drag, response.interact_pointer_pos()) {
                self.selection = Some(drag.apply(to_px(p), mods, bounds));
            }
        }
        if response.drag_stopped() {
            self.drag = None;
            if self.selection.is_some_and(|s| s.width() < 1.0 || s.height() < 1.0) {
                self.selection = None;
            }
        }

        // Cursor reflects what a drag would do from here.
        let preview = self
            .drag
            .or_else(|| hover.map(|p| self.begin_drag(to_px(p), tol)));
        ctx.set_cursor_icon(match preview {
            Some(Drag::Resize { hx, hy, .. }) => resize_cursor(hx, hy),
            Some(Drag::Move { .. }) => CursorIcon::Move,
            _ => CursorIcon::Crosshair,
        });

        // --- Keyboard ----------------------------------------------------------------
        let (nudge, select_all, copy, save, cancel) = ctx.input(|i| {
            let step = if i.modifiers.shift { 10.0 } else { 1.0 };
            let axis = |neg: Key, pos: Key| {
                (i.num_presses(pos) as f32 - i.num_presses(neg) as f32) * step
            };
            (
                Vec2::new(
                    axis(Key::ArrowLeft, Key::ArrowRight),
                    axis(Key::ArrowUp, Key::ArrowDown),
                ),
                i.modifiers.command && i.key_pressed(Key::A),
                // egui turns Ctrl+C into Event::Copy rather than a key press.
                i.events.iter().any(|e| matches!(e, egui::Event::Copy))
                    || i.key_pressed(Key::Enter),
                i.modifiers.command && i.key_pressed(Key::S),
                i.key_pressed(Key::Escape),
            )
        });

        if self.drag.is_none() {
            if select_all {
                self.selection = Some(bounds);
            } else if nudge != Vec2::ZERO {
                if let Some(sel) = self.selection {
                    self.selection = Some(keep_inside(sel.translate(nudge), bounds));
                }
            }
        }

        // --- Paint: dim outside the selection, outline, handles, labels --------------
        let dim = Color32::from_black_alpha(130);
        match self.selection {
            Some(sel_px) => {
                let sel = Rect::from_min_max(to_pt(sel_px.min), to_pt(sel_px.max));
                for rect in [
                    Rect::from_min_max(screen.min, Pos2::new(screen.right(), sel.top())),
                    Rect::from_min_max(Pos2::new(screen.left(), sel.bottom()), screen.max),
                    Rect::from_min_max(
                        Pos2::new(screen.left(), sel.top()),
                        Pos2::new(sel.left(), sel.bottom()),
                    ),
                    Rect::from_min_max(
                        Pos2::new(sel.right(), sel.top()),
                        Pos2::new(screen.right(), sel.bottom()),
                    ),
                ] {
                    painter.rect_filled(rect, 0.0, dim);
                }
                painter.rect_stroke(sel, 0.0, Stroke::new(1.0, Color32::WHITE), StrokeKind::Outside);

                // Eight handles: corners and edge midpoints.
                let xs = [sel.left(), sel.center().x, sel.right()];
                let ys = [sel.top(), sel.center().y, sel.bottom()];
                for (ix, &x) in xs.iter().enumerate() {
                    for (iy, &y) in ys.iter().enumerate() {
                        if (ix, iy) == (1, 1) {
                            continue;
                        }
                        let handle = Rect::from_center_size(Pos2::new(x, y), Vec2::splat(HANDLE_SIZE));
                        painter.rect_filled(handle, 0.0, Color32::WHITE);
                        painter.rect_stroke(
                            handle,
                            0.0,
                            Stroke::new(1.0, Color32::from_gray(40)),
                            StrokeKind::Inside,
                        );
                    }
                }

                // Size readout above the selection, or inside it when there is no room.
                let size = format!("{} x {}", sel_px.width() as u32, sel_px.height() as u32);
                let pos = if sel.top() - screen.top() > 30.0 {
                    sel.left_top() + vec2(6.0, -10.0)
                } else {
                    sel.left_top() + vec2(12.0, 28.0)
                };
                label(&painter, pos, Align2::LEFT_BOTTOM, size, Color32::WHITE);
            }
            None => {
                painter.rect_filled(screen, 0.0, dim);
            }
        }

        let hint = if self.selection.is_some() {
            "Ctrl+C copy · Ctrl+S save · arrows nudge · Shift constrain · Alt from centre · Esc cancel"
        } else {
            "Drag to select an area · Ctrl+A whole screen · Esc cancel"
        };
        let bottom = screen.center_bottom() + vec2(0.0, -24.0);
        label(&painter, bottom, Align2::CENTER_BOTTOM, hint.to_owned(), Color32::WHITE);
        if let Some(err) = &self.error {
            let pos = bottom + vec2(0.0, -30.0);
            label(&painter, pos, Align2::CENTER_BOTTOM, err.clone(), Color32::LIGHT_RED);
        }

        // --- Actions -------------------------------------------------------------------
        let close = || ctx.send_viewport_cmd(egui::ViewportCommand::Close);
        if cancel {
            close();
        } else if copy || save {
            if let Some(img) = self.cropped() {
                let result = if copy {
                    Self::copy_to_clipboard(&img).map(|()| true)
                } else {
                    Self::save_to_file(&img, frame)
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

#[cfg(test)]
mod tests {
    use super::*;

    const BOUNDS: Rect = Rect::from_min_max(Pos2::ZERO, Pos2::new(1000.0, 1000.0));
    const SHIFT: Modifiers = Modifiers { shift: true, ..Modifiers::NONE };
    const ALT: Modifiers = Modifiers { alt: true, ..Modifiers::NONE };

    fn rect(x0: f32, y0: f32, x1: f32, y1: f32) -> Rect {
        Rect::from_min_max(Pos2::new(x0, y0), Pos2::new(x1, y1))
    }

    #[test]
    fn create_plain_square_and_centred() {
        let drag = Drag::Create { anchor: Pos2::new(500.0, 500.0) };
        let p = Pos2::new(400.0, 550.0); // up-left is negative x, positive y
        assert_eq!(drag.apply(p, Modifiers::NONE, BOUNDS), rect(400.0, 500.0, 500.0, 550.0));
        assert_eq!(drag.apply(p, SHIFT, BOUNDS), rect(400.0, 500.0, 500.0, 600.0));
        assert_eq!(drag.apply(p, ALT, BOUNDS), rect(400.0, 450.0, 600.0, 550.0));
    }

    #[test]
    fn move_is_clamped_and_shift_locks_axis() {
        let orig = rect(100.0, 100.0, 300.0, 200.0);
        let drag = Drag::Move { start: Pos2::new(150.0, 150.0), orig };
        assert_eq!(drag.apply(Pos2::new(0.0, 160.0), Modifiers::NONE, BOUNDS), rect(0.0, 110.0, 200.0, 210.0));
        assert_eq!(drag.apply(Pos2::new(200.0, 160.0), SHIFT, BOUNDS), rect(150.0, 100.0, 350.0, 200.0));
    }

    #[test]
    fn resize_corner_edge_aspect_and_centre() {
        let orig = rect(100.0, 100.0, 300.0, 200.0); // 2:1
        let start = Pos2::new(300.0, 200.0);
        let corner = Drag::Resize { start, orig, hx: 1, hy: 1 };
        let p = Pos2::new(400.0, 210.0);
        assert_eq!(corner.apply(p, Modifiers::NONE, BOUNDS), rect(100.0, 100.0, 400.0, 210.0));
        assert_eq!(corner.apply(p, SHIFT, BOUNDS), rect(100.0, 100.0, 400.0, 250.0));
        assert_eq!(corner.apply(p, ALT, BOUNDS), rect(0.0, 90.0, 400.0, 210.0));

        let top_left = Drag::Resize { start: orig.min, orig, hx: -1, hy: -1 };
        assert_eq!(top_left.apply(Pos2::new(0.0, 90.0), SHIFT, BOUNDS), rect(0.0, 50.0, 300.0, 200.0));

        let right_edge = Drag::Resize { start, orig, hx: 1, hy: 0 };
        assert_eq!(right_edge.apply(p, SHIFT, BOUNDS), rect(100.0, 75.0, 400.0, 225.0));
    }

    #[test]
    fn edges_and_nudge_clamp() {
        let sel = rect(100.0, 100.0, 300.0, 200.0);
        assert_eq!(edges_at(sel, Pos2::new(101.0, 99.0), 6.0), Some((-1, -1)));
        assert_eq!(edges_at(sel, Pos2::new(200.0, 203.0), 6.0), Some((0, 1)));
        assert_eq!(edges_at(sel, Pos2::new(200.0, 150.0), 6.0), None);
        assert_eq!(keep_inside(rect(900.0, -5.0, 1010.0, 50.0), BOUNDS), rect(890.0, 0.0, 1000.0, 55.0));
    }
}
