//! The capture overlay: freeze every monitor, show the frozen desktop in one window
//! spanning all of them, let the user pick and adjust a region, then copy or save it.

use std::borrow::Cow;
use std::cell::Cell;
use std::io::Write;
use std::sync::{Mutex, OnceLock};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use eframe::egui::{
    self, vec2, Align2, Color32, CursorIcon, FontId, Key, Modifiers, Pos2, Rect, Sense, Stroke,
    StrokeKind, Vec2,
};
use image::RgbaImage;
use raw_window_handle::{HasWindowHandle, RawWindowHandle};
use windows_sys::Win32::Foundation::{HWND, LPARAM, LRESULT, POINT, RECT, WPARAM};
use windows_sys::Win32::Graphics::Gdi::{RedrawWindow, RDW_INTERNALPAINT, RDW_UPDATENOW};
use windows_sys::Win32::Graphics::Dwm::{DwmSetWindowAttribute, DWMWA_TRANSITIONS_FORCEDISABLED};
use windows_sys::Win32::UI::HiDpi::{
    GetDpiForSystem, SetProcessDpiAwarenessContext, DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE_V2,
};
use windows_sys::Win32::UI::Shell::{DefSubclassProc, RemoveWindowSubclass, SetWindowSubclass};
use windows_sys::Win32::UI::WindowsAndMessaging::{
    GetCursorPos, GetWindowRect, KillTimer, SetForegroundWindow, SetTimer, SetWindowPos,
    HWND_TOPMOST, WM_MOUSEFIRST, WM_MOUSELAST, WM_MOUSEMOVE, WM_NCDESTROY, WM_PAINT, WM_TIMER,
};

use winit::event_loop::{DeviceEvents, EventLoop};

use crate::AnyResult;

/// How close (in points) the pointer must be to an edge or handle to grab it.
const GRAB_TOLERANCE: f32 = 6.0;
const HANDLE_SIZE: f32 = 7.0;

/// Where trace lines go: `--trace` writes `darkshot-trace.log` in the temp folder,
/// the `DARKSHOT_TRACE` environment variable writes to stderr. `None` = tracing off.
fn trace_sink() -> Option<&'static Mutex<Box<dyn Write + Send>>> {
    static SINK: OnceLock<Option<Mutex<Box<dyn Write + Send>>>> = OnceLock::new();
    SINK.get_or_init(|| {
        if std::env::args().any(|a| a == "--trace") {
            let path = std::env::temp_dir().join("darkshot-trace.log");
            let file = std::fs::File::create(path).ok()?;
            Some(Mutex::new(Box::new(std::io::LineWriter::new(file)) as Box<dyn Write + Send>))
        } else if std::env::var_os("DARKSHOT_TRACE").is_some() {
            Some(Mutex::new(Box::new(std::io::stderr()) as Box<dyn Write + Send>))
        } else {
            None
        }
    })
    .as_ref()
}

/// Record a timing line (startup phases, then one per frame) when tracing is on.
fn trace(what: &str) {
    static START: OnceLock<Instant> = OnceLock::new();
    let elapsed = START.get_or_init(Instant::now).elapsed();
    if let Some(sink) = trace_sink() {
        let ms = elapsed.as_secs_f64() * 1000.0;
        let _ = writeln!(sink.lock().unwrap(), "TRACE {ms:>9.1}ms {what}");
    }
}

