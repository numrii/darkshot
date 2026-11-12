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

use crate::annotate::{self, Annotation, History, Repeat, ShapeKind, MAX_WIDTH, MIN_WIDTH};
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

/// Like `edges_at`, but only the eight handle points count. Used for shapes, whose
/// edges are the stroke itself and must stay grabbable for moving.
fn handle_at(rect: Rect, p: Pos2, tol: f32) -> Option<(i8, i8)> {
    let xs = [(rect.left(), -1), (rect.center().x, 0), (rect.right(), 1)];
    let ys = [(rect.top(), -1), (rect.center().y, 0), (rect.bottom(), 1)];
    // Corners first, so they win over edge midpoints on small shapes.
    let mut best: Option<((i8, i8), f32)> = None;
    for (x, hx) in xs {
        for (y, hy) in ys {
            if (hx, hy) == (0, 0) {
                continue;
            }
            let d = p.distance(Pos2::new(x, y)) - if hx != 0 && hy != 0 { 0.5 } else { 0.0 };
            if d <= tol && best.is_none_or(|(_, b)| d < b) {
                best = Some(((hx, hy), d));
            }
        }
    }
    best.map(|(h, _)| h)
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum Tool {
    /// Adjust the capture region and select, move or resize existing annotations.
    Select,
    /// Drag to draw a new annotation of this kind.
    Shape(ShapeKind),
}

/// What the current mouse drag is doing.
#[derive(Clone, Copy)]
enum Action {
    /// Create, move or resize the capture region.
    Region(Drag),
    /// Drawing a new annotation (the last one in the list) from `anchor`.
    NewShape { anchor: Pos2 },
    MoveShape { index: usize, start: Pos2, orig: Annotation },
    /// Resizing a rectangle or ellipse by one of its handles.
    ResizeBox { index: usize, drag: Drag },
    /// Dragging one end of a line or arrow; `end_b` = the end (tip) rather than the start.
    MoveEnd { index: usize, end_b: bool },
}

/// Default annotation colours: red, green, blue, amber.
const SWATCHES: [Color32; 4] = [
    Color32::from_rgb(0xE5, 0x48, 0x4D),
    Color32::from_rgb(0x30, 0xA4, 0x6C),
    Color32::from_rgb(0x3E, 0x63, 0xDD),
    Color32::from_rgb(0xFF, 0xB2, 0x24),
];
const ACCENT: Color32 = Color32::from_rgb(0x6C, 0xB6, 0xFF);

struct Overlay {
    desktop: Desktop,
    /// One texture per monitor, so no texture exceeds the GPU's size limit. Created on
    /// the first frame: the real limit is not known before then.
    textures: Vec<egui::TextureHandle>,
    hwnd: Option<HWND>,
    /// The monitor the pointer was last on; Ctrl+A and the hint bar use it.
    current_monitor: usize,
    /// The capture region in desktop pixels.
    selection: Option<Rect>,
    action: Option<Action>,
    /// The annotation list as it was when the current drag began. Recorded in the undo
    /// history only if the drag actually changes something, so a plain click adds nothing.
    pending_undo: Option<Vec<Annotation>>,

    tool: Tool,
    /// Colour and stroke width for new annotations; mirrors the selected one, if any.
    color: Color32,
    width: f32,
    annotations: Vec<Annotation>,
    selected: Option<usize>,
    history: History,

    error: Option<String>,
}

/// What the toolbar asked for this frame.
#[derive(Default)]
struct ToolbarOutput {
    copy: bool,
    save: bool,
    close: bool,
}

impl Overlay {
    fn with_desktop(desktop: Desktop, hwnd: Option<HWND>) -> Self {
        Self {
            current_monitor: desktop.home,
            desktop,
            textures: Vec::new(),
            hwnd,
            selection: None,
            action: None,
            pending_undo: None,
            tool: Tool::Select,
            color: SWATCHES[0],
            width: 4.0,
            annotations: Vec::new(),
            selected: None,
            history: History::default(),
            error: None,
        }
    }

    fn new(cc: &eframe::CreationContext<'_>, desktop: Desktop) -> Self {
        trace("window created");
        let overlay = Self::with_desktop(desktop, hwnd_of(cc));
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

    // --- Mouse interaction -----------------------------------------------------------

    /// What a drag starting at `p` would do. Pure, so it also drives the cursor shape.
    /// Priority: handles of the selected annotation, then the region's edges, then
    /// whatever the active tool does at that point.
    fn pick_action(&self, p: Pos2, tol: f32) -> Action {
        let Some(sel) = self.selection else {
            return Action::Region(Drag::Create { anchor: p });
        };

        if let Some(index) = self.selected {
            let shape = self.annotations[index];
            if shape.is_box() {
                if let Some((hx, hy)) = handle_at(shape.rect(), p, tol) {
                    let drag = Drag::Resize { start: p, orig: shape.rect(), hx, hy };
                    return Action::ResizeBox { index, drag };
                }
            } else if p.distance(shape.b) <= tol {
                return Action::MoveEnd { index, end_b: true };
            } else if p.distance(shape.a) <= tol {
                return Action::MoveEnd { index, end_b: false };
            }
        }

        if let Some((hx, hy)) = edges_at(sel, p, tol) {
            return Action::Region(Drag::Resize { start: p, orig: sel, hx, hy });
        }
        if !sel.contains(p) {
            return Action::Region(Drag::Create { anchor: p });
        }

        let move_shape = |index: usize| Action::MoveShape {
            index,
            start: p,
            orig: self.annotations[index],
        };
        match self.tool {
            // Topmost annotation under the pointer, else the region itself.
            Tool::Select => match self.annotations.iter().rposition(|a| a.hit(p, tol)) {
                Some(index) => move_shape(index),
                None => Action::Region(Drag::Move { start: p, orig: sel }),
            },
            // While drawing, only the selected annotation can be grabbed, so new
            // shapes can start on top of existing ones.
            Tool::Shape(_) => match self.selected {
                Some(index) if self.annotations[index].hit(p, tol) => move_shape(index),
                _ => Action::NewShape { anchor: p },
            },
        }
    }

    fn begin_action(&mut self, action: Action) {
        match action {
            Action::Region(Drag::Create { .. } | Drag::Move { .. }) => self.selected = None,
            Action::Region(Drag::Resize { .. }) => {}
            Action::NewShape { anchor } => {
                let Tool::Shape(kind) = self.tool else { return };
                self.history.record(&self.annotations, None);
                self.annotations.push(Annotation {
                    kind,
                    a: anchor,
                    b: anchor,
                    color: self.color,
                    width: self.width,
                });
                self.selected = Some(self.annotations.len() - 1);
            }
            Action::MoveShape { index, .. }
            | Action::ResizeBox { index, .. }
            | Action::MoveEnd { index, .. } => {
                self.select(index);
                self.pending_undo = Some(self.annotations.clone());
            }
        }
        self.action = Some(action);
    }

    /// Update whatever is being dragged for the pointer at `p`.
    fn apply_action(&mut self, p: Pos2, mods: Modifiers, bounds: Rect) {
        let Some(action) = self.action else { return };
        match action {
            Action::Region(drag) => {
                // A stray click outside the region must not replace it with nothing.
                if let Drag::Create { anchor } = drag {
                    if anchor.distance(p) < 3.0 {
                        return;
                    }
                }
                self.selection = Some(drag.apply(p, mods, bounds));
            }
            Action::NewShape { anchor } => {
                let Some(shape) = self.annotations.last_mut() else { return };
                if shape.is_box() {
                    shape.set_rect(Drag::Create { anchor }.apply(p, mods, bounds));
                } else {
                    let end = if mods.shift { annotate::snap_angle(anchor, p) } else { p };
                    shape.a = if mods.alt { anchor - (end - anchor) } else { anchor };
                    shape.b = end;
                }
            }
            Action::MoveShape { index, start, orig } => {
                let mut d = p - start;
                if mods.shift {
                    if d.x.abs() > d.y.abs() {
                        d.y = 0.0;
                    } else {
                        d.x = 0.0;
                    }
                }
                let mut moved = orig;
                moved.translate(d.round());
                self.change(index, moved);
            }
            Action::ResizeBox { index, drag } => {
                let mut shape = self.annotations[index];
                shape.set_rect(drag.apply(p, mods, bounds));
                self.change(index, shape);
            }
            Action::MoveEnd { index, end_b } => {
                let mut shape = self.annotations[index];
                let fixed = if end_b { shape.a } else { shape.b };
                let end = if mods.shift { annotate::snap_angle(fixed, p) } else { p };
                if end_b {
                    shape.b = end;
                } else {
                    shape.a = end;
                }
                self.change(index, shape);
            }
        }
    }

    fn end_action(&mut self) {
        match self.action.take() {
            Some(Action::NewShape { .. }) => {
                // A click without a drag leaves no shape behind, and no undo step.
                if self.annotations.last().is_some_and(Annotation::is_degenerate) {
                    self.annotations.pop();
                    self.history.forget_last();
                    self.selected = None;
                }
            }
            Some(Action::Region(_)) => {
                if self.selection.is_some_and(|s| s.width() < 1.0 || s.height() < 1.0) {
                    self.selection = None;
                }
            }
            _ => {}
        }
        self.pending_undo = None;
    }

    /// Replace annotation `index` during a drag, recording the undo step on first change.
    fn change(&mut self, index: usize, new: Annotation) {
        if self.annotations[index] == new {
            return;
        }
        if let Some(before) = self.pending_undo.take() {
            self.history.record(&before, None);
        }
        self.annotations[index] = new;
    }

    // --- Editing commands --------------------------------------------------------------

    fn select(&mut self, index: usize) {
        self.selected = Some(index);
        self.color = self.annotations[index].color;
        self.width = self.annotations[index].width;
    }

    /// Push the current colour and width onto the selected annotation, if they differ.
    fn restyle_selected(&mut self) {
        let Some(index) = self.selected else { return };
        let mut shape = self.annotations[index];
        shape.color = self.color;
        shape.width = self.width;
        if shape != self.annotations[index] {
            self.history.record(&self.annotations, Some(Repeat::Style));
            self.annotations[index] = shape;
        }
    }

    fn nudge_selected(&mut self, index: usize, by: Vec2) {
        self.history.record(&self.annotations, Some(Repeat::Nudge));
        self.annotations[index].translate(by);
    }

    fn delete_selected(&mut self) {
        if let Some(index) = self.selected.take() {
            self.history.record(&self.annotations, None);
            self.annotations.remove(index);
        }
    }

    fn undo(&mut self) {
        if self.history.undo(&mut self.annotations) {
            self.selected = None;
        }
    }

    fn redo(&mut self) {
        if self.history.redo(&mut self.annotations) {
            self.selected = None;
        }
    }

    // --- Output ------------------------------------------------------------------------

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

    /// The final image: the cropped capture with every annotation drawn onto it.
    fn output(&self) -> Option<RgbaImage> {
        let origin = self.selection?.intersect(self.bounds()).min;
        let mut img = self.cropped()?;
        annotate::render_onto(&mut img, origin, &self.annotations);
        Some(img)
    }

    fn copy_to_clipboard(img: &RgbaImage) -> AnyResult<()> {
        // Test hook: write the "copied" image to a file instead, so automated checks
        // can inspect the output without touching the user's clipboard.
        if let Some(path) = std::env::var_os("DARKSHOT_COPY_TO") {
            img.save(path)?;
            return Ok(());
        }
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

    // --- Toolbar -----------------------------------------------------------------------

    fn toolbar(&mut self, ctx: &egui::Context, pos: Pos2, pivot: Align2, within: Rect) -> ToolbarOutput {
        let mut out = ToolbarOutput::default();
        egui::Area::new(egui::Id::unique("darkshot-toolbar"))
            .order(egui::Order::Foreground)
            .pivot(pivot)
            .fixed_pos(pos)
            .constrain_to(within)
            .show(ctx, |ui| {
                egui::Frame::popup(ui.style()).show(ui, |ui| {
                    ui.horizontal(|ui| {
                        let tools = [
                            (Tool::Select, "Select", "Select, move and resize (V)"),
                            (Tool::Shape(ShapeKind::Rect), "Rect", "Rectangle (R)"),
                            (Tool::Shape(ShapeKind::Ellipse), "Ellipse", "Ellipse (E)"),
                            (Tool::Shape(ShapeKind::Line), "Line", "Line (L)"),
                            (Tool::Shape(ShapeKind::Arrow), "Arrow", "Arrow (A)"),
                        ];
                        for (tool, name, tip) in tools {
                            if ui.selectable_label(self.tool == tool, name).on_hover_text(tip).clicked() {
                                self.tool = tool;
                            }
                        }

                        ui.separator();
                        for swatch in SWATCHES {
                            let (rect, response) =
                                ui.allocate_exact_size(Vec2::splat(18.0), Sense::click());
                            ui.painter().rect_filled(rect.shrink(2.0), 3.0, swatch);
                            if self.color == swatch {
                                let ring = Stroke::new(1.5, Color32::WHITE);
                                ui.painter().rect_stroke(rect, 4.0, ring, StrokeKind::Inside);
                            }
                            if response.clicked() {
                                self.color = swatch;
                            }
                        }
                        ui.color_edit_button_srgba(&mut self.color)
                            .on_hover_text("Custom colour");
                        ui.add(
                            egui::DragValue::new(&mut self.width)
                                .range(MIN_WIDTH..=MAX_WIDTH)
                                .speed(0.25)
                                .fixed_decimals(0)
                                .suffix(" px"),
                        )
                        .on_hover_text("Thickness (mouse wheel)");

                        ui.separator();
                        let undo = ui.add_enabled(self.history.can_undo(), egui::Button::new("Undo"));
                        if undo.on_hover_text("Ctrl+Z").clicked() {
                            self.undo();
                        }
                        let redo = ui.add_enabled(self.history.can_redo(), egui::Button::new("Redo"));
                        if redo.on_hover_text("Ctrl+Y").clicked() {
                            self.redo();
                        }

                        ui.separator();
                        out.copy = ui.button("Copy").on_hover_text("Ctrl+C").clicked();
                        out.save = ui.button("Save").on_hover_text("Ctrl+S").clicked();
                        out.close = ui.button("Close").on_hover_text("Esc").clicked();
                    });
                });
            });
        out
    }
}

/// Text on a dark pill so it stays readable over any screenshot.
fn label(painter: &egui::Painter, pos: Pos2, anchor: Align2, text: String, color: Color32) {
    let galley = painter.layout_no_wrap(text, FontId::proportional(13.0), color);
    let rect = anchor.anchor_size(pos, galley.size());
    painter.rect_filled(rect.expand2(vec2(6.0, 3.0)), 4.0, Color32::from_black_alpha(200));
    painter.galley(rect.min, galley, color);
}

/// A small square grab handle centred on `at` (in points).
fn handle(painter: &egui::Painter, at: Pos2, fill: Color32) {
    let rect = Rect::from_center_size(at, Vec2::splat(HANDLE_SIZE));
    painter.rect_filled(rect, 0.0, fill);
    painter.rect_stroke(rect, 0.0, Stroke::new(1.0, Color32::from_gray(40)), StrokeKind::Inside);
}

/// The eight handle positions of a rectangle: corners and edge midpoints.
fn handle_points(r: Rect) -> impl Iterator<Item = Pos2> {
    let xs = [r.left(), r.center().x, r.right()];
    let ys = [r.top(), r.center().y, r.bottom()];
    (0..9).filter(|i| *i != 4).map(move |i| Pos2::new(xs[i % 3], ys[i / 3]))
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
        let rect_to_pt = |r: Rect| Rect::from_min_max(to_pt(r.min), to_pt(r.max));
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
            painter.image(texture.id(), rect_to_pt(*m), uv, Color32::WHITE);
        }

        // --- Mouse ---------------------------------------------------------------------
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
                self.action.is_some() as u8,
                m.moves, m.moves_forwarded, m.other, m.forced_paints, m.slowest.0, m.slowest.1, m.paint_us
            ));
        }

        if response.drag_started() {
            if let Some(origin) = press_origin.or(response.interact_pointer_pos()) {
                let action = self.pick_action(to_px(origin), tol);
                self.begin_action(action);
                self.error = None;
            }
        }
        if response.dragged() {
            if let Some(p) = response.interact_pointer_pos() {
                self.apply_action(to_px(p), mods, bounds);
            }
        }
        if response.drag_stopped() {
            self.end_action();
        }

        // Cursor reflects what a drag would do from here. Over the toolbar, egui decides.
        let preview = self.action.or_else(|| {
            let p = hover.filter(|_| response.hovered())?;
            Some(self.pick_action(to_px(p), tol))
        });
        if let Some(action) = preview {
            ctx.set_cursor_icon(match action {
                Action::Region(Drag::Resize { hx, hy, .. })
                | Action::ResizeBox { drag: Drag::Resize { hx, hy, .. }, .. } => resize_cursor(hx, hy),
                Action::Region(Drag::Move { .. }) | Action::MoveShape { .. } => CursorIcon::Move,
                Action::MoveEnd { .. } => CursorIcon::Grab,
                _ => CursorIcon::Crosshair,
            });
        }

        // --- Keyboard and wheel ----------------------------------------------------------
        struct Keys {
            nudge: Vec2,
            select_all: bool,
            copy: bool,
            save: bool,
            cancel: bool,
            undo: bool,
            redo: bool,
            delete: bool,
            tool: Option<Tool>,
            wheel: f32,
        }
        let typing = ctx.egui_wants_keyboard_input();
        let keys = ctx.input(|i| {
            let step = if i.modifiers.shift { 10.0 } else { 1.0 };
            let axis = |neg: Key, pos: Key| {
                (i.num_presses(pos) as f32 - i.num_presses(neg) as f32) * step
            };
            let ctrl = i.modifiers.command;
            let plain = !ctrl && !i.modifiers.alt;
            let tool = [
                (Key::V, Tool::Select),
                (Key::R, Tool::Shape(ShapeKind::Rect)),
                (Key::E, Tool::Shape(ShapeKind::Ellipse)),
                (Key::L, Tool::Shape(ShapeKind::Line)),
                (Key::A, Tool::Shape(ShapeKind::Arrow)),
            ]
            .into_iter()
            .find(|(key, _)| plain && i.key_pressed(*key))
            .map(|(_, tool)| tool);
            let wheel: f32 = i
                .events
                .iter()
                .map(|e| match e {
                    egui::Event::MouseWheel { delta, .. } => delta.y,
                    _ => 0.0,
                })
                .sum();
            Keys {
                nudge: Vec2::new(
                    axis(Key::ArrowLeft, Key::ArrowRight),
                    axis(Key::ArrowUp, Key::ArrowDown),
                ),
                select_all: ctrl && i.key_pressed(Key::A),
                // egui turns Ctrl+C into Event::Copy rather than a key press.
                copy: i.events.iter().any(|e| matches!(e, egui::Event::Copy))
                    || i.key_pressed(Key::Enter),
                save: ctrl && i.key_pressed(Key::S),
                cancel: i.key_pressed(Key::Escape),
                undo: ctrl && !i.modifiers.shift && i.key_pressed(Key::Z),
                redo: ctrl && (i.key_pressed(Key::Y) || i.modifiers.shift && i.key_pressed(Key::Z)),
                delete: i.key_pressed(Key::Delete) || i.key_pressed(Key::Backspace),
                tool,
                wheel,
            }
        });

        let mut close = false;
        if self.action.is_none() && !typing {
            if keys.undo {
                self.undo();
            } else if keys.redo {
                self.redo();
            } else if keys.delete {
                self.delete_selected();
            } else if keys.select_all {
                self.selection = Some(self.desktop.monitors[self.current_monitor]);
            } else if keys.nudge != Vec2::ZERO {
                // Arrows move the selected annotation if there is one, else the region.
                if let Some(index) = self.selected {
                    self.nudge_selected(index, keys.nudge);
                } else if let Some(sel) = self.selection {
                    self.selection = Some(keep_inside(sel.translate(keys.nudge), bounds));
                }
            }
            if let Some(tool) = keys.tool {
                self.tool = tool;
            }
            if keys.cancel {
                // Esc first lets go of the selected annotation, then closes.
                if self.selected.take().is_none() {
                    close = true;
                }
            }
        }
        if keys.wheel != 0.0 && response.hovered() {
            self.width = (self.width + keys.wheel.signum()).clamp(MIN_WIDTH, MAX_WIDTH);
        }

        // --- Toolbar -------------------------------------------------------------------
        let creating_region = matches!(self.action, Some(Action::Region(Drag::Create { .. })));
        let mut toolbar = ToolbarOutput::default();
        if let (Some(sel_px), false) = (self.selection, creating_region) {
            let sel = rect_to_pt(sel_px);
            // Below the region's bottom-right corner, or tucked inside when the region
            // reaches the bottom of its monitor.
            let corner = sel_px.max - vec2(1.0, 1.0);
            let monitor = self
                .desktop
                .monitors
                .iter()
                .find(|m| m.contains(corner))
                .copied()
                .unwrap_or(self.desktop.monitors[self.current_monitor]);
            let monitor = rect_to_pt(monitor);
            let (pos, pivot) = if monitor.bottom() - sel.bottom() > 48.0 {
                (sel.right_bottom() + vec2(0.0, 8.0), Align2::RIGHT_TOP)
            } else {
                (sel.right_bottom() + vec2(-8.0, -8.0), Align2::RIGHT_BOTTOM)
            };
            toolbar = self.toolbar(&ctx, pos, pivot, monitor);
        }
        self.restyle_selected();

        // --- Paint: annotations, dimming, region outline and handles, labels -----------
        for annotation in &self.annotations {
            annotation.paint(&painter, to_pt, scale);
        }

        let dim = Color32::from_black_alpha(130);
        match self.selection {
            Some(sel_px) => {
                let sel = rect_to_pt(sel_px);
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
                for point in handle_points(sel) {
                    handle(&painter, point, Color32::WHITE);
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

        // Handles of the selected annotation, in an accent colour to tell them apart
        // from the region's.
        if let Some(shape) = self.selected.map(|i| self.annotations[i]) {
            if shape.is_box() {
                let r = rect_to_pt(shape.rect());
                painter.rect_stroke(r, 0.0, Stroke::new(1.0, ACCENT), StrokeKind::Middle);
                for point in handle_points(r) {
                    handle(&painter, point, ACCENT);
                }
            } else {
                handle(&painter, to_pt(shape.a), ACCENT);
                handle(&painter, to_pt(shape.b), ACCENT);
            }
        }

        let hint = if self.selection.is_none() {
            "Drag to select an area · Ctrl+A whole screen · Esc cancel"
        } else if self.selected.is_some() {
            "Drag to move · handles resize · arrows nudge · wheel thickness · Del delete · Esc deselect"
        } else {
            "V select · R rectangle · E ellipse · L line · A arrow · Shift constrain · Alt from centre · Ctrl+Z undo"
        };
        let monitor = self.desktop.monitors[self.current_monitor];
        let bottom = to_pt(monitor.center_bottom()) + vec2(0.0, -24.0);
        label(&painter, bottom, Align2::CENTER_BOTTOM, hint.to_owned(), Color32::WHITE);
        if let Some(err) = &self.error {
            let pos = bottom + vec2(0.0, -30.0);
            label(&painter, pos, Align2::CENTER_BOTTOM, err.clone(), Color32::LIGHT_RED);
        }

        // --- Actions -------------------------------------------------------------------
        let copy = toolbar.copy || keys.copy && !typing;
        let save = toolbar.save || keys.save && !typing;
        if close || toolbar.close {
            ctx.send_viewport_cmd(egui::ViewportCommand::Close);
        } else if copy || save {
            if let Some(img) = self.output() {
                let result = if copy {
                    Self::copy_to_clipboard(&img).map(|()| true)
                } else {
                    Self::save_to_file(&img, frame)
                };
                match result {
                    Ok(true) => ctx.send_viewport_cmd(egui::ViewportCommand::Close),
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
        let mut overlay = Overlay::with_desktop(
            Desktop {
                shots: vec![solid([255, 0, 0, 255]), solid([0, 0, 255, 255])],
                bounds: RECT { left: -4, top: 0, right: 4, bottom: 6 },
                monitors: vec![rect(0.0, 0.0, 4.0, 4.0), rect(4.0, 2.0, 8.0, 6.0)],
                home: 0,
            },
            None,
        );
        overlay.selection = Some(rect(3.0, 1.0, 6.0, 4.0));
        let out = overlay.cropped().unwrap();
        assert_eq!(out.dimensions(), (3, 3));
        assert_eq!(out.get_pixel(0, 0).0, [255, 0, 0, 255]); // left monitor
        assert_eq!(out.get_pixel(1, 0).0, [0, 0, 0, 0]); // above the right monitor: no screen there
        assert_eq!(out.get_pixel(1, 1).0, [0, 0, 255, 255]); // right monitor
        assert_eq!(out.get_pixel(2, 2).0, [0, 0, 255, 255]);
    }
}