/// Capture all monitors and run the overlay until the user copies, saves or cancels.
pub fn run() -> AnyResult<()> {
    // Must happen first: without it Windows reports scaled, not physical, coordinates.
    unsafe { SetProcessDpiAwarenessContext(DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE_V2) };

    // Grab the screen before our own window exists so the overlay is never in the shot.
    trace("start");
    let desktop = Desktop::capture()?;
    trace("captured");

    // Create the window at its final size. If it were created small and resized
    // afterwards, the first frame would be drawn into the small surface and stretched.
    // egui takes points; `cover_desktop` corrects any rounding or mixed-DPI mismatch.
    let b = desktop.bounds;
    let scale = unsafe { GetDpiForSystem() } as f32 / 96.0;
    let options = eframe::NativeOptions {
        viewport: egui::ViewportBuilder::default()
            .with_decorations(false)
            .with_taskbar(false)
            .with_active(true)
            .with_clamp_size_to_monitor_size(false)
            .with_position([b.left as f32 / scale, b.top as f32 / scale])
            .with_inner_size([
                (b.right - b.left) as f32 / scale,
                (b.bottom - b.top) as f32 / scale,
            ]),
        // Present immediately with a one-frame queue, for the lowest latency between
        // the cursor and what is drawn under it.
        wgpu_options: eframe::egui_wgpu::WgpuConfiguration::default().with_surface_config(
            eframe::egui_wgpu::SurfaceConfig {
                present_mode: eframe::wgpu::PresentMode::AutoNoVsync,
                desired_maximum_frame_latency: Some(1),
            },
        ),
        ..Default::default()
    };

    // We own the event loop (rather than calling `eframe::run_native`) so raw device
    // events can be switched off. With them on, a high-polling-rate mouse floods the
    // message queue with thousands of raw input messages per second; Windows only
    // delivers paint messages when the queue is empty, so repaints starve and anything
    // being dragged lags far behind the cursor.
    let event_loop = EventLoop::<eframe::UserEvent>::with_user_event().build()?;
    event_loop.listen_device_events(DeviceEvents::Never);
    let mut app = eframe::create_native(
        "Darkshot",
        options,
        Box::new(move |cc| Ok(Box::new(Overlay::new(cc, desktop)))),
        &event_loop,
    );
    event_loop.run_app(&mut app)?;
    Ok(())
}

/// A frozen capture of the whole virtual desktop.
struct Desktop {
    /// One capture per monitor, in physical pixels. Kept separate (never composited
    /// up front) so opening the overlay stays fast on large multi-monitor desktops.
    shots: Vec<RgbaImage>,
    /// The virtual desktop's bounds in screen coordinates.
    bounds: RECT,
    /// Each monitor's area in desktop pixels (origin = the desktop's top-left corner).
    /// Selections use the same coordinate space.
    monitors: Vec<Rect>,
    /// Index of the monitor the cursor was on at capture time.
    home: usize,
}

impl Desktop {
    fn capture() -> AnyResult<Self> {
        let mut cursor = POINT { x: 0, y: 0 };
        unsafe { GetCursorPos(&mut cursor) };

        let mut shots = Vec::new();
        for monitor in xcap::Monitor::all()? {
            shots.push((monitor.x()?, monitor.y()?, monitor.capture_image()?));
            trace("monitor captured");
        }
        let edge = |f: fn(&(i32, i32, RgbaImage)) -> i32| shots.iter().map(f);
        let (Some(left), Some(top), Some(right), Some(bottom)) = (
            edge(|s| s.0).min(),
            edge(|s| s.1).min(),
            edge(|s| s.0 + s.2.width() as i32).max(),
            edge(|s| s.1 + s.2.height() as i32).max(),
        ) else {
            return Err("no monitors found".into());
        };

        let monitors: Vec<Rect> = shots
            .iter()
            .map(|(x, y, shot)| {
                Rect::from_min_size(
                    Pos2::new((x - left) as f32, (y - top) as f32),
                    vec2(shot.width() as f32, shot.height() as f32),
                )
            })
            .collect();
        let cursor = Pos2::new((cursor.x - left) as f32, (cursor.y - top) as f32);
        let home = monitors.iter().position(|m| m.contains(cursor)).unwrap_or(0);

        Ok(Self {
            shots: shots.into_iter().map(|s| s.2).collect(),
            bounds: RECT { left, top, right, bottom },
            monitors,
            home,
        })
    }
}

fn hwnd_of(window: &impl HasWindowHandle) -> Option<HWND> {
    match window.window_handle().ok()?.as_raw() {
        RawWindowHandle::Win32(h) => Some(h.hwnd.get() as HWND),
        _ => None,
    }
}

/// Minimum time between mouse-move messages passed on to the app (500 per second).
const MOUSE_MOVE_INTERVAL: Duration = Duration::from_millis(2);
/// If the mouse is moving and no frame has been painted for this long, paint one now.
const MAX_PAINT_DELAY: Duration = Duration::from_millis(5);
const MOUSE_THROTTLE_ID: usize = 0xDA5C_5407;

/// Message statistics gathered between two frames, for `--trace`.
#[derive(Clone, Copy, Default)]
struct MsgStats {
    moves: u32,
    moves_forwarded: u32,
    other: u32,
    forced_paints: u32,
    /// The slowest single message: (message id, microseconds).
    slowest: (u32, u32),
    /// Duration of the last WM_PAINT (a whole frame including present), microseconds.
    paint_us: u32,
}

thread_local! {
    static LAST_FORWARDED: Cell<Option<Instant>> = const { Cell::new(None) };
    static LAST_PAINT: Cell<Option<Instant>> = const { Cell::new(None) };
    static PENDING: Cell<Option<(WPARAM, LPARAM)>> = const { Cell::new(None) };
    static STATS: Cell<MsgStats> = const { Cell::new(MsgStats {
        moves: 0, moves_forwarded: 0, other: 0, forced_paints: 0, slowest: (0, 0), paint_us: 0,
    }) };
}

fn bump(f: impl FnOnce(&mut MsgStats)) {
    let mut stats = STATS.get();
    f(&mut stats);
    STATS.set(stats);
}

/// Window subclass that keeps the overlay repainting under a flood of mouse input.
///
/// Windows only delivers paint messages when the message queue is otherwise empty. A
/// mouse polling at up to 8000 Hz keeps a mouse-move pending almost all the time, so
/// during fast movement repaints starve for hundreds of milliseconds and a dragged
/// selection moves in jerks. Two measures, both applied before winit and egui see
/// the message:
///
/// 1. Rate-limit `WM_MOUSEMOVE`, so most of the flood costs almost nothing. The
///    latest dropped position is never lost: it is delivered before any button or
///    wheel message, or by a short timer once the mouse stops.
/// 2. Do not wait for an idle queue: when a move is passed on and the last frame is
///    older than `MAX_PAINT_DELAY`, paint synchronously right away.
unsafe extern "system" fn pump_guard(
    hwnd: HWND,
    msg: u32,
    wparam: WPARAM,
    lparam: LPARAM,
    _id: usize,
    _data: usize,
) -> LRESULT {
    unsafe {
        match msg {
            WM_MOUSEMOVE => {
                bump(|s| s.moves += 1);
                let now = Instant::now();
                let too_soon = LAST_FORWARDED
                    .get()
                    .is_some_and(|last| now.duration_since(last) < MOUSE_MOVE_INTERVAL);
                if too_soon {
                    if PENDING.replace(Some((wparam, lparam))).is_none() {
                        SetTimer(hwnd, MOUSE_THROTTLE_ID, 10, None);
                    }
                    return 0;
                }
                LAST_FORWARDED.set(Some(now));
                PENDING.set(None);
                bump(|s| s.moves_forwarded += 1);
                let result = DefSubclassProc(hwnd, msg, wparam, lparam);

                let overdue = LAST_PAINT
                    .get()
                    .is_none_or(|last| now.duration_since(last) >= MAX_PAINT_DELAY);
                if overdue {
                    bump(|s| s.forced_paints += 1);
                    // Sends WM_PAINT synchronously; it re-enters this function below.
                    RedrawWindow(hwnd, std::ptr::null(), std::ptr::null_mut(), RDW_INTERNALPAINT | RDW_UPDATENOW);
                }
                return result;
            }
            WM_PAINT => {
                let start = Instant::now();
                LAST_PAINT.set(Some(start));
                let result = DefSubclassProc(hwnd, msg, wparam, lparam);
                let us = start.elapsed().as_micros() as u32;
                bump(|s| s.paint_us = us);
                return result;
            }
            WM_TIMER if wparam == MOUSE_THROTTLE_ID => {
                KillTimer(hwnd, MOUSE_THROTTLE_ID);
                if let Some((w, l)) = PENDING.take() {
                    LAST_FORWARDED.set(Some(Instant::now()));
                    return DefSubclassProc(hwnd, WM_MOUSEMOVE, w, l);
                }
                return 0;
            }
            // Clicks and wheel turns must act at the true cursor position, so deliver
            // any held-back move first. Their lparam carries the same client coordinates.
            WM_MOUSEFIRST..=WM_MOUSELAST => {
                if let Some((w, _)) = PENDING.take() {
                    DefSubclassProc(hwnd, WM_MOUSEMOVE, w, lparam);
                }
            }
            WM_NCDESTROY => {
                RemoveWindowSubclass(hwnd, Some(pump_guard), MOUSE_THROTTLE_ID);
            }
            _ => {}
        }
        let start = Instant::now();
        let result = DefSubclassProc(hwnd, msg, wparam, lparam);
        let us = start.elapsed().as_micros() as u32;
        bump(|s| {
            s.other += 1;
            if us > s.slowest.1 {
                s.slowest = (msg, us);
            }
        });
        result
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
    desktop: Desktop,
    /// One texture per monitor, so no texture exceeds the GPU's size limit. Created on
    /// the first frame: the real limit is not known before then.
    textures: Vec<egui::TextureHandle>,
    hwnd: Option<HWND>,
    /// The monitor the pointer was last on; Ctrl+A and the hint bar use it.
    current_monitor: usize,
    /// Selected region in image pixels.
    selection: Option<Rect>,
    drag: Option<Drag>,
    error: Option<String>,
}

impl Overlay {
    fn new(cc: &eframe::CreationContext<'_>, desktop: Desktop) -> Self {
        trace("window created");
        let overlay = Self {
            current_monitor: desktop.home,
            desktop,
            textures: Vec::new(),
            hwnd: hwnd_of(cc),
            selection: None,
            drag: None,
            error: None,
        };
        if let Some(hwnd) = overlay.hwnd {
            // Switch off the window's fade-in. During that animation the live desktop
            // shows through the frozen capture, which looks like ghosting when
            // something on screen is moving. The window is still hidden at this point.
            let disable: i32 = 1;
            unsafe {
                SetWindowSubclass(hwnd, Some(pump_guard), MOUSE_THROTTLE_ID, 0);
                DwmSetWindowAttribute(
                    hwnd,
                    DWMWA_TRANSITIONS_FORCEDISABLED as u32,
                    std::ptr::from_ref(&disable).cast(),
                    std::mem::size_of::<i32>() as u32,
                );
            }
        }
        overlay.cover_desktop(true);
        if let Some(hwnd) = overlay.hwnd {
            unsafe { SetForegroundWindow(hwnd) };
        }
        overlay
    }

    /// Make the window cover exactly the captured desktop, above everything else.
    /// Returns true if it had to move. Checked every frame (`force` = false) because
    /// Windows may resize the window when it lands on a monitor with a different DPI scale.
    fn cover_desktop(&self, force: bool) -> bool {
        let Some(hwnd) = self.hwnd else { return false };
        let m = self.desktop.bounds;
        unsafe {
            let mut now: RECT = std::mem::zeroed();
            let in_place = GetWindowRect(hwnd, &mut now) != 0
                && (now.left, now.top, now.right, now.bottom) == (m.left, m.top, m.right, m.bottom);
            if force || !in_place {
                SetWindowPos(hwnd, HWND_TOPMOST, m.left, m.top, m.right - m.left, m.bottom - m.top, 0);
            }
            !in_place
        }
    }

    fn bounds(&self) -> Rect {
        let b = self.desktop.bounds;
        Rect::from_min_size(
            Pos2::ZERO,
            vec2((b.right - b.left) as f32, (b.bottom - b.top) as f32),
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

    /// The pixels inside the current selection, stitched from every monitor it touches.
    /// Areas outside all monitors (possible when monitors differ in size) stay transparent.
    fn cropped(&self) -> Option<RgbaImage> {
        let sel = self.selection?.intersect(self.bounds());
        let (w, h) = (sel.width() as u32, sel.height() as u32);
        if w == 0 || h == 0 {
            return None;
        }
        let mut out = RgbaImage::new(w, h);
        for (monitor, shot) in self.desktop.monitors.iter().zip(&self.desktop.shots) {
            let part = sel.intersect(*monitor);
            if part.width() < 1.0 || part.height() < 1.0 {
                continue;
            }
            let piece = image::imageops::crop_imm(
                shot,
                (part.min.x - monitor.min.x) as u32,
                (part.min.y - monitor.min.y) as u32,
                part.width() as u32,
                part.height() as u32,
            );
            image::imageops::replace(
                &mut out,
                &*piece,
                (part.min.x - sel.min.x) as i64,
                (part.min.y - sel.min.y) as i64,
            );
        }
        Some(out)
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
        if self.cover_desktop(false) {
            ctx.request_repaint(); // check again next frame that the move stuck
        }

        let screen = ctx.content_rect();
        let response = ui.allocate_rect(screen, Sense::drag());
        let painter = ui.painter_at(screen);

        // Image pixels per egui point, and conversions between the two spaces.
        let scale = self.bounds().width() / screen.width();
        let bounds = self.bounds();
        let to_px = |p: Pos2| ((p - screen.min) * scale).to_pos2();
        let to_pt = |p: Pos2| screen.min + p.to_vec2() / scale;
        let tol = GRAB_TOLERANCE * scale;

        // Frozen screenshot as the backdrop, one textured quad per monitor.
        if self.textures.is_empty() {
            trace("first frame");
            for (i, shot) in self.desktop.shots.iter().enumerate() {
                let size = [shot.width() as usize, shot.height() as usize];
                self.textures.push(ctx.load_texture(
                    format!("monitor-{i}"),
                    egui::ColorImage::from_rgba_unmultiplied(size, shot.as_raw()),
                    egui::TextureOptions::NEAREST,
                ));
            }
        }
        let uv = Rect::from_min_max(Pos2::ZERO, Pos2::new(1.0, 1.0));
        for (m, texture) in self.desktop.monitors.iter().zip(&self.textures) {
            let area = Rect::from_min_max(to_pt(m.min), to_pt(m.max));
            painter.image(texture.id(), area, uv, Color32::WHITE);
        }

        // --- Mouse: create, move or resize the selection ---------------------------
        let (hover, press_origin, mods) =
            ctx.input(|i| (i.pointer.latest_pos(), i.pointer.press_origin(), i.modifiers));
        if let Some(i) = hover.and_then(|p| {
            let p = to_px(p);
            self.desktop.monitors.iter().position(|m| m.contains(p))
        }) {
            self.current_monitor = i;
        }
        if trace_sink().is_some() {
            // Per frame: where egui thinks the pointer is vs. where Windows says it is
            // right now (both in desktop pixels), and where the selection was drawn.
            let mut real = POINT { x: 0, y: 0 };
            unsafe { GetCursorPos(&mut real) };
            let b = self.desktop.bounds;
            let seen = hover.map(to_px).unwrap_or(Pos2::new(-1.0, -1.0));
            let sel = self.selection.map_or(Pos2::new(-1.0, -1.0), |s| s.min);
            let m = STATS.take();
            trace(&format!(
                "frame egui={:.0},{:.0} real={},{} sel={:.0},{:.0} drag={} moves={} fwd={} other={} forced={} slowest=0x{:04x}:{}us paint={}us",
                seen.x, seen.y, real.x - b.left, real.y - b.top, sel.x, sel.y,
                self.drag.is_some() as u8,
                m.moves, m.moves_forwarded, m.other, m.forced_paints, m.slowest.0, m.slowest.1, m.paint_us
            ));
        }

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
                self.selection = Some(self.desktop.monitors[self.current_monitor]);
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
        let monitor = self.desktop.monitors[self.current_monitor];
        let bottom = to_pt(monitor.center_bottom()) + vec2(0.0, -24.0);
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

    #[test]
    fn crop_stitches_across_monitors() {
        // Two 4x4 monitors side by side: left is red, right is blue and sits 2 px lower.
        let solid = |c: [u8; 4]| RgbaImage::from_pixel(4, 4, image::Rgba(c));
        let overlay = Overlay {
            desktop: Desktop {
                shots: vec![solid([255, 0, 0, 255]), solid([0, 0, 255, 255])],
                bounds: RECT { left: -4, top: 0, right: 4, bottom: 6 },
                monitors: vec![rect(0.0, 0.0, 4.0, 4.0), rect(4.0, 2.0, 8.0, 6.0)],
                home: 0,
            },
            textures: Vec::new(),
            hwnd: None,
            current_monitor: 0,
            selection: Some(rect(3.0, 1.0, 6.0, 4.0)),
            drag: None,
            error: None,
        };
        let out = overlay.cropped().unwrap();
        assert_eq!(out.dimensions(), (3, 3));
        assert_eq!(out.get_pixel(0, 0).0, [255, 0, 0, 255]); // left monitor
        assert_eq!(out.get_pixel(1, 0).0, [0, 0, 0, 0]); // above the right monitor: no screen there
        assert_eq!(out.get_pixel(1, 1).0, [0, 0, 255, 255]); // right monitor
        assert_eq!(out.get_pixel(2, 2).0, [0, 0, 255, 255]);
    }
}
