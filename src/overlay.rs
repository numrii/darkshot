//! The capture overlay: freeze every monitor, show the frozen desktop in one window
//! spanning all of them, let the user pick and adjust a region, then copy or save it.

use std::cell::Cell;
use std::collections::{HashMap, HashSet};
use std::hash::{Hash, Hasher};
use std::io::Write;
use std::sync::{Mutex, OnceLock};
use std::time::{Duration, Instant};

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

use crate::annotate::{
    self, Annotation, ArrowHead, Dash, Effect, History, Repeat, ShapeKind, TextStyle, MAX_WIDTH,
    MIN_WIDTH,
};
use crate::effects;
use crate::help::{self, HelpLevel};
use crate::read;
use crate::save::{self, Format, SaveOptions, QUALITY_HIGH, QUALITY_MEDIUM};
use crate::settings::{GuideSettings, Settings};
use crate::snap::{self, Snap};
use crate::text::Layout;
use crate::tools::{ToolInfo, TOOLS};
use crate::icons::{self, Icon};
use crate::AnyResult;

/// How close (in points) the pointer must be to an edge or handle to grab it.
const GRAB_TOLERANCE: f32 = 6.0;
const HANDLE_SIZE: f32 = 7.0;

/// Where trace lines go: `--trace` writes `duskshot-trace.log` in the temp folder,
/// the `DUSKSHOT_TRACE` environment variable writes to stderr. `None` = tracing off.
/// Only builds with the `trace` feature, and debug builds, can trace at all.
fn trace_sink() -> Option<&'static Mutex<Box<dyn Write + Send>>> {
    if !cfg!(any(feature = "trace", debug_assertions)) {
        return None;
    }
    static SINK: OnceLock<Option<Mutex<Box<dyn Write + Send>>>> = OnceLock::new();
    SINK.get_or_init(|| {
        if std::env::args().any(|a| a == "--trace") {
            let path = std::env::temp_dir().join("duskshot-trace.log");
            let file = std::fs::File::create(path).ok()?;
            Some(Mutex::new(Box::new(std::io::LineWriter::new(file)) as Box<dyn Write + Send>))
        } else if std::env::var_os("DUSKSHOT_TRACE").is_some() {
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

/// What the overlay opens with.
pub enum Source {
    /// A capture of the screen, taken now.
    Capture,
    /// An image file to edit; `None` asks which one.
    File(Option<std::path::PathBuf>),
    /// The image on the clipboard, to edit.
    Clipboard,
    /// A capture kept by the tray for Reopen last capture, read from standard input.
    Kept,
}

/// Open the overlay over `source` and run it until the user copies, saves or cancels.
pub fn run(source: Source) -> AnyResult<()> {
    // Must happen first: without it Windows reports scaled, not physical, coordinates.
    unsafe { SetProcessDpiAwarenessContext(DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE_V2) };

    // Grab the screen before our own window exists so the overlay is never in the shot.
    trace("start");
    crate::perf::sample("start");
    let settings = Settings::load();
    let mut kept = None;
    let desktop = match source {
        Source::Kept => {
            let mut bytes = Vec::new();
            std::io::Read::read_to_end(&mut std::io::stdin().lock(), &mut bytes)?;
            let (desktop, state) = read_kept(&bytes).map_err(|e| format!("Could not reopen the capture: {e}"))?;
            kept = Some(state);
            desktop
        }
        Source::Capture => Desktop::capture(settings.include_cursor)?,
        Source::File(path) => {
            let path = path.or_else(|| {
                rfd::FileDialog::new()
                    .set_title("Open an image to edit")
                    .add_filter("Images", &["png", "jpg", "jpeg"])
                    .pick_file()
            });
            let Some(path) = path else { return Ok(()) };
            match open_file(&path) {
                Ok(desktop) => desktop,
                Err(e) => {
                    let name = path.file_name().unwrap_or_default().to_string_lossy();
                    return complain(&format!("Could not open {name}.\n\n{e}"));
                }
            }
        }
        Source::Clipboard => match clipboard_image() {
            Some(img) => Desktop::open(img, Opened::default()),
            None => return complain("There is no image on the clipboard to edit."),
        },
    };
    trace("captured");
    crate::perf::sample("captured");

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
        // Wait for the display's refresh with a one-frame queue: the lowest latency that
        // never draws more frames than the screen shows. Without the wait, moving the
        // mouse drew hundreds of frames a second, which kept the GPU busy in bursts
        // (audible as coil whine on some machines) for frames nobody could see.
        wgpu_options: gpu_options(eframe::egui_wgpu::SurfaceConfig {
            present_mode: eframe::wgpu::PresentMode::AutoVsync,
            desired_maximum_frame_latency: Some(1),
        }),
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
        "Duskshot",
        options,
        Box::new(move |cc| {
            let mut overlay = Overlay::new(cc, desktop, &settings);
            if let Some(kept) = kept {
                overlay.restore(kept);
            }
            // Only an overlay the tray started can be reopened: the tray keeps it.
            overlay.handoff = std::env::args().any(|a| a == "--tray") && settings.keep_minutes > 0;
            Ok(Box::new(overlay))
        }),
        &event_loop,
    );
    event_loop.run_app(&mut app)?;
    // After the window and its GPU resources are gone: what is left should be little.
    drop(app);
    crate::perf::sample("closed");
    Ok(())
}

/// Every tool, in a fixed order for kept captures.
fn all_tools() -> Vec<Tool> {
    std::iter::once(Tool::Select).chain(crate::handoff::SHAPES.iter().map(|&k| Tool::Shape(k))).collect()
}

const STYLE_SLOTS: [StyleSlot; 5] = [StyleSlot::Shared, StyleSlot::Marker, StyleSlot::Text, StyleSlot::Blur, StyleSlot::Step];

/// A kept capture's state besides the desktop, read back (see `Overlay::kept_bytes`).
struct Kept {
    current_monitor: usize,
    selection: Option<Rect>,
    rotation: Rotation,
    tool: Tool,
    color: Color32,
    width: f32,
    dash: Dash,
    head: ArrowHead,
    double_head: bool,
    text_style: TextStyle,
    effect: Effect,
    emoji: String,
    styles: Vec<(StyleSlot, Color32)>,
    widths: Vec<(Tool, f32)>,
    border_on: bool,
    border: effects::Border,
    border_unset: bool,
    shadow: bool,
    zoom: f32,
    pan: Vec2,
    annotations: Vec<Annotation>,
    history: History,
}

/// Read a kept capture: the desktop to open over, and the rest to put back.
fn read_kept(bytes: &[u8]) -> crate::handoff::Result<(Desktop, Kept)> {
    let mut r = crate::handoff::Reader::new(bytes);
    if r.take(4)? != crate::handoff::MAGIC {
        return Err("not a kept capture");
    }
    let shots = r.list(|r| {
        let png = r.bytes()?;
        image::load_from_memory_with_format(png, image::ImageFormat::Png).map(|i| i.to_rgba8()).map_err(|_| "unreadable screen")
    })?;
    let bounds = RECT { left: r.i32()?, top: r.i32()?, right: r.i32()?, bottom: r.i32()? };
    let monitors = r.list(|r| r.rect())?;
    let home = r.u32()? as usize;
    let windows = r.list(|r| r.rect())?;
    let screens = r.list(|r| r.rect())?;
    let space = r.vec2()?;
    let opened = r.option(|r| {
        Ok(Opened {
            stem: r.option(|r| r.str())?,
            folder: r.option(|r| r.str().map(std::path::PathBuf::from))?,
            format: r.option(|r| r.choice(&[Format::Png, Format::Jpeg]))?,
        })
    })?;
    if shots.is_empty() || shots.len() != monitors.len() || screens.len() != monitors.len() {
        return Err("no screens");
    }
    let snap = Snap::new(windows, &monitors);
    let desktop = Desktop { shots, bounds, home: home.min(monitors.len() - 1), monitors, snap, screens, space, opened };

    let dashes = Dash::ALL.map(|d| d.0);
    let kept = Kept {
        current_monitor: r.u32()? as usize,
        selection: r.option(|r| r.rect())?,
        rotation: Rotation { angle: r.f32()?, offset: r.vec2()? },
        tool: r.choice(&all_tools())?,
        color: r.color()?,
        width: r.f32()?,
        dash: r.choice(&dashes)?,
        head: r.choice(&ArrowHead::ALL.map(|h| h.0))?,
        double_head: r.bool()?,
        text_style: r.choice(&TextStyle::ALL.map(|t| t.0))?,
        effect: r.choice(&Effect::ALL.map(|e| e.0))?,
        emoji: r.str()?,
        styles: r.list(|r| Ok((r.choice(&STYLE_SLOTS)?, r.color()?)))?,
        widths: r.list(|r| Ok((r.choice(&all_tools())?, r.f32()?)))?,
        border_on: r.bool()?,
        border: effects::Border { color: r.color()?, width: r.f32()?, dash: r.choice(&dashes)? },
        border_unset: r.bool()?,
        shadow: r.bool()?,
        zoom: r.f32()?,
        pan: r.vec2()?,
        annotations: r.annotations()?,
        history: {
            let undo = r.list(|r| r.annotations())?;
            let redo = r.list(|r| r.annotations())?;
            History::from_steps(undo, redo)
        },
    };
    r.done()?;
    Ok((desktop, kept))
}

/// How Duskshot's windows use the GPU, set to hold as little as it can:
/// - Direct3D 12 alone. By default wgpu starts Vulkan, Direct3D 12 and OpenGL to choose
///   between them, which alone commits over 100 MB, and then picks Vulkan, whose driver
///   reserves several times the graphics memory. Without a Direct3D 12 GPU, the usual
///   choice.
/// - Memory reserved in small blocks rather than large ones kept for speed.
///
/// The high-performance GPU stays the choice: an integrated GPU's memory is the
/// computer's own RAM, so the low-power one used more of it (measured: 440 MB against
/// 216 MB). `WGPU_BACKEND` and `WGPU_POWER_PREF` override the choices.
pub(crate) fn gpu_options(surface: eframe::egui_wgpu::SurfaceConfig) -> eframe::egui_wgpu::WgpuConfiguration {
    use eframe::egui_wgpu::{WgpuConfiguration, WgpuSetup, WgpuSetupCreateNew};
    use eframe::wgpu;
    let mut setup = WgpuSetupCreateNew::without_display_handle();
    if wgpu::Backends::from_env().is_none() {
        setup.instance_descriptor.backends =
            if has_dx12_gpu() { wgpu::Backends::DX12 } else { wgpu::Backends::PRIMARY | wgpu::Backends::GL };
    }
    let device = setup.device_descriptor.clone();
    setup.device_descriptor =
        std::sync::Arc::new(move |adapter| wgpu::DeviceDescriptor { memory_hints: wgpu::MemoryHints::MemoryUsage, ..device(adapter) });
    WgpuConfiguration { wgpu_setup: WgpuSetup::CreateNew(setup), ..WgpuConfiguration::default() }.with_surface_config(surface)
}

/// Whether a GPU can be used through Direct3D 12.
fn has_dx12_gpu() -> bool {
    use eframe::wgpu;
    let instance = wgpu::Instance::new(wgpu::InstanceDescriptor {
        backends: wgpu::Backends::DX12,
        ..wgpu::InstanceDescriptor::new_without_display_handle()
    });
    // Answered at once on Windows: one poll is enough.
    let mut found = std::pin::pin!(instance.enumerate_adapters(wgpu::Backends::DX12));
    let mut context = std::task::Context::from_waker(std::task::Waker::noop());
    match found.as_mut().poll(&mut context) {
        std::task::Poll::Ready(adapters) => !adapters.is_empty(),
        std::task::Poll::Pending => false,
    }
}

/// Tell the user why there is nothing to show, and stop.
fn complain(message: &str) -> AnyResult<()> {
    rfd::MessageDialog::new()
        .set_level(rfd::MessageLevel::Warning)
        .set_title("Duskshot")
        .set_description(message)
        .show();
    Ok(())
}

/// An image file to edit, the right way up: photos taken with the camera turned say
/// so in their EXIF data rather than in their pixels.
fn open_file(path: &std::path::Path) -> AnyResult<Desktop> {
    use image::ImageDecoder;
    let reader = image::ImageReader::open(path)?.with_guessed_format()?;
    let unreadable = |e: image::ImageError| -> Box<dyn std::error::Error> {
        match e {
            image::ImageError::Unsupported(_) | image::ImageError::Decoding(_) => {
                "Duskshot opens PNG and JPEG images, and could not read this file as either.".into()
            }
            e => e.into(),
        }
    };
    let mut decoder = reader.into_decoder().map_err(unreadable)?;
    let orientation = decoder.orientation()?;
    let mut img = image::DynamicImage::from_decoder(decoder).map_err(unreadable)?;
    img.apply_orientation(orientation);
    let extension = path.extension().map(|e| e.to_string_lossy().to_lowercase());
    let opened = Opened {
        stem: path.file_stem().map(|s| s.to_string_lossy().into_owned()),
        folder: path.parent().map(std::path::Path::to_path_buf),
        format: match extension.as_deref() {
            Some("png") => Some(Format::Png),
            Some("jpg" | "jpeg") => Some(Format::Jpeg),
            _ => None,
        },
    };
    Ok(Desktop::open(img.to_rgba8(), opened))
}

/// The image on the clipboard, if there is one.
fn clipboard_image() -> Option<RgbaImage> {
    let data = arboard::Clipboard::new().ok()?.get_image().ok()?;
    RgbaImage::from_raw(data.width as u32, data.height as u32, data.bytes.into_owned())
}

/// Where an opened image came from, for saving it again.
#[derive(Clone, Debug, Default, PartialEq)]
struct Opened {
    /// Its file name without extension.
    stem: Option<String>,
    folder: Option<std::path::PathBuf>,
    /// Its format, to save it in the same one.
    format: Option<Format>,
}

/// An image opened for editing, on a monitor `screen` pixels in size: the pixel space
/// the overlay works in, and where the image sits in it. The space has the monitor's
/// shape, enlarged when the image does not fit with room round it for the toolbar, so
/// a large image is shown scaled down while staying in its own pixels.
fn place_image(image: Vec2, screen: Vec2) -> (Vec2, Rect) {
    let k = 1f32.max(image.x / (screen.x * 0.9)).max(image.y / (screen.y * 0.85));
    let space = (screen * k).ceil();
    let at = ((space - image) / 2.0).floor().to_pos2();
    (space, Rect::from_min_size(at, image))
}

/// A frozen capture of the whole virtual desktop, or an image opened for editing.
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
    /// The windows on screen at capture time, for Ctrl-snapping.
    snap: Snap,
    /// The areas the toolbar, popups and hints are kept in, in desktop pixels: the
    /// monitors. For an opened image `monitors` is just the image; this is the screen.
    screens: Vec<Rect>,
    /// The size of the space desktop pixels are in: the virtual desktop, or for an
    /// opened image, its monitor enlarged as needed to fit it (see `place_image`).
    /// `bounds` is where the window goes on screen.
    space: Vec2,
    /// Where an opened image came from; `None` for a capture.
    opened: Option<Opened>,
}

impl Desktop {
    /// Capture every monitor, with the mouse cursor drawn in if `include_cursor`.
    fn capture(include_cursor: bool) -> AnyResult<Self> {
        let mut cursor = POINT { x: 0, y: 0 };
        unsafe { GetCursorPos(&mut cursor) };
        // Read first, so it is the cursor as it was when the capture was asked for.
        let pointer = if include_cursor { crate::cursor::Cursor::current() } else { None };

        // Monitors showing HDR are read in their own format and converted (see `hdr`);
        // the ordinary capture of one looks washed out.
        let hdr = crate::hdr::Hdr::detect();
        let mut shots = Vec::new();
        for monitor in xcap::Monitor::all()? {
            let (x, y) = (monitor.x()?, monitor.y()?);
            let shot = match hdr.as_ref().and_then(|hdr| hdr.capture(x, y)) {
                Some(shot) => shot,
                None => monitor.capture_image()?,
            };
            shots.push((x, y, shot));
            trace("monitor captured");
        }
        if let Some(pointer) = pointer {
            pointer.draw(&mut shots);
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
            snap: Snap::new(snap::visible_windows(left, top), &monitors),
            screens: monitors.clone(),
            space: vec2((right - left) as f32, (bottom - top) as f32),
            monitors,
            home,
            opened: None,
        })
    }

    /// `img` opened for editing on the monitor under the mouse pointer.
    fn open(img: RgbaImage, opened: Opened) -> Self {
        use windows_sys::Win32::Graphics::Gdi::{GetMonitorInfoW, MonitorFromPoint, MONITORINFO, MONITOR_DEFAULTTONEAREST};
        let mut cursor = POINT { x: 0, y: 0 };
        let mut info: MONITORINFO = unsafe { std::mem::zeroed() };
        info.cbSize = size_of::<MONITORINFO>() as u32;
        unsafe {
            GetCursorPos(&mut cursor);
            GetMonitorInfoW(MonitorFromPoint(cursor, MONITOR_DEFAULTTONEAREST), &mut info);
        }
        Self::open_on(img, opened, info.rcMonitor)
    }

    /// `img` opened for editing on the monitor at `monitor` (screen pixels).
    fn open_on(img: RgbaImage, opened: Opened, monitor: RECT) -> Self {
        let screen = vec2((monitor.right - monitor.left) as f32, (monitor.bottom - monitor.top) as f32);
        let size = vec2(img.width() as f32, img.height() as f32);
        // A window a little larger than the image when it fits, with room for the
        // toolbar, the strip and the tool box; otherwise the whole monitor.
        let dpi = unsafe { GetDpiForSystem() } as f32 / 96.0;
        let want = (size + vec2(120.0, 220.0) * dpi).max(vec2(960.0, 640.0) * dpi).ceil();
        let (bounds, space, image) = if want.x <= screen.x && want.y <= screen.y {
            let left = monitor.left + ((screen.x - want.x) / 2.0) as i32;
            let top = monitor.top + ((screen.y - want.y) / 2.0) as i32;
            let bounds = RECT { left, top, right: left + want.x as i32, bottom: top + want.y as i32 };
            let at = ((want - size) / 2.0).floor().to_pos2();
            (bounds, want, Rect::from_min_size(at, size))
        } else {
            let (space, image) = place_image(size, screen);
            (monitor, space, image)
        };
        Self {
            shots: vec![img],
            bounds,
            monitors: vec![image],
            home: 0,
            // No windows to snap to: they are not in the picture.
            snap: Snap::default(),
            screens: vec![Rect::from_min_size(Pos2::ZERO, space)],
            space,
            opened: Some(opened),
        }
    }

    /// For tests: a capture of these monitors.
    #[cfg(test)]
    fn new(shots: Vec<RgbaImage>, bounds: RECT, monitors: Vec<Rect>, home: usize, snap: Snap) -> Self {
        let space = vec2((bounds.right - bounds.left) as f32, (bounds.bottom - bounds.top) as f32);
        Self { shots, bounds, screens: monitors.clone(), monitors, home, snap, space, opened: None }
    }

    /// What Ctrl+A selects with the pointer on screen `i`: that monitor, or the whole
    /// opened image.
    fn whole(&self, i: usize) -> Rect {
        match self.opened {
            Some(_) => self.monitors[0],
            None => self.screens[i.min(self.screens.len() - 1)],
        }
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
// WM_MOUSEMOVE is matched on its own first, then the rest of the mouse range.
#[allow(clippy::match_overlapping_arm)]
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

#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
enum Tool {
    /// Adjust the capture region and select, move or resize existing annotations.
    Select,
    /// Drag to draw a new annotation of this kind.
    Shape(ShapeKind),
}

/// The tool a toolbar icon stands for; `None` for the eyedropper, which is not a tool
/// you draw with but picks a colour.
fn tool_for(icon: Icon) -> Option<Tool> {
    Some(match icon {
        Icon::Select => Tool::Select,
        Icon::Rect => Tool::Shape(ShapeKind::Rect),
        Icon::Ellipse => Tool::Shape(ShapeKind::Ellipse),
        Icon::Line => Tool::Shape(ShapeKind::Line),
        Icon::Arrow => Tool::Shape(ShapeKind::Arrow),
        Icon::Pen => Tool::Shape(ShapeKind::Pen),
        Icon::Marker => Tool::Shape(ShapeKind::Marker),
        Icon::Text => Tool::Shape(ShapeKind::Text),
        Icon::Blur => Tool::Shape(ShapeKind::Blur),
        Icon::Step => Tool::Shape(ShapeKind::Step),
        Icon::Emoji => Tool::Shape(ShapeKind::Emoji),
        _ => return None,
    })
}

/// What the current mouse drag is doing.
#[derive(Clone)]
enum Action {
    /// Create, move or resize the capture region.
    Region(Drag),
    /// Drawing a new annotation (the last one in the list) from `anchor`.
    NewShape { anchor: Pos2 },
    MoveShape { index: usize, start: Pos2, orig: Annotation },
    /// Resizing a rectangle or ellipse by one of its handles.
    ResizeBox { index: usize, drag: Drag },
    /// Dragging one end of a straight line, arrow, pen or marker stroke; `end_b` = the
    /// end (tip) rather than the start.
    MoveEnd { index: usize, end_b: bool },
    /// A click with the text tool on empty space: places a new text when released.
    PlaceText { anchor: Pos2 },
    /// A click inside the text being edited: moves the caret there.
    Caret { at: Pos2 },
    /// Turning the capture by the region's round handle, around `pivot` (the region's
    /// centre). `from` is the pointer's direction from the pivot when the drag began,
    /// `orig` the rotation then.
    Rotate { pivot: Pos2, from: f32, orig: Rotation },
}

/// Default annotation colours: red, green, blue, and the highlighter's yellow (so the
/// highlighter's default shows as a selected swatch). The settings can change them.
#[cfg(test)]
const SWATCHES: [Color32; 4] = crate::settings::DEFAULT_SWATCHES;
const ACCENT: Color32 = Color32::from_rgb(0x6C, 0xB6, 0xFF);
/// Default highlighter colour (classic highlighter yellow) and thickness.
#[cfg(test)]
const MARKER_COLOR: Color32 = crate::settings::DEFAULT_MARKER_COLOR;
const MARKER_WIDTH: f32 = 16.0;
/// Default stroke width for the other drawing tools.
const DEFAULT_WIDTH: f32 = 4.0;
/// Width of the toolbar's context options: room for the largest set, the arrow's
/// (stroke style, arrowhead, both-ends toggle), each followed by the toolbar's 2 px
/// item spacing.
const OPTIONS_WIDTH: f32 = icons::PICKER_WIDTH * 2.0 + icons::BUTTON + 3.0 * 2.0;
/// Default text size and blur strength, in pixels.
const TEXT_SIZE: f32 = 24.0;
const BLUR_STRENGTH: f32 = 12.0;
/// Default step marker diameter, in pixels.
const STEP_SIZE: f32 = 28.0;
/// The stamp tool's pickers.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum StampSet {
    Emoji,
    Kaomoji,
    Symbols,
}

impl StampSet {
    fn items(self) -> &'static [&'static str] {
        match self {
            StampSet::Emoji => &crate::emoji::FAVOURITES,
            // Only what this computer's fonts can draw.
            StampSet::Kaomoji => crate::emoji::kaomoji(),
            StampSet::Symbols => crate::emoji::symbols(),
        }
    }

    /// Which picker `stamp` belongs with: its own list's, or else emoji for an emoji,
    /// symbols for a character or two, kaomoji for anything longer.
    fn of(stamp: &str) -> Self {
        let listed = [StampSet::Emoji, StampSet::Kaomoji, StampSet::Symbols].into_iter().find(|s| s.items().contains(&stamp));
        listed.unwrap_or(if crate::emoji::is_emoji(stamp) {
            StampSet::Emoji
        } else if stamp.chars().count() <= 2 {
            StampSet::Symbols
        } else {
            StampSet::Kaomoji
        })
    }
}

/// How long the emoji picker's field waits for the rest of what Windows' emoji panel
/// is sending before using it.
const PANEL_PAUSE: std::time::Duration = std::time::Duration::from_millis(150);

/// Default emoji stamp height, in pixels.
const EMOJI_SIZE: f32 = 48.0;

/// Tools that remember their own colour, separately from the rest (which share one).
/// The highlighter starts yellow. (Every tool remembers its own size: see `widths`.)
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
enum StyleSlot {
    Shared,
    Marker,
    Text,
    Blur,
    Step,
}

fn style_slot(tool: Tool) -> StyleSlot {
    match tool {
        Tool::Shape(ShapeKind::Marker) => StyleSlot::Marker,
        Tool::Shape(ShapeKind::Text) => StyleSlot::Text,
        Tool::Shape(ShapeKind::Blur) => StyleSlot::Blur,
        Tool::Shape(ShapeKind::Step) => StyleSlot::Step,
        _ => StyleSlot::Shared,
    }
}

/// The range of the size control: font size for text, strength for blur, stroke
/// width for the rest.
fn width_range(kind: Option<ShapeKind>) -> std::ops::RangeInclusive<f32> {
    match kind {
        Some(ShapeKind::Text) => 8.0..=120.0,
        Some(ShapeKind::Blur) => 4.0..=60.0,
        Some(ShapeKind::Step) => 16.0..=80.0,
        Some(ShapeKind::Emoji) => 16.0..=256.0,
        _ => MIN_WIDTH..=MAX_WIDTH,
    }
}

/// A text annotation being typed into.
struct TextEdit {
    index: usize,
    /// Caret position, in characters.
    cursor: usize,
    /// The annotation list before editing began, for one undo step for the whole edit.
    before: Vec<Annotation>,
}

/// Byte offset of character `index` in `s`.
fn byte_at(s: &str, index: usize) -> usize {
    s.char_indices().nth(index).map_or(s.len(), |(b, _)| b)
}
/// Freehand points closer than this to the previous one are skipped while drawing.
const MIN_POINT_SPACING: f32 = 1.5;
/// How far the round handle that turns the capture sits from the region, and its
/// radius, in points.
const ROTATE_HANDLE_GAP: f32 = 30.0;
const ROTATE_HANDLE_RADIUS: f32 = 10.0;

/// How the capture is turned on screen, so that what the region holds is straight:
/// desktop point = the captured point turned by `angle` (clockwise on screen, in
/// radians), then moved by `offset`. The whole capture turns, not the region, so the
/// region stays an upright rectangle and everything that reads the capture (copy,
/// save, blur, Copy text, the eyedropper) sees it straightened.
#[derive(Clone, Copy, Debug, PartialEq)]
struct Rotation {
    angle: f32,
    offset: Vec2,
}

impl Rotation {
    /// The capture as it was taken.
    const NONE: Self = Self { angle: 0.0, offset: Vec2::ZERO };

    fn is_none(&self) -> bool {
        *self == Self::NONE
    }

    /// Where captured point `s` shows on the desktop.
    fn to_desktop(self, s: Pos2) -> Pos2 {
        let (sin, cos) = self.angle.sin_cos();
        Pos2::new(cos * s.x - sin * s.y, sin * s.x + cos * s.y) + self.offset
    }

    /// The captured point that shows at desktop point `d`.
    fn to_source(self, d: Pos2) -> Pos2 {
        let v = d - self.offset;
        let (sin, cos) = self.angle.sin_cos();
        Pos2::new(cos * v.x + sin * v.y, -sin * v.x + cos * v.y)
    }

    /// Turned by `delta` more, around desktop point `pivot`. Back at no turn at all,
    /// the capture is back where it was taken.
    fn turned(self, delta: f32, pivot: Pos2) -> Self {
        use std::f32::consts::{PI, TAU};
        let angle = (self.angle + delta + PI).rem_euclid(TAU) - PI;
        if angle.abs() < 1e-4 {
            return Self::NONE;
        }
        let (sin, cos) = delta.sin_cos();
        let v = self.offset - pivot.to_vec2();
        Self { angle, offset: vec2(cos * v.x - sin * v.y, sin * v.x + cos * v.y) + pivot.to_vec2() }
    }

    /// The turn in whole degrees, -179 to 180, for showing.
    fn degrees(&self) -> i32 {
        let d = self.angle.to_degrees().round() as i32;
        if d <= -180 { d + 360 } else { d }
    }
}

/// The horizon and grid shown over the region while the capture is turned, fixed on
/// screen so something in the capture can be lined up against them. The grid is centred
/// on the region, so its middle lines are the horizon and the upright through the centre.
/// Each line has a dark edge so it shows on light and dark captures alike.
fn rotation_guides(painter: &egui::Painter, sel: Rect, guides: &GuideSettings) {
    if !guides.horizon && !guides.grid {
        return;
    }
    let painter = painter.with_clip_rect(sel.intersect(painter.clip_rect()));
    let line = |a: Pos2, b: Pos2, color: Color32, strong: bool| {
        painter.line_segment([a, b], Stroke::new(if strong { 3.0 } else { 2.0 }, Color32::from_black_alpha(if strong { 60 } else { 35 })));
        painter.line_segment([a, b], Stroke::new(if strong { 1.25 } else { 1.0 }, color));
    };
    let faint = Color32::from_white_alpha(45);
    let accent = ACCENT.gamma_multiply(0.6);
    let centre = sel.center();
    if guides.grid {
        let step = guides.spacing.points();
        line(Pos2::new(centre.x, sel.top()), Pos2::new(centre.x, sel.bottom()), accent, true);
        let mut k = step;
        while k <= sel.width() / 2.0 {
            for x in [centre.x - k, centre.x + k] {
                line(Pos2::new(x, sel.top()), Pos2::new(x, sel.bottom()), faint, false);
            }
            k += step;
        }
        let mut k = step;
        while k <= sel.height() / 2.0 {
            for y in [centre.y - k, centre.y + k] {
                line(Pos2::new(sel.left(), y), Pos2::new(sel.right(), y), faint, false);
            }
            k += step;
        }
    }
    // The horizon goes last, over the grid's middle line.
    if guides.horizon || guides.grid {
        let colour = if guides.horizon { accent } else { faint };
        line(Pos2::new(sel.left(), centre.y), Pos2::new(sel.right(), centre.y), colour, guides.horizon);
    }
}

/// A turning arrow: most of a circle of radius `r` round `centre`, clockwise, with an
/// arrowhead at its end.
fn rotate_glyph(painter: &egui::Painter, centre: Pos2, r: f32, color: Color32) {
    let (start, end) = (-60f32.to_radians(), 220f32.to_radians());
    let at = |a: f32| centre + vec2(a.cos(), a.sin()) * r;
    let points: Vec<Pos2> = (0..=24).map(|i| at(start + (end - start) * i as f32 / 24.0)).collect();
    painter.add(egui::Shape::line(points, Stroke::new(1.6, color)));
    // The arrowhead points along the circle, clockwise.
    let tip = at(end);
    let along = vec2(-end.sin(), end.cos());
    let out = vec2(end.cos(), end.sin());
    let size = r * 0.75;
    let head = vec![tip + along * size * 0.6, tip - along * size * 0.2 + out * size * 0.55, tip - along * size * 0.2 - out * size * 0.55];
    painter.add(egui::Shape::convex_polygon(head, color, Stroke::NONE));
}

/// The direction of `v`, in radians, clockwise from pointing right.
fn angle_of(v: Vec2) -> f32 {
    v.y.atan2(v.x)
}

/// One monitor's capture as a quad with its corners placed by `place` (captured
/// pixels to screen points), for drawing it turned.
fn turned_quad(texture: egui::TextureId, monitor: Rect, place: impl Fn(Pos2) -> Pos2) -> egui::Mesh {
    let mut mesh = egui::Mesh::with_texture(texture);
    let corners = [monitor.left_top(), monitor.right_top(), monitor.right_bottom(), monitor.left_bottom()];
    let uvs = [Pos2::new(0.0, 0.0), Pos2::new(1.0, 0.0), Pos2::new(1.0, 1.0), Pos2::new(0.0, 1.0)];
    for (corner, uv) in corners.into_iter().zip(uvs) {
        mesh.vertices.push(egui::epaint::Vertex { pos: place(corner), uv, color: Color32::WHITE });
    }
    mesh.add_triangle(0, 1, 2);
    mesh.add_triangle(0, 2, 3);
    mesh
}

/// The colour at continuous point `p` of `img` (pixel centres at .5), blended from the
/// four pixels around it; edges extend outward.
fn bilinear(img: &RgbaImage, p: Pos2) -> image::Rgba<u8> {
    let (x, y) = (p.x - 0.5, p.y - 0.5);
    let (x0, y0) = (x.floor(), y.floor());
    let (fx, fy) = (x - x0, y - y0);
    let (w, h) = (img.width() as i64 - 1, img.height() as i64 - 1);
    let at = |x: f32, y: f32| img.get_pixel((x as i64).clamp(0, w) as u32, (y as i64).clamp(0, h) as u32).0;
    let (a, b, c, d) = (at(x0, y0), at(x0 + 1.0, y0), at(x0, y0 + 1.0), at(x0 + 1.0, y0 + 1.0));
    let mut out = [0u8; 4];
    for i in 0..4 {
        let top = f32::from(a[i]) * (1.0 - fx) + f32::from(b[i]) * fx;
        let bottom = f32::from(c[i]) * (1.0 - fx) + f32::from(d[i]) * fx;
        out[i] = (top * (1.0 - fy) + bottom * fy).round() as u8;
    }
    image::Rgba(out)
}

struct Overlay {
    desktop: Desktop,
    /// One texture per monitor, so no texture exceeds the GPU's size limit. Created on
    /// the first frame: the real limit is not known before then.
    textures: Vec<egui::TextureHandle>,
    /// The same, smoothly sampled, for drawing the capture turned. Made when first needed.
    smooth_textures: Vec<egui::TextureHandle>,
    /// How the capture is turned under the region (see `Rotation`).
    rotation: Rotation,
    hwnd: Option<HWND>,
    /// The monitor the pointer was last on; Ctrl+A and the hint bar use it.
    current_monitor: usize,
    /// The monitor popups and tooltips are kept on (see `raw_input_hook`).
    ui_monitor: usize,
    /// The capture region in desktop pixels.
    selection: Option<Rect>,
    action: Option<Action>,
    /// The annotation list as it was when the current drag began. Recorded in the undo
    /// history only if the drag actually changes something, so a plain click adds nothing.
    pending_undo: Option<Vec<Annotation>>,

    /// Modifier keys held this frame.
    mods: Modifiers,
    /// Whether the current region drag has moved far enough to count as a drag (a
    /// click with Ctrl selects the window under it instead).
    drag_moved: bool,

    tool: Tool,
    /// The tools in the toolbar, in order: those the settings leave switched on.
    /// The tools switched on in the settings, with the key that picks each.
    tools: Vec<(&'static ToolInfo, Key)>,
    /// The toolbar's colour swatches, from the settings.
    swatches: [Color32; 4],
    /// Style for new annotations; mirrors the selected one, if any.
    color: Color32,
    width: f32,
    dash: Dash,
    head: ArrowHead,
    double_head: bool,
    text_style: TextStyle,
    effect: Effect,
    /// The emoji the stamp tool places.
    emoji: String,
    /// What is typed (or put by Windows' emoji panel) into the emoji picker's field.
    emoji_field: String,
    /// The emoji picker's tab: 0 for recent emoji, else a group in `emoji::GROUPS` + 1.
    emoji_tab: usize,
    /// The emoji groups shown so far, read when first opened.
    emoji_groups: HashMap<usize, Vec<&'static str>>,
    /// The last search and what it found.
    emoji_search: Option<(String, Vec<&'static str>)>,
    /// The emoji stamped most recently, newest first: the picker's first tab. Starts
    /// as the favourites; each new one pushes the oldest out.
    recent_emoji: Vec<String>,
    /// Whether `recent_emoji` is saved (not in tests).
    persist_recent: bool,
    /// When Windows' emoji panel last put text in the field, while waiting for the
    /// rest of a kaomoji, which can arrive a character at a time.
    emoji_pending: Option<std::time::Instant>,
    /// The emoji shown in the toolbar and picker, drawn once per emoji and size.
    emoji_icons: HashMap<(String, [u32; 2], bool), egui::TextureHandle>,
    /// The stamp shown under the pointer before it is placed, for its emoji, size and
    /// colour, and the area it covers relative to the pointer.
    emoji_preview: Option<((String, u32, Color32), egui::TextureHandle, Rect)>,
    /// Colour and size remembered per style slot, swapped in and out with `color` and
    /// `width` when switching tools.
    styles: HashMap<StyleSlot, Color32>,
    /// The size each tool was last used at, so a text's 24 px does not carry over to
    /// the arrow drawn after it.
    widths: HashMap<Tool, f32>,
    editing: Option<TextEdit>,
    /// The eyedropper is active: the next click picks a colour from the capture.
    picking_color: bool,
    /// The press that picked a colour; its drag is ignored.
    ignore_drag: bool,
    /// The save panel: open or not, and its settings.
    save_open: bool,
    save_options: SaveOptions,
    /// The border added around the image when it is copied or saved, if switched on.
    border_on: bool,
    border: effects::Border,
    /// Switching the border on takes the toolbar's colour and line style, the first
    /// time (when the settings did not already give this capture a border).
    border_unset: bool,
    /// A drop shadow is added around the image when it is copied or saved.
    shadow: bool,
    /// The decoration panel (border and shadow) is open.
    decor_open: bool,
    /// The panel showing text or QR codes read from the region, while it is open.
    reading: Option<Reading>,
    /// The region was moved, resized or turned with the reading panel open: read it
    /// again on the next frame.
    reread: bool,
    /// Ask before closing without copying or saving when there are annotations.
    confirm_discard: bool,
    /// That question is on screen.
    confirm_open: bool,
    /// How much the box beside the selection explains about the current tool.
    help_level: HelpLevel,
    /// Horizon and grid shown over the region while it is being turned.
    guides: GuideSettings,
    /// The shortcut sheet (F1) is on screen.
    help_open: bool,
    /// The tool box's size last frame, to place it this frame.
    help_box_size: Vec2,
    /// Where the hotkey strip was drawn last frame, for the tool box to keep clear of.
    strip_rect: Option<Rect>,
    /// The same, in desktop pixels, for the round handle to keep clear of.
    strip_px: Option<Rect>,
    /// Where the toolbar was drawn last frame, in desktop pixels, for the same.
    toolbar_px: Option<Rect>,
    /// How far an opened image is zoomed in from fitting the screen (1 = fit), and how
    /// far it is panned, in points.
    zoom: f32,
    pan: Vec2,
    /// Space or the middle button is held on an opened image: dragging pans.
    panning: bool,
    /// Started from the tray with Reopen last capture on: hide when done, keeping the
    /// capture, for this many minutes.
    handoff: bool,
    /// Until when the window takes the keyboard back if something else has it. A
    /// second click on the tray icon while the overlay opens gives it to the taskbar,
    /// and then Esc would not reach the overlay.
    claim_focus_until: Option<std::time::Instant>,
    annotations: Vec<Annotation>,
    selected: Option<usize>,
    history: History,
    /// Every point of the pen or marker stroke being drawn, kept even while Shift shows
    /// it as a straight line, so releasing Shift returns to the freehand path.
    stroke_points: Vec<Pos2>,
    /// Finished freehand strokes rasterised for display, so they look on screen exactly
    /// as in the output (egui would darken the translucent marker where it overlaps
    /// itself). Keyed by the stroke's content; entries not drawn in a frame are dropped.
    layers: HashMap<u64, (egui::TextureHandle, Rect)>,
    /// Blur regions as displayed, keyed by their area and settings.
    blur_layers: HashMap<u64, (egui::TextureHandle, Rect)>,

    error: Option<String>,
}

/// Text or QR codes read from the region, shown in a panel for copying.
struct Reading {
    kind: read::Kind,
    /// Where the image that was read sits on the desktop: its top-left pixel.
    origin: Pos2,
    /// While reading runs in the background: where its outcome arrives.
    pending: Option<std::sync::mpsc::Receiver<read::Outcome>>,
    /// The lines found, or why there are none.
    outcome: Option<read::Outcome>,
    /// The words picked by clicking, as (line, word). None picked means all of them.
    picked: HashSet<(usize, usize)>,
    /// The text to copy: the picked words, or everything. Editable in the panel, and
    /// written afresh when the picks change.
    text: String,
}

impl Reading {
    /// The lines found, if reading is done and found some.
    fn lines(&self) -> &[read::Line] {
        match &self.outcome {
            Some(Ok(lines)) => lines,
            _ => &[],
        }
    }

    /// The line, and the word in it, at desktop pixel `p`.
    fn hit(&self, p: Pos2) -> Option<(usize, usize)> {
        let p = (p - self.origin).to_pos2();
        self.lines().iter().enumerate().find_map(|(i, line)| {
            // A little slack round each word, so the gaps between them count too.
            let slack = (line.quad.height() * 0.25).max(2.0);
            let j = line.words.iter().position(|w| w.quad.expand(slack * 1.6, slack).contains(p))?;
            Some((i, j))
        })
    }

    /// Click on a line: pick all of it, or let all of it go if it was all picked.
    /// With `word_only`, pick or let go of just the word.
    fn toggle(&mut self, (line, word): (usize, usize), word_only: bool) {
        let count = self.lines()[line].words.len();
        if word_only {
            if !self.picked.remove(&(line, word)) {
                self.picked.insert((line, word));
            }
        } else if (0..count).all(|j| self.picked.contains(&(line, j))) {
            for j in 0..count {
                self.picked.remove(&(line, j));
            }
        } else {
            self.picked.extend((0..count).map(|j| (line, j)));
        }
        self.text = self.picked_text();
    }

    /// The text of the picked words: whole lines as read, partly picked lines as their
    /// picked words, one line under another. Everything if nothing is picked.
    fn picked_text(&self) -> String {
        let lines = self.lines();
        if self.picked.is_empty() {
            return read::joined(lines);
        }
        let mut out = Vec::new();
        for (i, line) in lines.iter().enumerate() {
            let words: Vec<&str> =
                (0..line.words.len()).filter(|&j| self.picked.contains(&(i, j))).map(|j| line.words[j].text.as_str()).collect();
            if words.len() == line.words.len() {
                out.push(line.text.clone());
            } else if !words.is_empty() {
                out.push(words.join(" "));
            }
        }
        out.join("\n")
    }
}

/// Whether reading has finished and found something.
fn found_any(reading: &Reading) -> bool {
    !reading.lines().is_empty()
}

/// What the toolbar asked for this frame.
#[derive(Default)]
struct ToolbarOutput {
    /// Where the toolbar was drawn, in points.
    rect: Option<Rect>,
    decor: bool,
    read: Option<read::Kind>,
    copy: bool,
    save: bool,
    close: bool,
}

impl Overlay {
    fn with_desktop(desktop: Desktop, hwnd: Option<HWND>, settings: &Settings) -> Self {
        let swatches = settings.swatch_colors();
        // An opened image starts selected, and saves next to itself as "name-edited".
        let mut save_options = SaveOptions::from_settings(settings);
        let selection = desktop.opened.as_ref().map(|_| desktop.monitors[0]);
        if let Some(opened) = &desktop.opened {
            if let Some(stem) = &opened.stem {
                save_options.name = format!("{stem}-edited");
            }
            save_options.folder = opened.folder.clone().or(save_options.folder);
            save_options.format = opened.format.unwrap_or(save_options.format);
        }
        Self {
            swatches,
            mods: Modifiers::NONE,
            drag_moved: false,
            current_monitor: desktop.home,
            ui_monitor: desktop.home,
            desktop,
            textures: Vec::new(),
            smooth_textures: Vec::new(),
            rotation: Rotation::NONE,
            hwnd,
            selection,
            action: None,
            pending_undo: None,
            tool: Tool::Select,
            tools: TOOLS
                .iter()
                // Emoji stamps need Windows' emoji font.
                .filter(|t| settings.tool_enabled(t.id) && (t.icon != Icon::Emoji || crate::emoji::available()))
                .map(|t| (t, settings.tool_key(t)))
                .collect(),
            color: swatches[0],
            width: 4.0,
            dash: Dash::Solid,
            head: ArrowHead::Triangle,
            double_head: false,
            text_style: TextStyle::Plain,
            effect: Effect::Pixelate,
            emoji: crate::emoji::DEFAULT.to_owned(),
            emoji_field: String::new(),
            emoji_pending: None,
            emoji_tab: 0,
            emoji_groups: HashMap::new(),
            emoji_search: None,
            recent_emoji: crate::emoji::FAVOURITES.iter().map(|e| (*e).to_owned()).collect(),
            persist_recent: false,
            emoji_icons: HashMap::new(),
            emoji_preview: None,
            styles: HashMap::from([
                (StyleSlot::Marker, settings.marker()),
                (StyleSlot::Text, swatches[0]),
                // Black: the colour of a redaction (pixelate and blur ignore it).
                (StyleSlot::Blur, Color32::BLACK),
                (StyleSlot::Step, swatches[0]),
            ]),
            widths: HashMap::from([
                (Tool::Shape(ShapeKind::Marker), MARKER_WIDTH),
                (Tool::Shape(ShapeKind::Text), TEXT_SIZE),
                (Tool::Shape(ShapeKind::Blur), BLUR_STRENGTH),
                (Tool::Shape(ShapeKind::Step), STEP_SIZE),
                (Tool::Shape(ShapeKind::Emoji), EMOJI_SIZE),
            ]),
            editing: None,
            picking_color: false,
            ignore_drag: false,
            save_open: false,
            save_options,
            border_on: settings.frame.border,
            border: settings.border(),
            border_unset: !settings.frame.border,
            shadow: settings.frame.shadow,
            decor_open: false,
            reading: None,
            reread: false,
            confirm_discard: settings.confirm_discard,
            confirm_open: false,
            help_level: settings.help_level,
            guides: settings.guides.clone(),
            help_open: false,
            help_box_size: vec2(help::BOX_WIDTH, 200.0),
            strip_rect: None,
            strip_px: None,
            toolbar_px: None,
            zoom: 1.0,
            pan: Vec2::ZERO,
            panning: false,
            handoff: false,
            claim_focus_until: None,
            annotations: Vec::new(),
            selected: None,
            history: History::default(),
            stroke_points: Vec::new(),
            layers: HashMap::new(),
            blur_layers: HashMap::new(),
            error: None,
        }
    }

    fn new(cc: &eframe::CreationContext<'_>, desktop: Desktop, settings: &Settings) -> Self {
        trace("window created");
        setup_text(&cc.egui_ctx);
        cc.egui_ctx.set_theme(settings.theme);
        icons::set_marker_color(&cc.egui_ctx, settings.marker());
        let mut overlay = Self::with_desktop(desktop, hwnd_of(cc), settings);
        overlay.claim_focus_until = Some(std::time::Instant::now() + std::time::Duration::from_millis(1500));
        if let Some(recent) = crate::settings::load_recent() {
            overlay.recent_emoji = recent;
        }
        overlay.persist_recent = true;
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

    /// Take the keyboard back while the overlay is opening, if something else took it.
    fn claim_focus(&mut self, ctx: &egui::Context) {
        use windows_sys::Win32::UI::WindowsAndMessaging::GetForegroundWindow;
        let (Some(until), Some(hwnd)) = (self.claim_focus_until, self.hwnd) else { return };
        if std::time::Instant::now() >= until {
            self.claim_focus_until = None;
            return;
        }
        unsafe {
            if GetForegroundWindow() != hwnd {
                SetForegroundWindow(hwnd);
            }
        }
        ctx.request_repaint_after(std::time::Duration::from_millis(50));
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

    /// Done: close. Started from the tray with Reopen last capture on, first hand the
    /// tray what it takes to bring this back (see `handoff`).
    fn leave(&mut self, ctx: &egui::Context) {
        if self.handoff {
            // Gone from the screen at once; the capture is packed up after.
            if let Some(hwnd) = self.hwnd {
                use windows_sys::Win32::UI::WindowsAndMessaging::{ShowWindow, SW_HIDE};
                unsafe { ShowWindow(hwnd, SW_HIDE) };
            }
            // As it was, without anything half done.
            self.finish_editing();
            self.action = None;
            let bytes = self.kept_bytes();
            let mut out = std::io::stdout().lock();
            let _ = out.write_all(&bytes).and_then(|()| out.flush());
        }
        ctx.send_viewport_cmd(egui::ViewportCommand::Close);
    }

    /// The capture, region, drawings, undo history, and the tool and its style, as
    /// bytes for the tray to keep (see `handoff`).
    fn kept_bytes(&self) -> Vec<u8> {
        use crate::handoff::Writer;
        let mut w = Writer::default();
        w.0.extend_from_slice(crate::handoff::MAGIC);
        let d = &self.desktop;
        // Each screen as a quickly compressed PNG: a screenshot shrinks several times
        // over, and the tray holds less for as long as it keeps it.
        w.list(&d.shots, |w, shot| {
            use image::codecs::png::{CompressionType, FilterType, PngEncoder};
            let mut png = Vec::new();
            let encoder = PngEncoder::new_with_quality(&mut png, CompressionType::Fast, FilterType::Adaptive);
            let _ = shot.write_with_encoder(encoder);
            w.bytes(&png);
        });
        for v in [d.bounds.left, d.bounds.top, d.bounds.right, d.bounds.bottom] {
            w.i32(v);
        }
        w.list(&d.monitors, |w, r| w.rect(*r));
        w.len(d.home);
        w.list(&d.snap.windows, |w, r| w.rect(*r));
        w.list(&d.screens, |w, r| w.rect(*r));
        w.vec2(d.space);
        w.option(d.opened.as_ref(), |w, o| {
            w.option(o.stem.as_deref(), |w, s| w.str(s));
            w.option(o.folder.as_deref(), |w, f| w.str(&f.to_string_lossy()));
            w.option(o.format, |w, f| w.choice(&f, &[Format::Png, Format::Jpeg]));
        });

        w.len(self.current_monitor);
        w.option(self.selection, |w, r| w.rect(r));
        w.f32(self.rotation.angle);
        w.vec2(self.rotation.offset);
        w.choice(&self.tool, &all_tools());
        w.color(self.color);
        w.f32(self.width);
        w.choice(&self.dash, &Dash::ALL.map(|d| d.0));
        w.choice(&self.head, &ArrowHead::ALL.map(|h| h.0));
        w.bool(self.double_head);
        w.choice(&self.text_style, &TextStyle::ALL.map(|t| t.0));
        w.choice(&self.effect, &Effect::ALL.map(|e| e.0));
        w.str(&self.emoji);
        // In a fixed order, so the same state always gives the same bytes.
        let styles: Vec<(StyleSlot, Color32)> = STYLE_SLOTS.iter().filter_map(|k| Some((*k, *self.styles.get(k)?))).collect();
        w.list(&styles, |w, (slot, c)| {
            w.choice(slot, &STYLE_SLOTS);
            w.color(*c);
        });
        let widths: Vec<(Tool, f32)> = all_tools().into_iter().filter_map(|k| Some((k, *self.widths.get(&k)?))).collect();
        w.list(&widths, |w, (tool, width)| {
            w.choice(tool, &all_tools());
            w.f32(*width);
        });
        w.bool(self.border_on);
        w.color(self.border.color);
        w.f32(self.border.width);
        w.choice(&self.border.dash, &Dash::ALL.map(|d| d.0));
        w.bool(self.border_unset);
        w.bool(self.shadow);
        w.f32(self.zoom);
        w.vec2(self.pan);
        w.annotations(&self.annotations);
        let (undo, redo) = self.history.steps();
        w.list(undo, |w, s| w.annotations(s));
        w.list(redo, |w, s| w.annotations(s));
        w.0
    }

    /// Put back what [`Overlay::kept_bytes`] kept, besides the desktop.
    fn restore(&mut self, kept: Kept) {
        self.current_monitor = kept.current_monitor.min(self.desktop.monitors.len().saturating_sub(1));
        self.selection = kept.selection;
        self.rotation = kept.rotation;
        self.tool = kept.tool;
        self.color = kept.color;
        self.width = kept.width;
        self.dash = kept.dash;
        self.head = kept.head;
        self.double_head = kept.double_head;
        self.text_style = kept.text_style;
        self.effect = kept.effect;
        self.emoji = kept.emoji;
        self.styles = kept.styles.into_iter().collect();
        self.widths = kept.widths.into_iter().collect();
        self.border_on = kept.border_on;
        self.border = kept.border;
        self.border_unset = kept.border_unset;
        self.shadow = kept.shadow;
        self.zoom = kept.zoom;
        self.pan = kept.pan;
        self.annotations = kept.annotations;
        self.history = kept.history;
    }

    /// Ctrl+wheel zooms an opened image around the pointer, Ctrl+0 fits it again, and
    /// dragging with the middle button or with Space held pans it.
    fn zoom_and_pan(&mut self, ctx: &egui::Context, screen: Rect, fit_scale: f32) {
        let typing = ctx.egui_wants_keyboard_input() || self.editing.is_some();
        let (ticks, pointer, middle, primary, space, delta, fit) = ctx.input(|i| {
            let ticks: f32 = i
                .events
                .iter()
                .map(|e| match e {
                    egui::Event::MouseWheel { delta, modifiers, .. } if modifiers.command => delta.y.signum(),
                    _ => 0.0,
                })
                .sum();
            (
                ticks,
                i.pointer.hover_pos(),
                i.pointer.middle_down(),
                i.pointer.primary_down(),
                i.key_down(Key::Space),
                i.pointer.delta(),
                i.modifiers.command && i.key_pressed(Key::Num0),
            )
        });
        self.panning = middle || space && !typing;
        if fit {
            self.zoom = 1.0;
            self.pan = Vec2::ZERO;
        }
        if ticks != 0.0 {
            // Up to 16 points per image pixel.
            let max = (16.0 * fit_scale).max(1.0);
            let old = self.zoom;
            let new = (old * 1.25f32.powf(ticks)).clamp(1.0, max);
            // Keep the point under the pointer where it is.
            let p = pointer.unwrap_or(screen.center()) - screen.min;
            self.pan = p - (p - self.pan) * (new / old);
            self.zoom = new;
        }
        if self.panning && (middle || primary) {
            self.pan += delta;
        }
        if self.panning {
            ctx.set_cursor_icon(if middle || primary { egui::CursorIcon::Grabbing } else { egui::CursorIcon::Grab });
        }
        self.pan = self.pan.clamp(screen.size() * (1.0 - self.zoom), Vec2::ZERO);
    }

    fn bounds(&self) -> Rect {
        Rect::from_min_size(Pos2::ZERO, self.desktop.space)
    }

    /// Where the round handle that turns the capture is, in desktop pixels: above the
    /// region's top edge, or else the first of below, right, left, and just inside the
    /// top or bottom edge that is on screen and clear of the hotkey strip and toolbar. `tol`
    /// is the grab tolerance in desktop pixels, which also gives the screen's scale.
    ///
    /// Also once things are drawn, to straighten it a little more: annotations stay
    /// where they are on the region, and the capture turns under them. Blur regions
    /// blur whatever is under them after the turn, recomputed live while turning, so
    /// what is seen hidden is what the output hides.
    fn rotate_handle(&self, sel: Rect, tol: f32) -> Option<Pos2> {
        if sel.width() < 8.0 || sel.height() < 8.0 {
            return None;
        }
        let gap = ROTATE_HANDLE_GAP * tol / GRAB_TOLERANCE;
        let reach = (ROTATE_HANDLE_RADIUS + 2.0) * tol / GRAB_TOLERANCE;
        // On the region's own monitor: the desktop's bounds also take in other monitors
        // and the gaps between them.
        let screens = &self.desktop.screens;
        let screen = screens.iter().find(|m| m.contains(sel.center())).copied().unwrap_or(screens[self.current_monitor]).shrink(tol);
        let clear = |p: Pos2| screen.contains(p) && ![self.strip_px, self.toolbar_px].iter().flatten().any(|r| r.expand(reach).contains(p));
        let inset = gap.min(sel.height() / 2.0);
        let spots = [
            sel.center_top() - vec2(0.0, gap),
            sel.center_bottom() + vec2(0.0, gap),
            sel.right_center() + vec2(gap, 0.0),
            sel.left_center() - vec2(gap, 0.0),
            // A region filling the screen has no room outside it.
            sel.center_top() + vec2(0.0, inset),
            sel.center_bottom() - vec2(0.0, inset),
        ];
        Some(spots.into_iter().find(|&p| clear(p)).unwrap_or(spots[4]))
    }

    /// Back to the capture as it was taken, reading again if the panel is open.
    fn reset_rotation(&mut self) {
        self.rotation = Rotation::NONE;
        self.blur_layers.clear();
        self.reread = self.reading.is_some();
    }

    // --- Mouse interaction -----------------------------------------------------------

    /// What a drag starting at `p` would do. Pure, so it also drives the cursor shape.
    /// Priority: handles of the selected annotation, then the region's edges, then
    /// whatever the active tool does at that point.
    fn pick_action(&self, p: Pos2, tol: f32) -> Action {
        let Some(sel) = self.selection else {
            return Action::Region(Drag::Create { anchor: p });
        };

        // Ctrl: whatever the tool, grab and move things as the Select tool does.
        let grab = self.tool == Tool::Select || self.mods.ctrl;
        if let Some(edit) = self.editing.as_ref().filter(|_| !grab)
            && self.annotations[edit.index].hit(p, tol) {
                return Action::Caret { at: p };
            }

        if let Some(index) = self.selected {
            let shape = &self.annotations[index];
            if shape.is_box() {
                if let Some((hx, hy)) = handle_at(shape.rect(), p, tol) {
                    let drag = Drag::Resize { start: p, orig: shape.rect(), hx, hy };
                    return Action::ResizeBox { index, drag };
                }
            } else if shape.is_freehand() || shape.kind == ShapeKind::Text {
                // Freehand strokes and text can be moved, but have no handles.
            } else if p.distance(shape.b) <= tol {
                return Action::MoveEnd { index, end_b: true };
            } else if p.distance(shape.a) <= tol {
                return Action::MoveEnd { index, end_b: false };
            }
        }

        if let Some(handle) = self.rotate_handle(sel, tol)
            && p.distance(handle) <= (ROTATE_HANDLE_RADIUS + 2.0) * tol / GRAB_TOLERANCE {
                let pivot = sel.center();
                return Action::Rotate { pivot, from: angle_of(p - pivot), orig: self.rotation };
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
            orig: self.annotations[index].clone(),
        };
        match self.tool {
            // Topmost annotation under the pointer, else the region itself.
            _ if grab => match self.annotations.iter().rposition(|a| a.hit(p, tol)) {
                Some(index) => move_shape(index),
                None => Action::Region(Drag::Move { start: p, orig: sel }),
            },
            // The text tool edits a text it is clicked on (or moves it when dragged).
            Tool::Shape(ShapeKind::Text) => {
                let on_text = |a: &Annotation| a.kind == ShapeKind::Text && a.hit(p, tol);
                match self.annotations.iter().rposition(on_text) {
                    Some(index) => move_shape(index),
                    None => Action::PlaceText { anchor: p },
                }
            }
            // While drawing, only the selected annotation can be grabbed, so new
            // shapes can start on top of existing ones.
            Tool::Shape(_) | Tool::Select => match self.selected {
                Some(index) if self.annotations[index].hit(p, tol) => move_shape(index),
                _ => Action::NewShape { anchor: p },
            },
        }
    }

    fn begin_action(&mut self, mut action: Action) {
        self.drag_moved = false;
        if !matches!(action, Action::Caret { .. }) {
            // An empty text being typed into is dropped when editing ends: an
            // annotation after it moves down one place.
            let (edited, count) = (self.editing.as_ref().map(|e| e.index), self.annotations.len());
            self.finish_editing();
            if let Some(edited) = edited.filter(|_| self.annotations.len() < count) {
                match &mut action {
                    Action::MoveShape { index, .. }
                    | Action::ResizeBox { index, .. }
                    | Action::MoveEnd { index, .. } => {
                        if *index == edited {
                            self.action = None;
                            return;
                        }
                        if *index > edited {
                            *index -= 1;
                        }
                    }
                    _ => {}
                }
            }
        }
        match action {
            Action::Caret { at } => {
                if let Some(edit) = &mut self.editing {
                    let shape = &self.annotations[edit.index];
                    edit.cursor = Layout::new(&shape.text, shape.width).index_at(at - shape.a.to_vec2());
                }
            }
            Action::PlaceText { .. } => self.selected = None,
            Action::Region(Drag::Create { .. } | Drag::Move { .. }) => self.selected = None,
            Action::Region(Drag::Resize { .. }) => {}
            Action::Rotate { .. } => self.selected = None,
            Action::NewShape { anchor } => {
                let Tool::Shape(kind) = self.tool else { return };
                self.history.record(&self.annotations, None);
                self.annotations.push(Annotation {
                    kind,
                    a: anchor,
                    b: anchor,
                    color: self.color,
                    width: self.width,
                    dash: self.dash,
                    head: self.head,
                    double_head: self.double_head,
                    points: Vec::new(),
                    text: String::new(),
                    text_style: self.text_style,
                    effect: self.effect,
                });
                self.stroke_points = vec![anchor];
                if kind == ShapeKind::Step {
                    let next = self.next_step_number();
                    if let Some(step) = self.annotations.last_mut() {
                        step.text = next.to_string();
                    }
                } else if kind == ShapeKind::Emoji {
                    if let Some(stamp) = self.annotations.last_mut() {
                        stamp.text = self.emoji.clone();
                    }
                    self.note_recent();
                }
                self.selected = Some(self.annotations.len() - 1);
            }
            // Moved with Ctrl while drawing: a shape of another kind is moved without
            // being selected, so its colour and size do not become the tool's.
            Action::MoveShape { index, .. }
                if matches!(self.tool, Tool::Shape(kind) if kind != self.annotations[index].kind) =>
            {
                self.selected = None;
                self.pending_undo = Some(self.annotations.clone());
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
        let Some(action) = self.action.clone() else { return };
        match action {
            Action::PlaceText { .. } | Action::Caret { .. } => {}
            Action::Rotate { pivot, from, orig } => {
                if p.distance(pivot) < 1.0 {
                    return;
                }
                // Follows the pointer freely; Shift turns in 15-degree steps.
                let mut angle = orig.angle + angle_of(p - pivot) - from;
                if mods.shift {
                    let step = 15f32.to_radians();
                    angle = (angle / step).round() * step;
                }
                self.rotation = orig.turned(angle - orig.angle, pivot);
                self.blur_layers.clear();
            }
            Action::Region(drag) => {
                // A stray click outside the region must not replace it with nothing.
                if let Drag::Create { anchor } = drag
                    && anchor.distance(p) < 3.0 && !self.drag_moved {
                        return;
                    }
                self.drag_moved = true;
                let mut region = drag.apply(p, mods, bounds);
                // Ctrl: pull the edges being dragged onto nearby window or monitor edges
                // (where they were captured: not once the capture is turned).
                if mods.ctrl && self.rotation.is_none() {
                    let snap = &self.desktop.snap;
                    region = match drag {
                        Drag::Create { .. } => snap.edges(region, true, true, true, true),
                        Drag::Move { .. } => keep_inside(snap.moved(region), bounds),
                        Drag::Resize { hx, hy, .. } => {
                            let (x, y) = (hx != 0 && mods.alt, hy != 0 && mods.alt);
                            snap.edges(region, hx < 0 || x, hx > 0 || x, hy < 0 || y, hy > 0 || y)
                        }
                    };
                    region = region.intersect(bounds);
                }
                self.selection = Some(region);
            }
            Action::NewShape { anchor } => {
                let Some(shape) = self.annotations.last_mut() else { return };
                if matches!(shape.kind, ShapeKind::Step | ShapeKind::Emoji) {
                    (shape.a, shape.b) = (p, p);
                } else if shape.is_box() {
                    shape.set_rect(Drag::Create { anchor }.apply(p, mods, bounds));
                } else if matches!(shape.kind, ShapeKind::Pen | ShapeKind::Marker) {
                    if self.stroke_points.last().is_none_or(|last| last.distance(p) >= MIN_POINT_SPACING) {
                        self.stroke_points.push(p);
                    }
                    // Shift: a straight stroke from the start, in 45-degree steps.
                    // Releasing Shift goes back to the freehand path drawn so far.
                    shape.a = anchor;
                    if mods.shift {
                        shape.points.clear();
                        shape.b = annotate::snap_angle(anchor, p);
                    } else {
                        shape.points.clone_from(&self.stroke_points);
                        shape.b = p;
                    }
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
                let mut shape = self.annotations[index].clone();
                shape.set_rect(drag.apply(p, mods, bounds));
                self.change(index, shape);
            }
            Action::MoveEnd { index, end_b } => {
                let mut shape = self.annotations[index].clone();
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
        // The region now holds something else: what was read is read again.
        let changed = matches!(self.action, Some(Action::Rotate { .. }))
            || (matches!(self.action, Some(Action::Region(_))) && self.drag_moved);
        if changed && self.reading.is_some() {
            self.reread = true;
        }
        // A click (no change) on a text with the text tool starts editing it.
        if let Some(Action::MoveShape { index, start, .. }) = self.action {
            let unchanged = self.pending_undo.is_some();
            let text = self.annotations[index].kind == ShapeKind::Text;
            if unchanged && text && self.tool == Tool::Shape(ShapeKind::Text) {
                self.action = None;
                self.pending_undo = None;
                self.edit_text(index, start);
                return;
            }
        }
        match self.action.take() {
            Some(Action::PlaceText { anchor }) => self.start_text(anchor),
            Some(Action::NewShape { .. }) => {
                self.stroke_points.clear();
                if let Some(shape) = self.annotations.last_mut()
                    && shape.is_freehand() {
                        shape.points = annotate::simplify(&shape.points, 0.4);
                    }
                // A click without a drag leaves no shape behind, and no undo step.
                if self.annotations.last().is_some_and(Annotation::is_degenerate) {
                    self.annotations.pop();
                    self.history.forget_last();
                    self.selected = None;
                }
                // Steps and stamps are placed one after another, often in different
                // colours or emoji: leave a placed one unselected so picking a colour or
                // emoji for the next does not change it. (A stamp is sized before it is
                // placed, from the preview under the pointer.)
                if self.annotations.last().is_some_and(|a| matches!(a.kind, ShapeKind::Step | ShapeKind::Emoji)) {
                    self.selected = None;
                }
            }
            Some(Action::Region(drag)) => {
                // Ctrl+click: select the window under the pointer.
                if let Drag::Create { anchor } = drag
                    && !self.drag_moved && self.mods.ctrl && self.rotation.is_none()
                        && let Some(window) = self.desktop.snap.window_at(anchor) {
                            self.selection = Some(window.intersect(self.bounds()));
                            self.selected = None;
                        }
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

    /// Right-click: close whatever is open, or else let go of the selected annotation,
    /// or else put the tool down (back to Select). One step per click.
    fn back_out(&mut self, ctx: &egui::Context) {
        if egui::Popup::is_any_open(ctx) {
            egui::Popup::close_all(ctx);
        } else if self.editing.is_some() {
            self.finish_editing();
        } else if self.help_open {
            self.help_open = false;
        } else if self.picking_color {
            self.picking_color = false;
        } else if self.save_open || self.decor_open || self.reading.is_some() {
            self.save_open = false;
            self.decor_open = false;
            self.reading = None;
        } else if self.selected.is_some() {
            self.selected = None;
        } else if self.tool != Tool::Select {
            self.set_tool(Tool::Select);
        }
    }

    /// Switch tools. Every tool keeps its own size; the highlighter, text, blur and step
    /// tools also keep their own colour (the highlighter starts yellow), while the
    /// other tools share one.
    fn set_tool(&mut self, tool: Tool) {
        if tool == self.tool {
            return;
        }
        // Let go of the selected shape first, or the style of the new tool (such as
        // its thickness) would be applied to it.
        self.finish_editing();
        self.selected = None;
        let (old, new) = (style_slot(self.tool), style_slot(tool));
        if old != new {
            self.styles.insert(old, self.color);
            if let Some(&color) = self.styles.get(&new) {
                self.color = color;
            }
        }
        // The Select tool's size is that of whatever it last selected: not a size of
        // its own to remember.
        if self.tool != Tool::Select {
            self.widths.insert(self.tool, self.width);
        }
        if tool != Tool::Select {
            self.width = self.widths.get(&tool).copied().unwrap_or(DEFAULT_WIDTH);
        }
        self.tool = tool;
    }

    fn select(&mut self, index: usize) {
        self.selected = Some(index);
        let shape = self.annotations[index].clone();
        if shape.kind == ShapeKind::Emoji {
            self.emoji = shape.text.clone();
        }
        // An emoji stamp has no colour of its own to show; a kaomoji or symbol does.
        if shape.kind != ShapeKind::Emoji || !crate::emoji::is_emoji(&shape.text) {
            self.color = shape.color;
        }
        self.width = shape.width;
        self.dash = shape.dash;
        self.head = shape.head;
        self.double_head = shape.double_head;
        self.text_style = shape.text_style;
        self.effect = shape.effect;
    }

    /// Push the current style onto the selected annotation, if it differs.
    fn restyle_selected(&mut self) {
        let Some(index) = self.selected else { return };
        let mut shape = self.annotations[index].clone();
        shape.color = self.color;
        shape.width = self.width;
        shape.dash = self.dash;
        match shape.kind {
            ShapeKind::Arrow => {
                shape.head = self.head;
                shape.double_head = self.double_head;
            }
            ShapeKind::Text => shape.text_style = self.text_style,
            ShapeKind::Blur => shape.effect = self.effect,
            ShapeKind::Emoji => {
                shape.text = self.emoji.clone();
                if crate::emoji::is_emoji(&shape.text) {
                    shape.color = self.annotations[index].color;
                }
            }
            _ => {}
        }
        if shape != self.annotations[index] {
            self.history.record(&self.annotations, Some(Repeat::Style));
            self.annotations[index] = shape;
        }
    }

    /// The number for a new step marker: one more than the highest so far.
    fn next_step_number(&self) -> u32 {
        let numbers = self.annotations.iter().filter(|a| a.kind == ShapeKind::Step);
        numbers.filter_map(|a| a.text.parse::<u32>().ok()).max().unwrap_or(0) + 1
    }

    /// The captured colour at desktop pixel `p` (undimmed, without annotations).
    fn pixel_at(&self, p: Pos2) -> Option<Color32> {
        let p = self.rotation.to_source(p);
        let (monitor, shot) =
            self.desktop.monitors.iter().zip(&self.desktop.shots).find(|(m, _)| m.contains(p))?;
        let (x, y) = ((p.x - monitor.min.x) as u32, (p.y - monitor.min.y) as u32);
        let [r, g, b, _] = shot.get_pixel_checked(x, y)?.0;
        Some(Color32::from_rgb(r, g, b))
    }

    // --- Text editing ------------------------------------------------------------------

    /// The kind of shape the style controls currently apply to.
    fn style_kind(&self) -> Option<ShapeKind> {
        match (self.selected, self.tool) {
            (Some(i), _) => Some(self.annotations[i].kind),
            (None, Tool::Shape(kind)) => Some(kind),
            (None, Tool::Select) => None,
        }
    }

    /// Place a new, empty text with its first line centred on `at` and start typing.
    fn start_text(&mut self, at: Pos2) {
        let before = self.annotations.clone();
        let line_height = Layout::new("", self.width).line_height;
        let a = (at - vec2(0.0, line_height / 2.0)).round();
        self.annotations.push(Annotation {
            kind: ShapeKind::Text,
            a,
            b: a,
            color: self.color,
            width: self.width,
            dash: Dash::Solid,
            head: ArrowHead::Triangle,
            double_head: false,
            points: Vec::new(),
            text: String::new(),
            text_style: self.text_style,
            effect: self.effect,
        });
        let index = self.annotations.len() - 1;
        self.selected = Some(index);
        self.editing = Some(TextEdit { index, cursor: 0, before });
    }

    /// Start editing text `index` with the caret nearest to `at`.
    fn edit_text(&mut self, index: usize, at: Pos2) {
        self.finish_editing();
        self.select(index);
        let shape = &self.annotations[index];
        let cursor = Layout::new(&shape.text, shape.width).index_at(at - shape.a.to_vec2());
        self.editing = Some(TextEdit { index, cursor, before: self.annotations.clone() });
    }

    /// Stop editing. A text left empty is removed; any change becomes one undo step.
    fn finish_editing(&mut self) {
        let Some(edit) = self.editing.take() else { return };
        if self.annotations[edit.index].is_degenerate() {
            self.annotations.remove(edit.index);
            self.selected = None;
        }
        if self.annotations != edit.before {
            self.history.record(&edit.before, None);
        }
    }

    /// Apply one keyboard event to the text being edited. Returns true if the event
    /// ends editing.
    fn text_event(&mut self, event: &egui::Event) -> bool {
        let Some(edit) = &mut self.editing else { return false };
        let shape = &mut self.annotations[edit.index];
        let insert = |shape: &mut Annotation, edit: &mut TextEdit, s: &str| {
            let at = byte_at(&shape.text, edit.cursor);
            shape.text.insert_str(at, s);
            edit.cursor += s.chars().count();
        };
        match event {
            egui::Event::Text(t) => {
                let t: String = t.chars().filter(|c| !c.is_control()).collect();
                insert(shape, edit, &t);
            }
            egui::Event::Paste(t) => {
                let t = t.replace("\r\n", "\n").replace('\r', "\n");
                let t: String = t.chars().filter(|c| *c == '\n' || !c.is_control()).collect();
                insert(shape, edit, &t);
            }
            egui::Event::Key { key, pressed: true, modifiers, .. } => {
                let layout = Layout::new(&shape.text, shape.width);
                let (row, start, len) = layout.line_of(edit.cursor);
                let count = shape.text.chars().count();
                match key {
                    // As in chat apps: Enter finishes, Shift+Enter starts a new line.
                    // Esc finishes too.
                    Key::Escape => return true,
                    Key::Enter if modifiers.shift => insert(shape, edit, "\n"),
                    Key::Enter => return true,
                    Key::Backspace if edit.cursor > 0 => {
                        let at = byte_at(&shape.text, edit.cursor - 1);
                        shape.text.remove(at);
                        edit.cursor -= 1;
                    }
                    Key::Delete if edit.cursor < count => {
                        let at = byte_at(&shape.text, edit.cursor);
                        shape.text.remove(at);
                    }
                    Key::ArrowLeft => edit.cursor = edit.cursor.saturating_sub(1),
                    Key::ArrowRight => edit.cursor = (edit.cursor + 1).min(count),
                    Key::Home => edit.cursor = start,
                    Key::End => edit.cursor = start + len,
                    Key::ArrowUp | Key::ArrowDown => {
                        let target = if *key == Key::ArrowUp { row.checked_sub(1) } else { Some(row + 1) };
                        if let Some(target) = target.filter(|r| *r < layout.line_count()) {
                            let x = layout.caret(edit.cursor).x;
                            let y = (target as f32 + 0.5) * layout.line_height;
                            edit.cursor = layout.index_at(Pos2::new(x, y));
                        }
                    }
                    _ => {}
                }
            }
            _ => {}
        }
        false
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
        self.crop_desktop(self.selection?)
    }

    /// The capture's pixels inside `area` (desktop pixels), stitched across monitors.
    fn crop_desktop(&self, area: Rect) -> Option<RgbaImage> {
        let sel = area.intersect(self.bounds());
        let sel = Rect::from_min_max(sel.min.round(), sel.max.round());
        if sel.width() < 1.0 || sel.height() < 1.0 {
            return None;
        }
        let (w, h) = (sel.width() as u32, sel.height() as u32);
        if w == 0 || h == 0 {
            return None;
        }
        if !self.rotation.is_none() {
            return Some(self.crop_turned(sel, w, h));
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

    /// The `w` by `h` pixels at `sel` (desktop pixels) of the turned capture, each
    /// blended from the captured pixels around where it came from. Areas that came from
    /// outside every monitor stay transparent.
    fn crop_turned(&self, sel: Rect, w: u32, h: u32) -> RgbaImage {
        let origin = self.rotation.to_source(sel.min + vec2(0.5, 0.5));
        // One pixel right, and one down, on the desktop, as steps in the capture.
        let right = self.rotation.to_source(sel.min + vec2(1.5, 0.5)) - origin;
        let down = self.rotation.to_source(sel.min + vec2(0.5, 1.5)) - origin;
        let sources: Vec<(&Rect, &RgbaImage)> = self.desktop.monitors.iter().zip(&self.desktop.shots).collect();
        RgbaImage::from_fn(w, h, |x, y| {
            let s = origin + right * x as f32 + down * y as f32;
            match sources.iter().find(|(m, _)| m.contains(s)) {
                Some((m, shot)) => bilinear(shot, s - m.min.to_vec2()),
                None => image::Rgba([0; 4]),
            }
        })
    }

    /// A blur region's processed pixels and the desktop area they cover.
    fn blurred(&self, shape: &Annotation) -> Option<(RgbaImage, Rect)> {
        let area = shape.rect().intersect(self.bounds());
        let area = Rect::from_min_max(area.min.round(), area.max.round());
        let strength = shape.width.round().max(1.0) as u32;
        if shape.effect == Effect::Redact {
            // Not derived from the capture at all, and always opaque, so nothing
            // underneath survives (including areas between monitors).
            let [r, g, b, _] = shape.color.to_srgba_unmultiplied();
            let (w, h) = (area.width() as u32, area.height() as u32);
            if w == 0 || h == 0 {
                return None;
            }
            return Some((RgbaImage::from_pixel(w, h, image::Rgba([r, g, b, 255])), area));
        }
        let mut img = self.crop_desktop(area)?;
        match shape.effect {
            Effect::Pixelate => effects::pixelate(&mut img, strength),
            Effect::Gaussian => effects::blur(&mut img, strength),
            Effect::Redact => unreachable!(),
        }
        Some((img, area))
    }

    /// Blur regions in the order they are applied: pixelate and Gaussian first, then
    /// every redaction. Pixelate and blur are computed from the original capture, so
    /// one applied after an overlapping redaction would put content derived from the
    /// hidden pixels back on top of it.
    fn blur_order(&self) -> Vec<usize> {
        let blurs = (0..self.annotations.len()).filter(|&i| self.annotations[i].kind == ShapeKind::Blur);
        let (redactions, mut order): (Vec<_>, Vec<_>) =
            blurs.partition(|&i| self.annotations[i].effect == Effect::Redact);
        order.extend(redactions);
        order
    }

    /// The final image: the cropped capture with blur regions applied and every other
    /// annotation drawn on top, then the border and shadow from the settings around it.
    /// Blur regions always act on the capture itself, below all other annotations, both
    /// here and on screen.
    fn output(&self) -> Option<RgbaImage> {
        let origin = self.selection?.intersect(self.bounds()).min.round();
        let mut img = self.redacted()?;
        annotate::render_onto(&mut img, origin, &self.annotations);
        Some(effects::frame(img, &self.frame()))
    }

    /// The selection with blur regions and redactions applied, and nothing else: what
    /// text and QR codes are read from. What the user hid stays hidden from reading too;
    /// their own arrows and text are left out, as they would only get in the way.
    fn redacted(&self) -> Option<RgbaImage> {
        let origin = self.selection?.intersect(self.bounds()).min.round();
        let mut img = self.cropped()?;
        for shape in self.blur_order().into_iter().map(|i| &self.annotations[i]) {
            if let Some((blurred, area)) = self.blurred(shape) {
                let offset = area.min - origin;
                image::imageops::replace(&mut img, &blurred, offset.x as i64, offset.y as i64);
            }
        }
        Some(img)
    }

    /// Start reading the region's text or QR codes in the background, and open the
    /// panel that shows what is found.
    fn start_reading(&mut self, ctx: &egui::Context, kind: read::Kind) {
        self.finish_editing();
        self.save_open = false;
        self.decor_open = false;
        self.picking_color = false;
        let Some(img) = self.redacted() else { return };
        let Some(origin) = self.selection.map(|s| s.intersect(self.bounds()).min.round()) else { return };
        // Turned by hand, the text is taken to be the way up the user put it.
        let as_is = !self.rotation.is_none();
        let (send, receive) = std::sync::mpsc::channel();
        let ctx = ctx.clone();
        std::thread::spawn(move || {
            let _ = send.send(read::read(kind, &img, as_is));
            ctx.request_repaint();
        });
        self.reading =
            Some(Reading { kind, origin, pending: Some(receive), outcome: None, picked: HashSet::new(), text: String::new() });
    }

    /// The reading panel, next to the toolbar: what was read, editable, and a button to
    /// copy it. Returns the text to copy when the user asks for it.
    fn reading_panel(&mut self, ctx: &egui::Context, toolbar: Rect, below: bool, within: Rect) -> Option<String> {
        let reading = self.reading.as_mut()?;
        if let Some(outcome) = reading.pending.as_ref().and_then(|r| r.try_recv().ok()) {
            reading.outcome = Some(outcome);
            reading.pending = None;
            reading.text = reading.picked_text();
        }
        let (pos, pivot) = if below {
            (toolbar.right_bottom() + vec2(0.0, 6.0), Align2::RIGHT_TOP)
        } else {
            (toolbar.right_top() - vec2(0.0, 6.0), Align2::RIGHT_BOTTOM)
        };
        let mut copy = None;
        let mut close = false;
        egui::Area::new(egui::Id::unique("duskshot-reading-panel"))
            .order(egui::Order::Foreground)
            .pivot(pivot)
            .fixed_pos(pos)
            .constrain_to(within)
            .show(ctx, |ui| {
                egui::Frame::popup(ui.style()).show(ui, |ui| {
                    ui.set_width(380.0);
                    let title = match reading.kind {
                        read::Kind::Text => "Text in the region",
                        read::Kind::Qr => "QR code",
                    };
                    ui.label(egui::RichText::new(title).strong());
                    ui.add_space(4.0);
                    let weak = ui.visuals().weak_text_color();
                    match &reading.outcome {
                        None => {
                            ui.horizontal(|ui| {
                                ui.spinner();
                                ui.label("Reading…");
                            });
                        }
                        Some(Err(why)) => {
                            ui.label(why.as_str());
                        }
                        Some(Ok(lines)) => {
                            let several = lines.len() > 1 || lines.iter().any(|l| l.words.len() > 1);
                            let hint = match (reading.kind, reading.picked.is_empty()) {
                                (_, false) => "Showing what you picked. Click it again to let it go.",
                                (read::Kind::Text, true) if several => {
                                    "Click lines in the region to copy only those; Shift+click picks single words."
                                }
                                (read::Kind::Qr, true) if several => "Click a code in the region to copy only that one.",
                                _ => "",
                            };
                            if !hint.is_empty() {
                                ui.label(egui::RichText::new(hint).color(weak).small());
                            }
                            egui::ScrollArea::vertical().max_height(240.0).show(ui, |ui| {
                                ui.add(egui::TextEdit::multiline(&mut reading.text).desired_width(f32::INFINITY).desired_rows(3));
                            });
                            if reading.kind == read::Kind::Qr && reading.text.contains("://") {
                                ui.label(egui::RichText::new("Links are copied as text, never opened.").color(weak).small());
                            }
                        }
                    }
                    ui.add_space(4.0);
                    ui.horizontal(|ui| {
                        let found = matches!(reading.outcome, Some(Ok(_)));
                        let button = ui.add_enabled(found, egui::Button::new("Copy text"));
                        if button.on_hover_text("Copy and close (Ctrl+Enter)").clicked() {
                            copy = Some(reading.text.clone());
                        }
                        if found && !reading.picked.is_empty() && ui.button("Reset picks").clicked() {
                            reading.picked.clear();
                            reading.text = reading.picked_text();
                        }
                        if ui.button("Close").on_hover_text("Esc").clicked() {
                            close = true;
                        }
                    });
                    if found_any(reading) && ui.input(|i| i.modifiers.command && i.key_pressed(Key::Enter)) {
                        copy = Some(reading.text.clone());
                    }
                });
            });
        if close {
            self.reading = None;
        }
        copy
    }

    /// Outline what was read, on the capture: every line (or QR code) faintly, the
    /// picked words filled.
    fn paint_reading(&self, painter: &egui::Painter, to_pt: impl Fn(Pos2) -> Pos2, hover: Option<(usize, usize)>, word_only: bool) {
        let Some(reading) = &self.reading else { return };
        // While the region moves or the capture turns, they no longer sit on what they
        // mark; it is read again when the drag ends.
        if matches!(self.action, Some(Action::Region(_) | Action::Rotate { .. })) {
            return;
        }
        // At the text's own angle, a little outside it.
        let area = |q: read::Quad| q.expand(2.0, 2.0).map(|p| to_pt(p + reading.origin.to_vec2())).0.to_vec();
        for (i, line) in reading.lines().iter().enumerate() {
            let hovered_line = hover.is_some_and(|(h, _)| h == i) && !word_only;
            let outline = if hovered_line { ACCENT } else { ACCENT.gamma_multiply(0.55) };
            painter.add(egui::Shape::closed_line(area(line.quad), Stroke::new(1.0, outline)));
            for (j, word) in line.words.iter().enumerate() {
                if reading.picked.contains(&(i, j)) {
                    painter.add(egui::Shape::convex_polygon(area(word.quad), ACCENT.gamma_multiply(0.35), Stroke::NONE));
                } else if word_only && hover == Some((i, j)) {
                    painter.add(egui::Shape::closed_line(area(word.quad), Stroke::new(1.0, ACCENT)));
                }
            }
        }
    }

    fn copy_text_to_clipboard(text: &str) -> AnyResult<()> {
        #[cfg(debug_assertions)]
        if let Some(path) = std::env::var_os("DUSKSHOT_COPY_TO") {
            std::fs::write(path, text)?;
            return Ok(());
        }
        // Like images: kept out of clipboard history and cloud sync.
        use arboard::SetExtWindows;
        arboard::Clipboard::new()?.set().exclude_from_history().exclude_from_cloud().text(text)?;
        Ok(())
    }

    /// The border and shadow this capture gets.
    fn frame(&self) -> effects::Frame {
        effects::Frame { border: self.border_on.then_some(self.border), shadow: self.shadow }
    }

    /// The decoration panel, next to the toolbar: the border and the drop shadow.
    fn decor_panel(&mut self, ctx: &egui::Context, toolbar: Rect, below: bool, within: Rect) {
        let (pos, pivot) = if below {
            (toolbar.right_bottom() + vec2(0.0, 6.0), Align2::RIGHT_TOP)
        } else {
            (toolbar.right_top() - vec2(0.0, 6.0), Align2::RIGHT_BOTTOM)
        };
        egui::Area::new(egui::Id::unique("duskshot-decor-panel"))
            .order(egui::Order::Foreground)
            .pivot(pivot)
            .fixed_pos(pos)
            .constrain_to(within)
            .show(ctx, |ui| {
                egui::Frame::popup(ui.style()).show(ui, |ui| {
                    ui.horizontal(|ui| {
                        ui.spacing_mut().item_spacing.x = 2.0;
                        if ui.checkbox(&mut self.border_on, "Border").changed() && self.border_on {
                            // The colour and line style in use, as for a new shape.
                            if std::mem::take(&mut self.border_unset) {
                                self.border.color = self.color;
                                self.border.dash = self.dash;
                            }
                        }
                        ui.add_space(6.0);
                        ui.add_enabled_ui(self.border_on, |ui| {
                            icons::color_picker_button(ui, &mut self.border.color).on_hover_text("Border colour");
                            ui.add_sized(
                                [52.0, 22.0],
                                egui::DragValue::new(&mut self.border.width)
                                    .range(1.0..=crate::settings::MAX_BORDER_WIDTH as f32)
                                    .speed(0.25)
                                    .fixed_decimals(0)
                                    .suffix(" px"),
                            )
                            .on_hover_text("Border thickness");
                            icons::picker(ui, &mut self.border.dash, true, &Dash::ALL, icons::dash_preview)
                                .on_hover_text("Border style");
                        });
                    });
                    ui.checkbox(&mut self.shadow, "Drop shadow")
                        .on_hover_text("A soft shadow around the image. Not shown here: it appears in the copied or saved image.");
                });
            });
    }

    /// Esc or Close: leave at once, or ask first if there are annotations to lose and
    /// the settings say to ask. Returns true to close now.
    fn request_close(&mut self) -> bool {
        self.finish_editing();
        if self.confirm_discard && !self.annotations.is_empty() {
            self.confirm_open = true;
            self.save_open = false;
            self.decor_open = false;
            self.reading = None;
            self.picking_color = false;
            return false;
        }
        true
    }

    /// The question asked before discarding annotations, in the middle of `monitor`
    /// (in points). Returns `Some(true)` to discard and close, `Some(false)` to go back.
    fn confirm_panel(&self, ctx: &egui::Context, monitor: Rect) -> Option<bool> {
        let mut answer = None;
        egui::Area::new(egui::Id::unique("duskshot-confirm-discard"))
            .order(egui::Order::Foreground)
            .pivot(Align2::CENTER_CENTER)
            .fixed_pos(monitor.center())
            .constrain_to(monitor)
            .show(ctx, |ui| {
                egui::Frame::popup(ui.style()).inner_margin(14.0).show(ui, |ui| {
                    ui.label(egui::RichText::new("Discard this capture?").strong().size(15.0));
                    ui.add_space(2.0);
                    let count = self.annotations.len();
                    let what = if count == 1 { "1 annotation".to_owned() } else { format!("{count} annotations") };
                    ui.label(format!("It has {what}, and has not been copied or saved."));
                    ui.add_space(10.0);
                    ui.horizontal(|ui| {
                        if ui.button("Discard").on_hover_text("Enter").clicked() {
                            answer = Some(true);
                        }
                        if ui.button("Keep editing").on_hover_text("Esc").clicked() {
                            answer = Some(false);
                        }
                    });
                });
            });
        answer
    }

    fn copy_to_clipboard(img: &RgbaImage) -> AnyResult<()> {
        // Test hook: write the "copied" image to a file instead, so automated checks
        // can inspect the output without touching the user's clipboard. Debug builds
        // only: in a release build Copy always goes to the clipboard.
        #[cfg(debug_assertions)]
        if let Some(path) = std::env::var_os("DUSKSHOT_COPY_TO") {
            img.save(path)?;
            return Ok(());
        }
        // As PNG with its transparency and as a bitmap on white, kept out of clipboard
        // history and cloud sync (see `clipboard`).
        crate::clipboard::set_image(img)
    }

    // --- Toolbar -----------------------------------------------------------------------

    /// The style options that apply to `kind`: stroke style for lines and outlines,
    /// plus arrowheads for arrows, text style for text, the effect for blur regions,
    /// and nothing when no tool or shape is active.
    fn context_options(&mut self, ui: &mut egui::Ui, kind: Option<ShapeKind>) {
        let sample = |kind, width| Annotation {
            kind,
            a: Pos2::ZERO,
            b: Pos2::ZERO,
            color: Color32::WHITE,
            width,
            dash: Dash::Solid,
            head: ArrowHead::Triangle,
            double_head: false,
            points: Vec::new(),
            text: String::new(),
            text_style: TextStyle::Plain,
            effect: Effect::Pixelate,
        };
        let Some(kind) = kind else { return };
        match kind {
            ShapeKind::Text => {
                icons::picker(ui, &mut self.text_style, true, &TextStyle::ALL, icons::text_style_preview)
                    .on_hover_text("Text style");
            }
            // Step markers have no options beyond colour and size.
            ShapeKind::Step => {}
            ShapeKind::Emoji => self.stamp_pickers(ui),
            ShapeKind::Blur => {
                icons::picker(ui, &mut self.effect, true, &Effect::ALL, icons::effect_preview)
                    .on_hover_text("Effect");
            }
            _ => {
                icons::picker(ui, &mut self.dash, true, &Dash::ALL, icons::dash_preview)
                    .on_hover_text("Stroke style");
            }
        }
        if kind == ShapeKind::Arrow {
            icons::picker(ui, &mut self.head, true, &ArrowHead::ALL, |painter, area, head, color| {
                let mut shape = sample(ShapeKind::Arrow, 1.6);
                (shape.a, shape.b, shape.color, shape.head) =
                    (area.left_center(), area.right_center(), color, head);
                shape.paint(painter, |p| p, 1.0);
            })
            .on_hover_text("Arrowhead");
            let both = icons::button(ui, Icon::DoubleHead, self.double_head, true);
            if both.on_hover_text("Heads on both ends").clicked() {
                self.double_head = !self.double_head;
            }
        }
    }

    /// What the hotkey strip shows now, in groups.
    fn strip_entries(&self, reading_lines: bool) -> Vec<Vec<help::Entry>> {
        use help::Entry as E;
        let one = |entries: Vec<E>| vec![entries];
        if self.confirm_open {
            one(vec![E::new(&["Enter"], "discard"), E::new(&["Esc"], "keep editing")])
        } else if reading_lines {
            one(vec![
                E::new(&["click"], "pick a line"),
                E::new(&["Shift", "click"], "a word"),
                E::new(&["Ctrl", "Enter"], "copy"),
                E::new(&["Esc"], "close"),
            ])
        } else if self.picking_color {
            one(vec![E::new(&["click"], "pick"), E::new(&["Shift"], "magnify"), E::new(&["Esc"], "cancel")])
        } else if self.editing.is_some() {
            one(vec![
                E::new(&["Enter"], "finish"),
                E::new(&["Shift", "Enter"], "new line"),
                E::new(&["←", "→"], "move"),
                E::new(&["Ctrl", "drag"], "move text"),
            ])
        } else if matches!(self.action, Some(Action::Rotate { .. })) {
            one(vec![E::new(&["Shift"], "15° steps"), E::new(&["right-click"], "the handle to reset")])
        } else if self.selection.is_none() {
            one(vec![
                E::new(&["Drag"], "select an area"),
                E::new(&["Ctrl", "click"], "a window"),
                E::new(&["Ctrl"], "snap"),
                E::new(&["Ctrl", "A"], "whole screen"),
                E::new(&["F1"], "help"),
                E::new(&["Esc"], "cancel"),
            ])
        } else {
            let tools = self
                .tools
                .iter()
                .map(|(t, k)| E {
                    keys: vec![crate::keys::label(*k)],
                    what: help::What::Tool(t.icon),
                    active: tool_for(t.icon) == Some(self.tool),
                })
                .collect();
            let mut rest = vec![E::new(&["Ctrl", "C"], "copy"), E::new(&["Ctrl", "S"], "save"), E::new(&["Ctrl", "Z"], "undo")];
            if self.desktop.opened.is_some() {
                rest.push(E::new(&["Ctrl", "A"], "whole image"));
            }
            if self.desktop.opened.is_some() {
                rest.push(E::new(&["Ctrl", "wheel"], "zoom"));
                if self.zoom > 1.0 {
                    rest.extend([E::new(&["Space", "drag"], "pan"), E::new(&["Ctrl", "0"], "fit")]);
                }
            }
            rest.extend([E::new(&["F1"], "help"), E::new(&["Esc"], "close")]);
            // The current tool's own keys and modifiers, shortened to their first clause.
            let own: Vec<E> = TOOLS
                .iter()
                .find(|t| tool_for(t.icon) == Some(self.tool))
                .and_then(|t| help::tool_help(t.id))
                .map(|h| {
                    h.mods
                        .iter()
                        .map(|&(keys, does)| E::new(keys, does.split(", ").next().unwrap_or(does)))
                        .collect()
                })
                .unwrap_or_default();
            if own.is_empty() { vec![tools, rest] } else { vec![tools, own, rest] }
        }
    }

    /// Draw `emoji` as large as fits in `area`, whole and centred, from a texture made
    /// once per emoji and size.
    fn paint_emoji(&mut self, ui: &egui::Ui, emoji: &str, area: Rect) {
        self.paint_icon(ui, emoji, area, false);
    }

    /// Draw `text` as large as fits in `area`, centred, from a texture made once per
    /// text and size. The texture is drawn at the screen's own pixel size, on whole
    /// pixels, so it stays sharp; with `plain`, in one colour (the font's black-and-white
    /// emoji).
    fn paint_icon(&mut self, ui: &egui::Ui, text: &str, area: Rect, plain: bool) {
        let ppp = ui.ctx().pixels_per_point();
        let fit = [(area.width() * ppp).floor().max(1.0) as u32, (area.height() * ppp).floor().max(1.0) as u32];
        let key = (text.to_owned(), fit, plain);
        if !self.emoji_icons.contains_key(&key) {
            let ink = ui.visuals().strong_text_color();
            let Some(texture) = fitted_texture(ui.ctx(), text, fit, ink, plain) else { return };
            self.emoji_icons.insert(key.clone(), texture);
        }
        if let Some(texture) = self.emoji_icons.get(&key) {
            let size = vec2(texture.size()[0] as f32, texture.size()[1] as f32);
            // Normally it fits as made; if it still does not (a glyph that kept growing
            // past the cell), shrink it evenly rather than spill into the next cell.
            let size = size * (fit[0] as f32 / size.x).min(fit[1] as f32 / size.y).min(1.0);
            let min = ((area.center().to_vec2() * ppp - size / 2.0).round() / ppp).to_pos2();
            let shown = Rect::from_min_size(min, size / ppp);
            let uv = Rect::from_min_max(Pos2::ZERO, Pos2::new(1.0, 1.0));
            ui.painter().image(texture.id(), shown, uv, Color32::WHITE);
        }
    }

    /// The stamp tool's three pickers side by side: emoji, kaomoji and symbols. Each is
    /// a button that opens a grid; the one the current stamp comes from shows it.
    fn stamp_pickers(&mut self, ui: &mut egui::Ui) {
        for set in [StampSet::Emoji, StampSet::Kaomoji, StampSet::Symbols] {
            self.stamp_picker(ui, set);
        }
    }

    /// One stamp picker. The emoji grid also has a field that takes any emoji,
    /// kaomoji or symbol, put there by Windows' emoji panel (Win+.) or typed.
    fn stamp_picker(&mut self, ui: &mut egui::Ui, set: StampSet) {
        let (columns, cell, button_width, tip) = match set {
            StampSet::Emoji => (8, vec2(30.0, 30.0), icons::PICKER_WIDTH, "Emoji"),
            StampSet::Kaomoji => (3, vec2(124.0, 30.0), 64.0, "Kaomoji"),
            StampSet::Symbols => (10, vec2(28.0, 28.0), icons::PICKER_WIDTH, "Symbols"),
        };
        let popup_id = ui.auto_id_with(("stamp-picker", tip));
        let open = egui::Popup::is_id_open(ui.ctx(), popup_id);
        let current = self.emoji.clone();
        let ours = StampSet::of(&current) == set;
        let (rect, response) = ui.allocate_exact_size(vec2(button_width, icons::BUTTON), Sense::click());
        let visuals = ui.style().interact_selectable(&response, open || ours);
        if open || ours || response.hovered() {
            ui.painter().rect_filled(rect, 4.0, visuals.bg_fill);
        }
        // The current stamp on its own picker; a sample on the others.
        let shown = if ours { current.as_str() } else { set.items().first().copied().unwrap_or_default() };
        let icon = Rect::from_min_max(rect.left_top() + vec2(5.0, 5.0), rect.right_bottom() - vec2(15.0, 5.0));
        let icon = if set == StampSet::Kaomoji { icon.shrink2(vec2(0.0, 3.0)) } else { icon };
        self.paint_emoji(ui, shown, icon);
        let c = Pos2::new(rect.right() - 7.0, rect.center().y + 1.0);
        let caret = vec![c + vec2(-3.0, -1.5), c + vec2(3.0, -1.5), c + vec2(0.0, 1.8)];
        ui.painter().add(egui::Shape::convex_polygon(caret, visuals.fg_stroke.color, Stroke::NONE));
        let response = response.on_hover_text(tip);
        let just_opened = response.clicked() && !open;

        let mut chosen = None;
        egui::Popup::menu(&response)
            .id(popup_id)
            .close_behavior(egui::PopupCloseBehavior::CloseOnClickOutside)
            .show(|ui| {
                if set == StampSet::Emoji {
                    chosen = self.emoji_panel(ui, just_opened);
                    return;
                }
                let mut list: Vec<&str> = set.items().to_vec();
                if ours && !list.contains(&current.as_str()) {
                    list.insert(0, &current);
                }
                egui::Grid::new(("stamp-grid", tip)).spacing(vec2(2.0, 2.0)).show(ui, |ui| {
                    for (i, &item) in list.iter().enumerate() {
                        let (area, cell_response) = ui.allocate_exact_size(cell, Sense::click());
                        let selected = item == current;
                        let visuals = ui.style().interact_selectable(&cell_response, selected);
                        if selected || cell_response.hovered() {
                            ui.painter().rect_filled(area, 4.0, visuals.bg_fill);
                        }
                        let inset = if set == StampSet::Kaomoji { vec2(6.0, 7.0) } else { Vec2::splat(4.0) };
                        self.paint_emoji(ui, item, area.shrink2(inset));
                        if cell_response.clicked() {
                            chosen = Some(item.to_owned());
                        }
                        if (i + 1) % columns == 0 {
                            ui.end_row();
                        }
                    }
                });
            });
        // Picking closes the grid, so the stamp shows under the pointer at once.
        if let Some(stamp) = chosen {
            egui::Popup::close_id(ui.ctx(), popup_id);
            self.choose_emoji(stamp);
        }
    }

    /// The emoji picker: a search field, a tab for recent emoji and one per group, and
    /// the grid. Only the rows in view are drawn. Returns the emoji picked, if any.
    fn emoji_panel(&mut self, ui: &mut egui::Ui, focus: bool) -> Option<String> {
        const COLUMNS: usize = 9;
        const CELL: f32 = 30.0;
        const ROWS_SHOWN: f32 = 7.0;
        let width = COLUMNS as f32 * (CELL + 2.0) - 2.0;
        // Icons are drawn once per emoji and kept; let them go now and then, so
        // browsing every group does not pile them up.
        if self.emoji_icons.len() > 600 {
            self.emoji_icons.clear();
        }
        ui.set_max_width(width);
        let mut chosen = self.panel_field(ui, width, focus);

        let query = self.emoji_field.trim().to_owned();
        let searching = !query.is_empty() && self.emoji_pending.is_none();
        // Tabs: recent, then the groups.
        if !searching {
            ui.horizontal(|ui| {
                ui.spacing_mut().item_spacing.x = 2.0;
                let tabs = std::iter::once("🕘").chain(crate::emoji::GROUPS.iter().map(|g| g.1));
                for (i, icon) in tabs.enumerate() {
                    let (area, tab) = ui.allocate_exact_size(vec2(CELL - 3.0, CELL - 3.0), Sense::click());
                    let selected = self.emoji_tab == i;
                    let visuals = ui.style().interact_selectable(&tab, selected);
                    if selected || tab.hovered() {
                        ui.painter().rect_filled(area, 4.0, visuals.bg_fill);
                    }
                    // In one colour, so the tabs read as tabs, not as emoji to pick.
                    self.paint_icon(ui, icon, area.shrink(5.0), true);
                    let name = if i == 0 { "Recent" } else { crate::emoji::GROUPS[i - 1].0 };
                    if tab.on_hover_text(name).clicked() {
                        self.emoji_tab = i;
                    }
                }
            });
            ui.separator();
        }
        let list: Vec<String> = if searching {
            if self.emoji_search.as_ref().is_none_or(|(q, _)| *q != query) {
                self.emoji_search = Some((query.clone(), crate::emoji::search(&query, 200)));
            }
            self.emoji_search.as_ref().map(|(_, found)| found.iter().map(|e| (*e).to_owned()).collect()).unwrap_or_default()
        } else if self.emoji_tab == 0 {
            self.recent_emoji.clone()
        } else {
            let tab = self.emoji_tab - 1;
            self.emoji_groups.entry(tab).or_insert_with(|| crate::emoji::group(tab)).iter().map(|e| (*e).to_owned()).collect()
        };
        let entered = ui.input(|i| i.key_pressed(Key::Enter));
        if searching && entered && chosen.is_none() {
            chosen = list.first().cloned();
        }
        if searching && list.is_empty() {
            ui.label(egui::RichText::new("No emoji found").color(ui.visuals().weak_text_color()));
            return chosen;
        }
        let current = self.emoji.clone();
        let rows = list.len().div_ceil(COLUMNS);
        egui::ScrollArea::vertical()
            .max_height(ROWS_SHOWN * (CELL + 2.0))
            .min_scrolled_height(ROWS_SHOWN * (CELL + 2.0))
            .auto_shrink([true, true])
            .show_rows(ui, CELL, rows, |ui, range| {
                ui.set_width(width);
                ui.spacing_mut().item_spacing = vec2(2.0, 2.0);
                for row in range {
                    ui.horizontal(|ui| {
                        for emoji in list.iter().skip(row * COLUMNS).take(COLUMNS) {
                            let (area, cell) = ui.allocate_exact_size(Vec2::splat(CELL), Sense::click());
                            let selected = *emoji == current;
                            let visuals = ui.style().interact_selectable(&cell, selected);
                            if selected || cell.hovered() {
                                ui.painter().rect_filled(area, 4.0, visuals.bg_fill);
                            }
                            self.paint_emoji(ui, emoji, area.shrink(4.0));
                            if cell.clicked() {
                                chosen = Some(emoji.clone());
                            }
                        }
                    });
                }
            });
        chosen
    }

    /// Put `self.emoji` first among the recent emoji, if it is one, and save the list.
    fn note_recent(&mut self) {
        if !crate::emoji::is_emoji(&self.emoji) {
            return;
        }
        self.recent_emoji.retain(|e| *e != self.emoji);
        self.recent_emoji.insert(0, self.emoji.clone());
        self.recent_emoji.truncate(crate::emoji::FAVOURITES.len());
        if self.persist_recent {
            crate::settings::store_recent(&self.recent_emoji);
        }
    }

    /// The emoji picker's field: typing searches. It also takes what Windows' emoji
    /// panel (Win+.) puts there. Returns a stamp once one is complete.
    fn panel_field(&mut self, ui: &mut egui::Ui, width: f32, focus: bool) -> Option<String> {
        let field = ui.add(
            egui::TextEdit::singleline(&mut self.emoji_field)
                .hint_text("Search emoji")
                .desired_width(width),
        );
        if focus {
            field.request_focus();
        }
        if field.changed() {
            // Keys typed here come with the key that typed them; the panel sends its
            // text alone, a kaomoji perhaps a character at a time.
            let keyed = ui.input(|i| {
                i.events.iter().any(|e| matches!(e, egui::Event::Key { pressed: true, physical_key: Some(_), .. }))
            });
            self.emoji_pending = (!keyed).then(std::time::Instant::now);
        }
        let typed = self.emoji_field.trim().to_owned();
        let settled = self.emoji_pending.is_some_and(|t| t.elapsed() >= PANEL_PAUSE);
        if (field.changed() && crate::emoji::is_emoji(&typed)) || settled {
            self.emoji_pending = None;
            if crate::emoji::can_stamp(&typed) {
                self.emoji_field.clear();
                return Some(typed);
            }
        } else if self.emoji_pending.is_some() {
            ui.ctx().request_repaint_after(PANEL_PAUSE);
        }
        None
    }

    /// The next stamp, drawn as it would be placed with its centre at the origin: the
    /// image and the desktop-pixel area it covers.
    fn emoji_preview_layer(&self) -> Option<(egui::ColorImage, Rect)> {
        let stamp = Annotation {
            text: self.emoji.clone(),
            ..Annotation::stroked(ShapeKind::Emoji, Pos2::ZERO, Pos2::ZERO, self.color, self.width, Dash::Solid)
        };
        stamp.layer(Rect::EVERYTHING)
    }

    /// Make `emoji` the stamp. With the stamp tool, it is for the next stamp; with the
    /// Select tool, it replaces the selected stamp's.
    fn choose_emoji(&mut self, emoji: String) {
        if self.tool == Tool::Shape(ShapeKind::Emoji) {
            self.selected = None;
        }
        self.emoji = emoji;
    }

    fn toolbar(&mut self, ctx: &egui::Context, pos: Pos2, pivot: Align2, within: Rect) -> ToolbarOutput {
        let mut out = ToolbarOutput::default();
        let shown = egui::Area::new(egui::Id::unique("duskshot-toolbar"))
            .order(egui::Order::Foreground)
            .pivot(pivot)
            .fixed_pos(pos)
            .constrain_to(within)
            .show(ctx, |ui| {
                egui::Frame::popup(ui.style()).show(ui, |ui| {
                    ui.horizontal(|ui| {
                        ui.spacing_mut().item_spacing.x = 2.0;
                        for (info, key) in self.tools.clone() {
                            let Some(tool) = tool_for(info.icon) else { continue };
                            let button = icons::button(ui, info.icon, self.tool == tool, true);
                            if button.on_hover_text(info.tip(key)).clicked() {
                                self.set_tool(tool);
                            }
                        }

                        ui.separator();
                        for swatch in self.swatches {
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
                        if let Some(&(info, key)) = self.tools.iter().find(|(t, _)| t.icon == Icon::Eyedropper) {
                            let dropper = icons::button(ui, Icon::Eyedropper, self.picking_color, true);
                            if dropper.on_hover_text(info.tip(key)).clicked() {
                                self.picking_color = !self.picking_color;
                            }
                        }
                        icons::color_picker_button(ui, &mut self.color)
                            .on_hover_text("Custom colour");
                        let kind = self.style_kind();
                        let redact = kind == Some(ShapeKind::Blur) && self.effect == Effect::Redact;
                        let size_tip = match kind {
                            _ if redact => "Not used by Redact",
                            Some(ShapeKind::Text) => "Font size (mouse wheel)",
                            Some(ShapeKind::Blur) => "Strength (mouse wheel)",
                            Some(ShapeKind::Step | ShapeKind::Emoji) => "Size (mouse wheel)",
                            _ => "Thickness (mouse wheel)",
                        };
                        // Fixed width, so the toolbar does not shift when the value
                        // gains a digit.
                        ui.add_enabled_ui(!redact, |ui| {
                            ui.add_sized(
                                [52.0, 22.0],
                                egui::DragValue::new(&mut self.width)
                                    .range(width_range(kind))
                                    .speed(0.25)
                                    .fixed_decimals(0)
                                    .suffix(" px"),
                            )
                        })
                        .inner
                        .on_hover_text(size_tip)
                        .on_disabled_hover_text(size_tip);

                        // Options for the current tool or selected shape. They sit in an
                        // area sized for the largest set (the arrow's), so the toolbar
                        // keeps its size when switching tools.
                        let start = ui.cursor().min.x;
                        self.context_options(ui, kind);
                        let used = ui.cursor().min.x - start;
                        if used < OPTIONS_WIDTH {
                            ui.add_space(OPTIONS_WIDTH - used);
                        }

                        ui.separator();
                        let undo = icons::button(ui, Icon::Undo, false, self.history.can_undo());
                        if undo.on_hover_text("Undo (Ctrl+Z)").clicked() {
                            self.undo();
                        }
                        let redo = icons::button(ui, Icon::Redo, false, self.history.can_redo());
                        if redo.on_hover_text("Redo (Ctrl+Y)").clicked() {
                            self.redo();
                        }

                        // The readers, if switched on in the settings.
                        let readers = [(Icon::ReadText, read::Kind::Text), (Icon::ReadQr, read::Kind::Qr)];
                        let shown: Vec<_> =
                            readers.into_iter().filter_map(|(icon, kind)| Some((*self.tools.iter().find(|(t, _)| t.icon == icon)?, kind))).collect();
                        if !shown.is_empty() {
                            ui.separator();
                        }
                        for ((info, key), kind) in shown {
                            let open = self.reading.as_ref().is_some_and(|r| r.kind == kind);
                            if icons::button(ui, info.icon, open, true).on_hover_text(info.tip(key)).clicked() {
                                out.read = Some(kind);
                            }
                        }

                        ui.separator();
                        // Lit while a border or shadow is on, so it is not forgotten.
                        let decorated = self.border_on || self.shadow;
                        let decor = icons::button(ui, Icon::Decoration, self.decor_open || decorated, true);
                        out.decor = decor.on_hover_text("Decoration: border and shadow").clicked();

                        ui.separator();
                        let action = |ui: &mut egui::Ui, icon, tip| {
                            icons::button(ui, icon, false, true).on_hover_text(tip).clicked()
                        };
                        out.copy = action(ui, Icon::Copy, "Copy to clipboard (Ctrl+C)");
                        let save = icons::button(ui, Icon::Save, self.save_open, true);
                        out.save = save.on_hover_text("Save to file (Ctrl+S)").clicked();
                        out.close = action(ui, Icon::Close, "Close (Esc)");
                    });
                });
            });
        out.rect = Some(shown.response.rect);
        out
    }

    /// The save panel, next to the toolbar: file name, format and JPEG quality.
    /// Returns true when the user asks to save.
    fn save_panel(&mut self, ctx: &egui::Context, toolbar: Rect, below: bool, within: Rect) -> bool {
        let (pos, pivot) = if below {
            (toolbar.right_bottom() + vec2(0.0, 6.0), Align2::RIGHT_TOP)
        } else {
            (toolbar.right_top() - vec2(0.0, 6.0), Align2::RIGHT_BOTTOM)
        };
        let mut save = false;
        let options = &mut self.save_options;
        egui::Area::new(egui::Id::unique("duskshot-save-panel"))
            .order(egui::Order::Foreground)
            .pivot(pivot)
            .fixed_pos(pos)
            .constrain_to(within)
            .show(ctx, |ui| {
                egui::Frame::popup(ui.style()).show(ui, |ui| {
                    ui.set_width(300.0);
                    egui::Grid::new("save-grid").num_columns(2).spacing([10.0, 8.0]).show(ui, |ui| {
                        let field = crate::save::name_row(ui, "Name", &mut options.name, 176.0);
                        if field.lost_focus() && ui.input(|i| i.key_pressed(Key::Enter)) {
                            save = true;
                        }

                        ui.label("Format");
                        ui.horizontal(|ui| {
                            ui.selectable_value(&mut options.format, Format::Png, "PNG")
                                .on_hover_text("Lossless");
                            ui.selectable_value(&mut options.format, Format::Jpeg, "JPEG")
                                .on_hover_text("Smaller files, some quality loss");
                        });
                        ui.end_row();

                        if options.format == Format::Jpeg {
                            ui.label("Quality");
                            ui.horizontal(|ui| {
                                ui.selectable_value(&mut options.quality, QUALITY_HIGH, "High");
                                ui.selectable_value(&mut options.quality, QUALITY_MEDIUM, "Medium");
                                ui.add(egui::Slider::new(&mut options.quality, 10..=100));
                            });
                            ui.end_row();
                        }
                    });

                    ui.add_space(4.0);
                    let weak = ui.visuals().weak_text_color();
                    ui.label(egui::RichText::new(options.file_name()).color(weak).small());
                    if let Some(folder) = &options.folder {
                        let name = folder.file_name().map_or_else(
                            || folder.display().to_string(),
                            |n| n.to_string_lossy().into_owned(),
                        );
                        ui.label(egui::RichText::new(format!("Opens in {name}")).color(weak).small());
                    }
                    ui.add_space(4.0);
                    ui.horizontal(|ui| {
                        if ui.button("Save…").on_hover_text("Choose where (Enter)").clicked() {
                            save = true;
                        }
                        if ui.button("Cancel").on_hover_text("Esc").clicked() {
                            self.save_open = false;
                        }
                    });
                });
            });
        save
    }

    /// Ask where to save and write the file. Returns Ok(false) if the user cancelled.
    fn save_to_file(&mut self, parent: &eframe::Frame) -> AnyResult<bool> {
        let Some(img) = self.output() else { return Ok(false) };
        let Some(path) = self.save_options.ask_path(parent) else { return Ok(false) };
        save::write(&img, &path, self.save_options.format, self.save_options.quality)?;
        Ok(true)
    }
}

/// Make UI text as crisp as native Windows text.
///
/// - Use Segoe UI, the Windows UI font, read from the local Windows font folder. It
///   ships hinting instructions tuned for small sizes, which egui's bundled font lacks.
///   If it is missing, egui's own font is kept.
/// - Turn off sub-pixel binning: it lets glyphs sit at fractional pixel offsets, which
///   smears their edges at 100% display scaling.
pub(crate) fn setup_text(ctx: &egui::Context) {
    let font = std::env::var_os("WINDIR")
        .map(|dir| std::path::PathBuf::from(dir).join("Fonts").join("segoeui.ttf"))
        .and_then(|path| std::fs::read(path).ok());
    if let Some(bytes) = font {
        ctx.add_font(egui::epaint::text::FontInsert::new(
            "Segoe UI",
            egui::FontData::from_owned(bytes),
            vec![egui::epaint::text::InsertFontFamily {
                family: egui::FontFamily::Proportional,
                priority: egui::epaint::text::FontPriority::Highest,
            }],
        ));
    }
    ctx.all_styles_mut(|style| {
        // Tooltips cannot reach outside the window, so wrap them well inside it.
        style.spacing.tooltip_width = 300.0;
        style.visuals.text_options.subpixel_binning = false;
        style.visuals.text_options.font_hinting = true;
    });
}

/// `text` drawn as large as fits `fit` pixels once cropped: drawn at the height that
/// fills it, then smaller if what it covers is wider or taller than that.
fn fitted_texture(ctx: &egui::Context, text: &str, fit: [u32; 2], ink: Color32, plain: bool) -> Option<egui::TextureHandle> {
    let mut px = fit[1];
    for _ in 0..4 {
        let texture = emoji_texture_with(ctx, text, px, ink, plain)?;
        let [w, h] = texture.size().map(|n| n as u32);
        if w <= fit[0] && h <= fit[1] || px <= 2 {
            return Some(texture);
        }
        let scale = (fit[0] as f32 / w as f32).min(fit[1] as f32 / h as f32);
        px = ((px as f32 * scale).floor() as u32).min(px - 1).max(1);
    }
    emoji_texture_with(ctx, text, px, ink, plain)
}

#[cfg(test)]
fn emoji_texture(ctx: &egui::Context, emoji: &str, px: u32, ink: Color32) -> Option<egui::TextureHandle> {
    emoji_texture_with(ctx, emoji, px, ink, false)
}

/// `emoji` drawn about `px` pixels tall, cropped to the pixels it actually covers, with
/// any kaomoji or symbol in `ink`. Emoji differ in width and some reach beyond their
/// box, so the toolbar and picker fit this whole image into their cells rather than
/// drawing a fixed square.
fn emoji_texture_with(ctx: &egui::Context, emoji: &str, px: u32, ink: Color32, plain: bool) -> Option<egui::TextureHandle> {
    // Room on every side for whatever spills out of the emoji's box; a kaomoji is wide.
    let wide = crate::emoji::bounds(emoji, px as f32, Pos2::ZERO).width().ceil() as u32;
    let canvas = px.max(wide) * 2;
    let mut pixmap = tiny_skia::Pixmap::new(canvas, canvas)?;
    let centre = Pos2::new(canvas as f32 / 2.0, canvas as f32 / 2.0);
    let draw = if plain { crate::emoji::rasterize_plain } else { crate::emoji::rasterize };
    draw(emoji, px as f32, centre, ink, &mut pixmap.as_mut(), Pos2::ZERO);
    let (mut x0, mut y0, mut x1, mut y1) = (canvas, canvas, 0, 0);
    for (i, p) in pixmap.pixels().iter().enumerate() {
        if p.alpha() > 0 {
            let (x, y) = (i as u32 % canvas, i as u32 / canvas);
            (x0, y0, x1, y1) = (x0.min(x), y0.min(y), x1.max(x + 1), y1.max(y + 1));
        }
    }
    if x1 <= x0 || y1 <= y0 {
        return None; // nothing drawn
    }
    let (w, h) = ((x1 - x0) as usize, (y1 - y0) as usize);
    let data = pixmap.data();
    let mut cropped = Vec::with_capacity(w * h * 4);
    for y in y0..y1 {
        let row = (y * canvas + x0) as usize * 4;
        cropped.extend_from_slice(&data[row..row + w * 4]);
    }
    let image = egui::ColorImage::from_rgba_premultiplied([w, h], &cropped);
    Some(ctx.load_texture("emoji", image, egui::TextureOptions::LINEAR))
}

/// Identifies how an annotation looks, for the layer caches.
fn layer_key(a: &Annotation) -> u64 {
    let mut h = std::collections::hash_map::DefaultHasher::new();
    (a.kind, a.dash, a.color, a.width.to_bits(), a.text_style, a.effect).hash(&mut h);
    (a.text.as_str(), a.a.x.to_bits(), a.a.y.to_bits(), a.b.x.to_bits(), a.b.y.to_bits()).hash(&mut h);
    for p in &a.points {
        (p.x.to_bits(), p.y.to_bits()).hash(&mut h);
    }
    h.finish()
}

/// Text on a dark pill so it stays readable over any screenshot.
fn label(painter: &egui::Painter, pos: Pos2, anchor: Align2, text: String, color: Color32) {
    let galley = painter.layout_no_wrap(text, FontId::proportional(13.0), color);
    let rect = anchor.anchor_size(pos, galley.size());
    painter.rect_filled(rect.expand2(vec2(6.0, 3.0)), 4.0, Color32::from_black_alpha(200));
    painter.galley(rect.min, galley, color);
}

/// The hotkey strip's distance from the top of the screen, and from the bottom (clear
/// of the taskbar), in points.
const STRIP_TOP: f32 = 12.0;
const STRIP_BOTTOM: f32 = 56.0;

/// The eyedropper spotlight's radius, in points.
const SPOTLIGHT_RADIUS: f32 = 80.0;
/// Points per captured pixel in the spotlight while Shift magnifies it.
const SPOTLIGHT_ZOOM: f32 = 12.0;

/// A disc of radius `radius` around `centre` (in points) textured with `texture`,
/// where `uv` gives the texture coordinate for a point.
fn spotlight_mesh(texture: egui::TextureId, centre: Pos2, radius: f32, uv: impl Fn(Pos2) -> Pos2) -> egui::Mesh {
    const SEGMENTS: u32 = 96;
    let mut mesh = egui::Mesh::with_texture(texture);
    let mut vertex = |pos: Pos2| mesh.vertices.push(egui::epaint::Vertex { pos, uv: uv(pos), color: Color32::WHITE });
    vertex(centre);
    for i in 0..SEGMENTS {
        let angle = i as f32 / SEGMENTS as f32 * std::f32::consts::TAU;
        vertex(centre + vec2(angle.cos(), angle.sin()) * radius);
    }
    for i in 0..SEGMENTS {
        mesh.add_triangle(0, 1 + i, 1 + (i + 1) % SEGMENTS);
    }
    mesh
}

/// The picked colour's values beside the spotlight at `centre`: a swatch, hex, RGB and
/// HSL. On the right of the spotlight, or on its left near the screen's right edge.
fn color_readout(painter: &egui::Painter, centre: Pos2, radius: f32, color: Color32, screen: Rect) {
    let (h, s, l) = hsl(color);
    let text = format!(
        "{}\nRGB  {}, {}, {}\nHSL  {h}°, {s}%, {l}%",
        crate::settings::to_hex(color),
        color.r(),
        color.g(),
        color.b()
    );
    let galley = painter.layout_no_wrap(text, FontId::monospace(13.0), Color32::WHITE);
    const SWATCH: f32 = 22.0;
    const PAD: f32 = 8.0;
    let size = vec2(SWATCH + PAD + galley.size().x, galley.size().y.max(SWATCH)) + Vec2::splat(2.0 * PAD);
    let gap = radius + 14.0;
    let mut rect = Align2::LEFT_CENTER.anchor_size(centre + vec2(gap, 0.0), size);
    if rect.right() > screen.right() - 4.0 {
        rect = Align2::RIGHT_CENTER.anchor_size(centre - vec2(gap, 0.0), size);
    }
    // Keep it on screen vertically too.
    let below_top = (screen.top() + 4.0 - rect.top()).max(0.0);
    let above_bottom = (screen.bottom() - 4.0 - rect.bottom()).min(0.0);
    let rect = rect.translate(vec2(0.0, below_top + above_bottom));
    painter.rect_filled(rect, 6.0, Color32::from_black_alpha(215));
    let swatch = Rect::from_min_size(rect.min + Vec2::splat(PAD), Vec2::splat(SWATCH));
    painter.rect_filled(swatch, 3.0, color);
    painter.rect_stroke(swatch, 3.0, Stroke::new(1.0, Color32::from_gray(200)), StrokeKind::Outside);
    painter.galley(Pos2::new(swatch.right() + PAD, rect.top() + PAD), galley, Color32::WHITE);
}

/// Hue in degrees, saturation and lightness in percent, all rounded.
fn hsl(color: Color32) -> (u32, u32, u32) {
    let [r, g, b] = [color.r(), color.g(), color.b()].map(|c| f32::from(c) / 255.0);
    let (max, min) = (r.max(g).max(b), r.min(g).min(b));
    let l = (max + min) / 2.0;
    let d = max - min;
    if d == 0.0 {
        return (0, 0, (l * 100.0).round() as u32);
    }
    let s = d / (1.0 - (2.0 * l - 1.0).abs());
    let h = if max == r {
        ((g - b) / d).rem_euclid(6.0)
    } else if max == g {
        (b - r) / d + 2.0
    } else {
        (r - g) / d + 4.0
    };
    ((h * 60.0).round() as u32 % 360, (s * 100.0).round() as u32, (l * 100.0).round() as u32)
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
    /// Keep popups, tooltips and menus on one monitor. egui places them inside its
    /// content rect, which for this window spans every monitor, so one opened near a
    /// monitor's edge could land on the next monitor or off screen. Narrowing the content
    /// rect (through the safe-area insets) to the pointer's monitor keeps them on it.
    /// While a popup is open its monitor is kept, so it does not jump when the pointer
    /// leaves it.
    fn raw_input_hook(&mut self, ctx: &egui::Context, raw: &mut egui::RawInput) {
        let viewport = raw.screen_rect.unwrap_or_else(|| ctx.viewport_rect());
        if viewport.width() <= 0.0 || self.desktop.screens.is_empty() {
            return;
        }
        if self.desktop.opened.is_some() {
            return; // one screen, which may be zoomed: popups may use all of it
        }
        let scale = self.bounds().width() / viewport.width();
        let pointer = raw.events.iter().rev().find_map(|e| match e {
            egui::Event::PointerMoved(p) => Some(*p),
            _ => None,
        });
        if let Some(p) = pointer.filter(|_| !egui::Popup::is_any_open(ctx)) {
            let px = ((p - viewport.min) * scale).to_pos2();
            if let Some(i) = self.desktop.screens.iter().position(|m| m.contains(px)) {
                self.ui_monitor = i;
            }
        }
        let m = self.desktop.screens[self.ui_monitor.min(self.desktop.screens.len() - 1)];
        let (min, max) = (viewport.min + m.min.to_vec2() / scale, viewport.min + m.max.to_vec2() / scale);
        raw.safe_area_insets = Some(egui::SafeAreaInsets(egui::epaint::MarginF32 {
            left: (min.x - viewport.left()).max(0.0),
            right: (viewport.right() - max.x).max(0.0),
            top: (min.y - viewport.top()).max(0.0),
            bottom: (viewport.bottom() - max.y).max(0.0),
        }));
    }

    fn ui(&mut self, ui: &mut egui::Ui, frame: &mut eframe::Frame) {
        let ctx = ui.ctx().clone();
        if self.hwnd.is_none() {
            self.hwnd = hwnd_of(frame);
        }
        if self.cover_desktop(false) {
            ctx.request_repaint(); // check again next frame that the move stuck
        }
        self.claim_focus(&ctx);
        if crate::perf::frame(&ctx) {
            self.leave(&ctx); // `--trace-cycles`: shown, measured, done
        }

        // The whole window, across every monitor. (The content rect is only the monitor
        // popups are kept on; see `raw_input_hook`.)
        let screen = ctx.viewport_rect();
        let response = ui.allocate_rect(screen, Sense::drag());
        let painter = ui.painter_at(screen);

        // Image pixels per egui point, and conversions between the two spaces. An opened
        // image can be zoomed and panned; `pan` is in points.
        let fit_scale = self.bounds().width() / screen.width();
        if self.desktop.opened.is_some() {
            self.zoom_and_pan(&ctx, screen, fit_scale);
        }
        let scale = fit_scale / self.zoom;
        let pan = self.pan;
        let bounds = self.bounds();
        let to_px = |p: Pos2| ((p - screen.min - pan) * scale).to_pos2();
        let to_pt = |p: Pos2| screen.min + pan + p.to_vec2() / scale;
        let rect_to_pt = |r: Rect| Rect::from_min_max(to_pt(r.min), to_pt(r.max));
        if self.desktop.opened.is_some() {
            // The toolbar, popups and the strip stay in the part of the image on screen.
            self.desktop.screens[0] = Rect::from_min_max(to_px(screen.min), to_px(screen.max));
        }
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
        if self.rotation.is_none() {
            // Round an opened image, and in gaps between monitors of different sizes.
            painter.rect_filled(screen, 0.0, Color32::from_gray(24));
            let uv = Rect::from_min_max(Pos2::ZERO, Pos2::new(1.0, 1.0));
            for (m, texture) in self.desktop.monitors.iter().zip(&self.textures) {
                painter.image(texture.id(), rect_to_pt(*m), uv, Color32::WHITE);
            }
        } else {
            // Turned: smoothly sampled, on black where no monitor turns into view.
            if self.smooth_textures.is_empty() {
                for (i, shot) in self.desktop.shots.iter().enumerate() {
                    let size = [shot.width() as usize, shot.height() as usize];
                    self.smooth_textures.push(ctx.load_texture(
                        format!("monitor-smooth-{i}"),
                        egui::ColorImage::from_rgba_unmultiplied(size, shot.as_raw()),
                        egui::TextureOptions::LINEAR,
                    ));
                }
            }
            painter.rect_filled(screen, 0.0, Color32::BLACK);
            let rotation = self.rotation;
            for (m, texture) in self.desktop.monitors.iter().zip(&self.smooth_textures) {
                painter.add(turned_quad(texture.id(), *m, |p| to_pt(rotation.to_desktop(p))));
            }
        }

        // --- Mouse ---------------------------------------------------------------------
        let (hover, press_origin, mods) =
            ctx.input(|i| (i.pointer.latest_pos(), i.pointer.press_origin(), i.modifiers));
        self.mods = mods;
        if let Some(i) = hover.and_then(|p| {
            let p = to_px(p);
            self.desktop.screens.iter().position(|m| m.contains(p))
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

        // While the discard question is up, the capture underneath is left alone: only
        // the question (and the toolbar's Copy and Save) respond.
        let asking = self.confirm_open;
        // With text or QR codes read, a click on one picks it.
        // Only inside the region: nothing is read outside it, and the slack round words
        // must not reach the rotation handle or the region's edges.
        let read_hit = |overlay: &Self, p: Pos2| {
            let p = to_px(p);
            overlay.selection.filter(|s| s.contains(p)).and_then(|_| overlay.reading.as_ref()?.hit(p))
        };
        // A click while the shortcut sheet is up only closes it.
        // Only the left button draws and drags: right-click backs out (below), and the
        // middle button pans an opened image.
        let primary = egui::PointerButton::Primary;
        if response.drag_started_by(primary) && !asking && !self.help_open && !self.panning
            && let Some(origin) = press_origin.or(response.interact_pointer_pos()) {
                if let (Some(hit), Some(reading)) = (read_hit(self, origin), self.reading.as_mut()) {
                    reading.toggle(hit, mods.shift);
                    self.ignore_drag = true;
                } else if self.picking_color {
                    if let Some(color) = self.pixel_at(to_px(origin)) {
                        self.color = color;
                    }
                    self.picking_color = false;
                    self.ignore_drag = true;
                } else {
                    let action = self.pick_action(to_px(origin), tol);
                    self.begin_action(action);
                    self.error = None;
                }
            }
        if response.dragged_by(primary) && !self.ignore_drag
            && let Some(p) = response.interact_pointer_pos() {
                self.apply_action(to_px(p), mods, bounds);
            }
        if response.drag_stopped_by(primary) {
            if std::mem::take(&mut self.ignore_drag) {
                // The eyedropper's click: nothing to finish.
            } else {
                self.end_action();
            }
        }
        // Right-click on the rotation handle: back to no turn. Anywhere else it backs
        // out one step, like Esc, but never closes the capture.
        let right_click = ctx.input(|i| i.pointer.button_clicked(egui::PointerButton::Secondary).then(|| i.pointer.interact_pos()).flatten());
        if let Some(p) = right_click.filter(|_| !asking && self.action.is_none()) {
            if matches!(self.pick_action(to_px(p), tol), Action::Rotate { .. }) && !self.rotation.is_none() {
                self.reset_rotation();
            } else {
                self.back_out(&ctx);
            }
        }
        if std::mem::take(&mut self.reread) {
            match (self.reading.as_ref().map(|r| r.kind), self.selection) {
                (Some(kind), Some(_)) => self.start_reading(&ctx, kind),
                _ => self.reading = None,
            }
        }

        // Cursor reflects what a drag would do from here. Over the toolbar, egui decides.
        let preview = self.action.clone().or_else(|| {
            let p = hover.filter(|_| response.hovered())?;
            Some(self.pick_action(to_px(p), tol))
        });
        let read_hover = hover.filter(|_| response.hovered() && self.action.is_none()).and_then(|p| read_hit(self, p));
        if read_hover.is_some() {
            ctx.set_cursor_icon(CursorIcon::PointingHand);
        } else if let Some(action) = preview {
            ctx.set_cursor_icon(match action {
                Action::Region(Drag::Resize { hx, hy, .. })
                | Action::ResizeBox { drag: Drag::Resize { hx, hy, .. }, .. } => resize_cursor(hx, hy),
                Action::Region(Drag::Move { .. }) | Action::MoveShape { .. } => CursorIcon::Move,
                Action::MoveEnd { .. } | Action::Rotate { .. } => CursorIcon::Grab,
                Action::PlaceText { .. } | Action::Caret { .. } => CursorIcon::Text,
                _ => CursorIcon::Crosshair,
            });
        }

        // --- Keyboard and wheel ----------------------------------------------------------
        #[derive(Default)]
        struct Keys {
            nudge: Vec2,
            select_all: bool,
            copy: bool,
            enter: bool,
            save: bool,
            cancel: bool,
            undo: bool,
            redo: bool,
            delete: bool,
            tool: Option<Tool>,
            eyedropper: bool,
            read: Option<read::Kind>,
            wheel: f32,
            help: bool,
        }
        let typing = ctx.egui_wants_keyboard_input();
        let enabled = &self.tools;
        let zoomable = self.desktop.opened.is_some();
        let keys = ctx.input(|i| {
            let step = if i.modifiers.shift { 10.0 } else { 1.0 };
            let axis = |neg: Key, pos: Key| {
                (i.num_presses(pos) as f32 - i.num_presses(neg) as f32) * step
            };
            let ctrl = i.modifiers.command;
            let plain = !ctrl && !i.modifiers.alt;
            // Keys of tools switched off in the settings do nothing.
            let pressed = enabled.iter().find(|(_, key)| plain && i.key_pressed(*key)).map(|(t, _)| t.icon);
            let wheel: f32 = i
                .events
                .iter()
                .map(|e| match e {
                    // Ctrl+wheel zooms an opened image instead.
                    egui::Event::MouseWheel { delta, modifiers, .. } if !(zoomable && modifiers.command) => delta.y,
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
                copy: i.events.iter().any(|e| matches!(e, egui::Event::Copy)),
                enter: i.key_pressed(Key::Enter),
                save: ctrl && i.key_pressed(Key::S),
                cancel: i.key_pressed(Key::Escape),
                undo: ctrl && !i.modifiers.shift && i.key_pressed(Key::Z),
                redo: ctrl && (i.key_pressed(Key::Y) || i.modifiers.shift && i.key_pressed(Key::Z)),
                delete: i.key_pressed(Key::Delete) || i.key_pressed(Key::Backspace),
                tool: pressed.and_then(tool_for),
                eyedropper: pressed == Some(Icon::Eyedropper),
                read: match pressed {
                    Some(Icon::ReadText) => Some(read::Kind::Text),
                    Some(Icon::ReadQr) => Some(read::Kind::Qr),
                    _ => None,
                },
                wheel,
                // F1, or ? unless a tool is on the / key.
                help: plain
                    && (i.key_pressed(Key::F1)
                        || !enabled.iter().any(|(_, key)| *key == Key::Slash)
                            && i.events.iter().any(|e| matches!(e, egui::Event::Text(t) if t == "?"))),
            }
        });

        // The discard question takes the keyboard: Enter discards, Esc goes back.
        let mut discard = false;
        let keys = if asking {
            discard = keys.enter;
            if keys.cancel {
                self.confirm_open = false;
            }
            Keys::default()
        } else {
            keys
        };

        // The shortcut sheet: F1 (or ?) opens it, and any key or click closes it without
        // doing anything else.
        let keys = if self.help_open {
            let any = ctx.input(|i| {
                i.pointer.any_released() || i.events.iter().any(|e| matches!(e, egui::Event::Key { pressed: true, .. }))
            });
            if any {
                self.help_open = false;
            }
            Keys::default()
        } else {
            if keys.help && !typing && self.editing.is_none() && !asking {
                self.help_open = true;
            }
            keys
        };

        // While typing into a text, keys go to the text. Undo, redo, copy and save
        // still work; they finish the edit first.
        let was_editing = self.editing.is_some();
        if was_editing && !typing {
            let events = ctx.input(|i| i.events.clone());
            for event in &events {
                if self.text_event(event) {
                    self.finish_editing();
                    break;
                }
            }
            if keys.undo || keys.redo {
                self.finish_editing();
                if keys.undo {
                    self.undo();
                } else {
                    self.redo();
                }
            }
        }

        // With the save panel open, Esc closes it (even while typing a name), and Ctrl+S
        // or Enter saves.
        let mut esc_used = false;
        let mut save_requested = false;
        if self.picking_color && ctx.input(|i| i.key_pressed(Key::Escape)) {
            self.picking_color = false;
            esc_used = true;
        } else if self.reading.is_some() && ctx.input(|i| i.key_pressed(Key::Escape)) {
            // Even while the text is being edited.
            self.reading = None;
            esc_used = true;
        } else if self.decor_open && !typing && ctx.input(|i| i.key_pressed(Key::Escape)) {
            self.decor_open = false;
            esc_used = true;
        } else if self.save_open {
            if ctx.input(|i| i.key_pressed(Key::Escape)) {
                self.save_open = false;
                esc_used = true;
            }
            if keys.save || keys.enter && !typing {
                save_requested = true;
            }
        } else if keys.save && !typing {
            self.save_open = true;
            self.decor_open = false;
            self.reading = None;
        }

        let mut close = false;
        if self.action.is_none() && !typing && !was_editing && !esc_used {
            if keys.undo {
                self.undo();
            } else if keys.redo {
                self.redo();
            } else if keys.delete {
                self.delete_selected();
            } else if keys.select_all {
                self.selection = Some(self.desktop.whole(self.current_monitor));
            } else if keys.nudge != Vec2::ZERO {
                // Arrows move the selected annotation if there is one, else the region.
                if let Some(index) = self.selected {
                    self.nudge_selected(index, keys.nudge);
                } else if let Some(sel) = self.selection {
                    self.selection = Some(keep_inside(sel.translate(keys.nudge), bounds));
                }
            }
            if let Some(tool) = keys.tool {
                self.set_tool(tool);
            }
            if keys.eyedropper {
                self.picking_color = !self.picking_color;
            }
            if let (Some(kind), Some(_)) = (keys.read, self.selection) {
                self.start_reading(&ctx, kind);
            }
            // Esc with a picker open closes just the picker.
            if keys.cancel && !egui::Popup::is_any_open(&ctx) {
                // Esc first lets go of the selected annotation, then closes.
                if self.selected.take().is_none() {
                    close = true;
                }
            }
        }
        if keys.wheel != 0.0 && response.hovered() {
            let range = width_range(self.style_kind());
            self.width = (self.width + keys.wheel.signum()).clamp(*range.start(), *range.end());
        }

        // Double-click a text with the select tool to edit it.
        let double = ctx.input(|i| i.pointer.button_double_clicked(egui::PointerButton::Primary));
        if double && self.tool == Tool::Select && response.hovered() && !asking
            && let Some(p) = hover.map(to_px) {
                let on_text = |a: &Annotation| a.kind == ShapeKind::Text && a.hit(p, tol);
                if let Some(index) = self.annotations.iter().rposition(on_text) {
                    self.edit_text(index, p);
                }
            }

        // --- Toolbar -------------------------------------------------------------------
        let creating_region = matches!(self.action, Some(Action::Region(Drag::Create { .. })));
        let mut toolbar = ToolbarOutput::default();
        let mut copy_text = None;
        if let (Some(sel_px), false) = (self.selection, creating_region) {
            let sel = rect_to_pt(sel_px);
            // Below the region's bottom-right corner, or tucked inside when the region
            // reaches the bottom of its monitor.
            let corner = sel_px.max - vec2(1.0, 1.0);
            let monitor = self
                .desktop
                .screens
                .iter()
                .find(|m| m.contains(corner))
                .copied()
                .unwrap_or(self.desktop.screens[self.current_monitor]);
            let monitor = rect_to_pt(monitor);
            let (pos, pivot) = if monitor.bottom() - sel.bottom() > 48.0 {
                (sel.right_bottom() + vec2(0.0, 8.0), Align2::RIGHT_TOP)
            } else {
                (sel.right_bottom() + vec2(-8.0, -8.0), Align2::RIGHT_BOTTOM)
            };
            toolbar = self.toolbar(&ctx, pos, pivot, monitor);
            // One panel at a time under the toolbar.
            if toolbar.save {
                self.save_open = !self.save_open;
                self.decor_open = false;
                self.reading = None;
            }
            if toolbar.decor {
                self.decor_open = !self.decor_open;
                self.save_open = false;
                self.reading = None;
            }
            if let Some(kind) = toolbar.read {
                // The same button again closes the panel; the other one reads anew.
                if self.reading.as_ref().is_some_and(|r| r.kind == kind) {
                    self.reading = None;
                } else {
                    self.start_reading(&ctx, kind);
                }
            }
            if let (true, Some(rect)) = (self.reading.is_some(), toolbar.rect) {
                copy_text = self.reading_panel(&ctx, rect, pivot == Align2::RIGHT_TOP, monitor);
            }
            if let (true, Some(rect)) = (self.save_open, toolbar.rect) {
                save_requested |= self.save_panel(&ctx, rect, pivot == Align2::RIGHT_TOP, monitor);
            }
            if let (true, Some(rect)) = (self.decor_open, toolbar.rect) {
                self.decor_panel(&ctx, rect, pivot == Align2::RIGHT_TOP, monitor);
            }
        } else {
            self.save_open = false;
            self.decor_open = false;
            self.reading = None;
        }

        // The hotkey strip's band: across the top of the screen, or the bottom while the
        // selection covers the top and not the bottom.
        let strip_band = {
            let m = rect_to_pt(self.desktop.screens[self.current_monitor]);
            let top = Rect::from_min_max(m.min, Pos2::new(m.right(), m.top() + STRIP_TOP + help::STRIP_HEIGHT + 6.0));
            let bottom = Rect::from_min_max(Pos2::new(m.left(), m.bottom() - STRIP_BOTTOM - help::STRIP_HEIGHT - 6.0), m.max);
            match self.selection.map(rect_to_pt) {
                Some(sel) if sel.intersects(top) && !sel.intersects(bottom) => bottom,
                _ => top,
            }
        };

        // --- Tool help box ---------------------------------------------------------------
        if let (Some(sel_px), false) = (self.selection, creating_region) {
            let id = if self.picking_color {
                Some("eyedropper")
            } else if let Some(reading) = &self.reading {
                Some(match reading.kind {
                    read::Kind::Text => "read-text",
                    read::Kind::Qr => "read-qr",
                })
            } else {
                TOOLS.iter().find(|t| tool_for(t.icon) == Some(self.tool)).map(|t| t.id)
            };
            let show = self.help_level != HelpLevel::Off && !self.confirm_open && !self.help_open;
            if let Some(info) = id.and_then(help::tool_help).filter(|_| show) {
                let key = self.tools.iter().find(|(t, _)| Some(t.id) == id).map(|(_, k)| crate::keys::label(*k));
                let centre = sel_px.center();
                let screen_px = self
                    .desktop
                    .screens
                    .iter()
                    .find(|m| m.contains(centre))
                    .copied()
                    .unwrap_or(self.desktop.screens[self.current_monitor]);
                let monitor = rect_to_pt(screen_px);
                let sel = rect_to_pt(sel_px);
                // Keep clear of the toolbar, the round handle and any open panel.
                let mut avoid: Vec<Rect> = toolbar.rect.into_iter().collect();
                if let Some(handle) = self.rotate_handle(sel_px, tol) {
                    let c = to_pt(handle);
                    let edge = c.y.clamp(sel.top(), sel.bottom());
                    avoid.push(Rect::from_two_pos(c, Pos2::new(c.x, edge)).expand(20.0));
                }
                for (open, name) in [
                    (self.save_open, "duskshot-save-panel"),
                    (self.decor_open, "duskshot-decor-panel"),
                    (self.reading.is_some(), "duskshot-reading-panel"),
                ] {
                    if let Some(r) = ctx.memory(|m| m.area_rect(egui::Id::unique(name))).filter(|_| open) {
                        avoid.push(r);
                    }
                }
                avoid.push(strip_band);
                avoid.extend(self.strip_rect);
                let floor = monitor.bottom();
                let (at, docked) = help::place_box(self.help_box_size, sel, monitor, &avoid, floor, hover);
                self.help_box_size = help::tool_box(&ctx, &info, key.as_deref(), self.help_level, at, docked);
            }
        }
        self.restyle_selected();

        // --- Paint: annotations, dimming, region outline and handles, labels -----------
        let uv = Rect::from_min_max(Pos2::ZERO, Pos2::new(1.0, 1.0));

        // Blur regions first: they act on the capture, below every other annotation.
        // Redactions go last, as in the output.
        let mut used = HashSet::new();
        for i in self.blur_order() {
            let annotation = &self.annotations[i];
            let key = layer_key(annotation);
            if !self.blur_layers.contains_key(&key)
                && let Some((img, area)) = self.blurred(annotation) {
                    let size = [img.width() as usize, img.height() as usize];
                    let image = egui::ColorImage::from_rgba_unmultiplied(size, img.as_raw());
                    let texture = ctx.load_texture("blur", image, egui::TextureOptions::NEAREST);
                    self.blur_layers.insert(key, (texture, area));
                }
            if let Some((texture, area)) = self.blur_layers.get(&key) {
                painter.image(texture.id(), rect_to_pt(*area), uv, Color32::WHITE);
                used.insert(key);
            }
        }
        self.blur_layers.retain(|key, _| used.contains(key));

        let drawing = matches!(self.action, Some(Action::NewShape { .. }));
        let last = self.annotations.len().wrapping_sub(1);
        let mut used = HashSet::new();
        for (i, annotation) in self.annotations.iter().enumerate() {
            if annotation.kind == ShapeKind::Blur {
                continue;
            }
            // Freehand strokes and text are shown from rasterised layers, exactly as
            // they will be saved. The stroke being drawn changes every frame, so it is
            // painted directly.
            let layered = annotation.is_freehand() || matches!(annotation.kind, ShapeKind::Text | ShapeKind::Step | ShapeKind::Emoji);
            // A stamp has nothing to paint directly: it is drawn from its layer even
            // while being placed.
            if layered && !(drawing && i == last && annotation.kind != ShapeKind::Emoji) {
                let key = layer_key(annotation);
                if !self.layers.contains_key(&key)
                    && let Some((image, area)) = annotation.layer(bounds) {
                        let texture = ctx.load_texture("stroke", image, egui::TextureOptions::LINEAR);
                        self.layers.insert(key, (texture, area));
                    }
                if let Some((texture, area)) = self.layers.get(&key) {
                    painter.image(texture.id(), rect_to_pt(*area), uv, Color32::WHITE);
                    used.insert(key);
                    continue;
                }
            }
            annotation.paint(&painter, to_pt, scale);
        }
        self.layers.retain(|key, _| used.contains(key));

        // The stamp tool shows the stamp under the pointer, at its size, before it is
        // placed: the wheel resizes it there. Faded, so it is clearly not placed yet.
        let stamp_at = hover.filter(|_| {
            self.tool == Tool::Shape(ShapeKind::Emoji)
                && self.action.is_none()
                && response.hovered()
                && !egui::Popup::is_any_open(&ctx)
        });
        if let Some(p) = stamp_at.map(to_px).filter(|&p| matches!(self.pick_action(p, tol), Action::NewShape { .. })) {
            let key = (self.emoji.clone(), self.width.to_bits(), self.color);
            if self.emoji_preview.as_ref().is_none_or(|(k, _, _)| *k != key) {
                self.emoji_preview = self.emoji_preview_layer().map(|(image, area)| {
                    (key, ctx.load_texture("stamp-preview", image, egui::TextureOptions::LINEAR), area)
                });
            }
            if let Some((_, texture, area)) = &self.emoji_preview {
                let shown = rect_to_pt(area.translate(p.to_vec2()));
                painter.image(texture.id(), shown, uv, Color32::WHITE.gamma_multiply(0.7));
            }
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
                if self.border_on {
                    // The border as it will be added: just outside the region.
                    let b = self.border;
                    let band = sel_px.expand(b.width.round().max(1.0) / 2.0);
                    Annotation::stroked(ShapeKind::Rect, band.min, band.max, b.color, b.width.round().max(1.0), b.dash)
                        .paint(&painter, to_pt, scale);
                } else {
                    painter.rect_stroke(sel, 0.0, Stroke::new(1.0, Color32::WHITE), StrokeKind::Outside);
                }
                let turning = matches!(self.action, Some(Action::Rotate { .. }));
                if turning {
                    rotation_guides(&painter, sel, &self.guides);
                }
                for point in handle_points(sel) {
                    handle(&painter, point, Color32::WHITE);
                }
                // The round handle that turns the capture, on a stem from the region.
                if let Some(knob) = self.rotate_handle(sel_px, tol).map(to_pt) {
                    // From the nearest point of the region's edge; inside it, from the
                    // nearer of top and bottom.
                    let stem = if sel.contains(knob) {
                        Pos2::new(knob.x, if knob.y < sel.center().y { sel.top() } else { sel.bottom() })
                    } else {
                        sel.clamp(knob)
                    };
                    painter.line_segment([stem, knob], Stroke::new(1.0, Color32::WHITE));
                    let fill = if turning { ACCENT } else { Color32::WHITE };
                    painter.circle_filled(knob, ROTATE_HANDLE_RADIUS, fill);
                    painter.circle_stroke(knob, ROTATE_HANDLE_RADIUS, Stroke::new(1.0, Color32::from_black_alpha(160)));
                    rotate_glyph(&painter, knob, ROTATE_HANDLE_RADIUS * 0.55, Color32::from_gray(40));
                    if turning || !self.rotation.is_none() {
                        let degrees = format!("{}°", self.rotation.degrees());
                        let at = knob + vec2(ROTATE_HANDLE_RADIUS + 6.0, 0.0);
                        label(&painter, at, Align2::LEFT_CENTER, degrees, Color32::WHITE);
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

        // With Ctrl held over a spot where a click would start a new region, outline the
        // window a click would select. Only before an area is chosen: afterwards Ctrl is
        // mostly pressed for shortcuts (Ctrl+C, Ctrl+S), and the outline would flash.
        if mods.ctrl && self.selection.is_none() && self.action.is_none() && self.rotation.is_none() && response.hovered()
            && let Some(p) = hover.map(to_px)
                && let Action::Region(Drag::Create { .. }) = self.pick_action(p, tol)
                    && let Some(window) = self.desktop.snap.window_at(p) {
                        let window = window.intersect(bounds);
                        let r = rect_to_pt(window);
                        painter.rect_filled(r, 0.0, ACCENT.gamma_multiply(0.12));
                        painter.rect_stroke(r, 0.0, Stroke::new(2.0, ACCENT), StrokeKind::Inside);
                        let size = format!("{} x {}", window.width() as u32, window.height() as u32);
                        label(&painter, r.left_top() + vec2(8.0, 8.0), Align2::LEFT_TOP, size, Color32::WHITE);
                    }

        self.paint_reading(&painter, to_pt, read_hover, mods.shift);

        // Handles of the selected annotation, in an accent colour to tell them apart
        // from the region's.
        if let Some(shape) = self.selected.map(|i| &self.annotations[i]) {
            if shape.kind == ShapeKind::Text {
                let r = rect_to_pt(shape.path_bounds().expand(3.0));
                painter.rect_stroke(r, 2.0, Stroke::new(1.0, ACCENT), StrokeKind::Middle);
            } else if shape.is_freehand() {
                let r = rect_to_pt(shape.path_bounds().expand(shape.width / 2.0 + 3.0));
                painter.rect_stroke(r, 2.0, Stroke::new(1.0, ACCENT), StrokeKind::Middle);
            } else if shape.is_box() {
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

        if let Some(edit) = &self.editing {
            let shape = &self.annotations[edit.index];
            let layout = Layout::new(&shape.text, shape.width);
            let top = to_pt(shape.a + layout.caret(edit.cursor).to_vec2());
            let caret = [top, top + vec2(0.0, layout.line_height / scale)];
            painter.line_segment(caret, Stroke::new(1.5, ACCENT));
        }

        // The eyedropper: a spotlight of the true, undimmed capture around the cursor
        // (magnified while Shift is held) and the values of the colour under it.
        if self.picking_color {
            ctx.set_cursor_icon(CursorIcon::Crosshair);
            if let Some(p) = hover {
                let px = to_px(p);
                if let Some(color) = self.pixel_at(px) {
                    // Desktop pixels per point inside the spotlight.
                    let density = if mods.shift { 1.0 / SPOTLIGHT_ZOOM } else { scale };
                    // Spotlight point to desktop pixel, and back. The centre of the pixel
                    // a click picks sits under the cursor (the cursor itself points at a
                    // pixel's corner), so the magnified pixel is centred on the crosshair.
                    // Unmagnified, the spotlight lines up with the capture around it
                    // instead: shifted by half a pixel, nearest-pixel sampling would land
                    // on pixel edges and draw some pixels twice and skip others.
                    let centre = if mods.shift { Pos2::new(px.x.floor() + 0.5, px.y.floor() + 0.5) } else { px };
                    let to_desktop = |q: Pos2| centre + (q - p) * density;
                    let from_desktop = |d: Pos2| p + (d - centre) / density;
                    painter.circle_filled(p, SPOTLIGHT_RADIUS, Color32::BLACK);
                    let rotation = self.rotation;
                    for (m, texture) in self.desktop.monitors.iter().zip(&self.textures) {
                        let uv = |q: Pos2| {
                            let d = rotation.to_source(to_desktop(q));
                            Pos2::new((d.x - m.min.x) / m.width(), (d.y - m.min.y) / m.height())
                        };
                        if !rotation.is_none() {
                            // Turned, monitor edges are not straight on screen: the monitor
                            // under the cursor fills the spotlight.
                            if m.contains(rotation.to_source(px)) {
                                painter.add(spotlight_mesh(texture.id(), p, SPOTLIGHT_RADIUS, uv));
                            }
                            continue;
                        }
                        let area = Rect::from_min_max(from_desktop(m.min), from_desktop(m.max));
                        if !area.expand(SPOTLIGHT_RADIUS).contains(p) {
                            continue;
                        }
                        painter.with_clip_rect(area).add(spotlight_mesh(texture.id(), p, SPOTLIGHT_RADIUS, uv));
                    }
                    if mods.shift {
                        // Outline the pixel that a click picks.
                        let pixel = from_desktop(Pos2::new(px.x.floor(), px.y.floor()));
                        let r = Rect::from_min_size(pixel, Vec2::splat(SPOTLIGHT_ZOOM));
                        painter.rect_stroke(r, 0.0, Stroke::new(1.0, Color32::WHITE), StrokeKind::Inside);
                        painter.rect_stroke(r, 0.0, Stroke::new(1.0, Color32::BLACK), StrokeKind::Outside);
                    }
                    painter.circle_stroke(p, SPOTLIGHT_RADIUS, Stroke::new(2.0, Color32::WHITE));
                    painter.circle_stroke(p, SPOTLIGHT_RADIUS + 1.5, Stroke::new(1.0, Color32::from_black_alpha(160)));
                    color_readout(&painter, p, SPOTLIGHT_RADIUS, color, screen);
                }
            }
        }

        let reading_lines = self.reading.as_ref().is_some_and(|r| !r.lines().is_empty());
        let monitor = self.desktop.screens[self.current_monitor];
        let at_top = strip_band.top() <= to_pt(monitor.min).y + 0.5;
        let bottom = if at_top {
            Pos2::new(strip_band.center().x, strip_band.top() + STRIP_TOP)
        } else {
            Pos2::new(strip_band.center().x, strip_band.bottom() - STRIP_BOTTOM)
        };
        // At most 95% of the screen wide; longer, it wraps onto more rows.
        let max_width = rect_to_pt(monitor).width() * 0.95;
        let strip = help::strip(&painter, bottom, at_top, max_width, &self.strip_entries(reading_lines));
        self.strip_rect = Some(strip);
        // The round handle moves out from under the strip; it was placed before the
        // strip was drawn, so a change shows on the next frame.
        let strip_px = Some(Rect::from_two_pos(to_px(strip.min), to_px(strip.max)));
        let toolbar_px = toolbar.rect.map(|r| Rect::from_two_pos(to_px(r.min), to_px(r.max)));
        if (strip_px, toolbar_px) != (self.strip_px, self.toolbar_px) {
            self.strip_px = strip_px;
            self.toolbar_px = toolbar_px;
            ctx.request_repaint();
        }
        if let Some(err) = &self.error {
            let (pos, align) = if at_top {
                (Pos2::new(bottom.x, strip.bottom() + 8.0), Align2::CENTER_TOP)
            } else {
                (Pos2::new(bottom.x, strip.top() - 8.0), Align2::CENTER_BOTTOM)
            };
            label(&painter, pos, align, err.clone(), Color32::LIGHT_RED);
        }
        // Hovering the round handle says what it does.
        if let (None, false, Some(p), Some(sel)) = (&self.action, asking || self.help_open, hover, self.selection)
            && let Some(handle) = self.rotate_handle(sel, tol)
                && to_px(p).distance(handle) <= (ROTATE_HANDLE_RADIUS + 2.0) * tol / GRAB_TOLERANCE {
                    let tip = "Drag to turn the capture · Shift: 15° steps · right-click: reset".to_owned();
                    label(&painter, p + vec2(16.0, 20.0), Align2::LEFT_TOP, tip, Color32::WHITE);
                }
        if self.help_open {
            let tools: Vec<(&'static str, String)> =
                self.tools.iter().map(|(t, k)| (t.name, crate::keys::label(*k))).collect();
            help::sheet(&ctx, rect_to_pt(monitor), &tools);
        }

        // --- Actions -------------------------------------------------------------------
        let enter_copies = keys.enter && !was_editing && !self.save_open && self.reading.is_none();
        let copy = toolbar.copy || (keys.copy || enter_copies) && !typing;
        let save = save_requested && self.save_open;
        if copy || save {
            self.finish_editing();
        }
        if self.confirm_open {
            let monitor = rect_to_pt(self.desktop.screens[self.current_monitor]);
            match self.confirm_panel(&ctx, monitor) {
                Some(true) => discard = true,
                Some(false) => self.confirm_open = false,
                None => {}
            }
        }
        if discard || (close || toolbar.close) && self.request_close() {
            self.leave(&ctx);
        } else if let Some(text) = copy_text {
            match Self::copy_text_to_clipboard(&text) {
                Ok(()) => self.leave(&ctx),
                Err(e) => self.error = Some(format!("Failed: {e}")),
            }
        } else if copy || save {
            let result = if copy {
                match self.output() {
                    Some(img) => Self::copy_to_clipboard(&img).map(|()| true),
                    None => Ok(false),
                }
            } else {
                self.save_to_file(frame)
            };
            match result {
                Ok(true) => self.leave(&ctx),
                Ok(false) => {} // save dialog cancelled: stay in the overlay
                Err(e) => self.error = Some(format!("Failed: {e}")),
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

    pub(super) fn rect(x0: f32, y0: f32, x1: f32, y1: f32) -> Rect {
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
    fn highlighter_keeps_its_own_colour_and_width() {
        let desktop = Desktop::new(
            vec![RgbaImage::new(4, 4)],
            RECT { left: 0, top: 0, right: 4, bottom: 4 },
            vec![rect(0.0, 0.0, 4.0, 4.0)],
            0,
            Snap::default(),
        );
        let mut overlay = Overlay::with_desktop(desktop, None, &Settings::default());
        let marker = Tool::Shape(ShapeKind::Marker);
        let rect_tool = Tool::Shape(ShapeKind::Rect);

        overlay.set_tool(rect_tool);
        assert_eq!((overlay.color, overlay.width), (SWATCHES[0], 4.0));
        overlay.set_tool(marker);
        assert_eq!((overlay.color, overlay.width), (MARKER_COLOR, MARKER_WIDTH));

        // Changes on either side are remembered.
        overlay.color = SWATCHES[1];
        overlay.width = 24.0;
        overlay.set_tool(Tool::Shape(ShapeKind::Pen));
        assert_eq!((overlay.color, overlay.width), (SWATCHES[0], 4.0));
        overlay.width = 6.0;
        overlay.set_tool(marker);
        assert_eq!((overlay.color, overlay.width), (SWATCHES[1], 24.0));
        overlay.set_tool(Tool::Select);
        assert_eq!(overlay.color, SWATCHES[0]);
        // Each tool keeps its own size, even tools that share a colour.
        overlay.set_tool(rect_tool);
        assert_eq!(overlay.width, 4.0);
        overlay.set_tool(Tool::Shape(ShapeKind::Pen));
        assert_eq!(overlay.width, 6.0);
    }

    #[test]
    fn size_does_not_carry_over_from_text_or_a_selected_shape() {
        let mut overlay = plain_overlay(200, 200, RgbaImage::new(200, 200));
        let arrow = Tool::Shape(ShapeKind::Arrow);
        overlay.set_tool(arrow);
        overlay.width = 3.0;
        overlay.set_tool(Tool::Shape(ShapeKind::Text));
        assert_eq!(overlay.width, TEXT_SIZE);
        overlay.width = 40.0;
        overlay.set_tool(arrow);
        assert_eq!(overlay.width, 3.0);
        // Selecting a big text with the Select tool does not change the arrow's size.
        overlay.set_tool(Tool::Select);
        overlay.width = 40.0;
        overlay.set_tool(arrow);
        assert_eq!(overlay.width, 3.0);
    }

    pub(super) fn plain_overlay(w: u32, h: u32, shot: RgbaImage) -> Overlay {
        let desktop = Desktop::new(
            vec![shot],
            RECT { left: 0, top: 0, right: w as i32, bottom: h as i32 },
            vec![rect(0.0, 0.0, w as f32, h as f32)],
            0,
            Snap::default(),
        );
        Overlay::with_desktop(desktop, None, &Settings::default())
    }

    #[test]
    fn large_images_are_placed_to_fit_in_their_own_pixels() {
        // Small: at 1:1, centred, the space is the screen.
        let (space, at) = place_image(vec2(800.0, 600.0), vec2(1920.0, 1080.0));
        assert_eq!(space, vec2(1920.0, 1080.0));
        assert_eq!(at, rect(560.0, 240.0, 1360.0, 840.0));
        // A 4000 x 3000 photo: the space grows to fit it with room round it, so it
        // shows scaled down, and it keeps every one of its pixels.
        let (space, at) = place_image(vec2(4000.0, 3000.0), vec2(1920.0, 1080.0));
        assert!((space.y * 0.85 - 3000.0).abs() < 1.0 && (space.x / space.y - 1920.0 / 1080.0).abs() < 1e-3, "{space:?}");
        assert_eq!(at.size(), vec2(4000.0, 3000.0));
        assert!(rect(0.0, 0.0, space.x, space.y).contains_rect(at));
    }

    #[test]
    fn an_opened_image_starts_selected_and_saves_beside_itself() {
        let photo = RgbaImage::from_fn(3000, 2000, |x, y| image::Rgba([(x % 251) as u8, (y % 241) as u8, 9, 255]));
        let opened = Opened { stem: Some("receipt".into()), folder: Some(r"C:\photos".into()), format: Some(Format::Jpeg) };
        let monitor = RECT { left: 1920, top: 0, right: 3840, bottom: 1080 };
        let overlay = Overlay::with_desktop(Desktop::open_on(photo.clone(), opened, monitor), None, &Settings::default());
        // The whole image is selected, and copied or saved at its own size.
        assert_eq!(overlay.selection, Some(overlay.desktop.monitors[0]));
        assert_eq!(overlay.output().unwrap(), photo);
        assert_eq!(overlay.save_options.file_name(), "receipt-edited.jpg");
        assert_eq!(overlay.save_options.folder.as_deref(), Some(std::path::Path::new(r"C:\photos")));
        // The window covers just that monitor; Ctrl+A selects the image, not the screen.
        assert_eq!((overlay.desktop.bounds.left, overlay.desktop.bounds.right), (1920, 3840));
        assert_eq!(overlay.desktop.whole(0), overlay.desktop.monitors[0]);
    }

    #[test]
    fn opens_image_files() {
        let dir = std::env::temp_dir().join(format!("duskshot-open-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let png = dir.join("shot.png");
        let img = RgbaImage::from_fn(30, 20, |x, y| image::Rgba([x as u8 * 8, y as u8 * 12, 200, 255]));
        img.save(&png).unwrap();
        let desktop = open_file(&png).unwrap();
        assert_eq!(desktop.shots[0], img);
        assert_eq!(
            desktop.opened,
            Some(Opened { stem: Some("shot".into()), folder: Some(dir.clone()), format: Some(Format::Png) })
        );
        // Not an image: says which formats it opens.
        let text = dir.join("notes.png");
        std::fs::write(&text, "not a picture").unwrap();
        let error = open_file(&text).err().unwrap().to_string();
        assert!(error.contains("PNG and JPEG"), "{error}");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn rotation_maps_both_ways() {
        let pivot = Pos2::new(50.0, 30.0);
        let r = Rotation::NONE.turned(0.7, pivot).turned(-0.2, Pos2::new(10.0, 80.0));
        for p in [Pos2::ZERO, Pos2::new(13.0, -4.0), Pos2::new(200.0, 90.0)] {
            assert!(r.to_source(r.to_desktop(p)).distance(p) < 1e-3, "{p:?}");
        }
        assert!((r.angle - 0.5).abs() < 1e-5);
        // The pivot stays put.
        let once = Rotation::NONE.turned(1.1, pivot);
        assert!(once.to_desktop(pivot).distance(pivot) < 1e-3);
        // Turned back to nothing, the capture is where it was taken.
        assert!(once.turned(-1.1, Pos2::new(400.0, 7.0)).is_none());
        assert_eq!(Rotation::NONE.turned(std::f32::consts::PI, pivot).degrees(), 180);
        assert_eq!(Rotation::NONE.turned(-0.5f32.to_radians() - std::f32::consts::FRAC_PI_2, pivot).degrees(), -91);
    }

    #[test]
    fn a_turned_capture_is_cropped_straightened() {
        let shot = RgbaImage::from_fn(40, 40, |x, y| image::Rgba([(x * 6) as u8, (y * 6) as u8, ((x * y) % 251) as u8, 255]));
        let mut overlay = plain_overlay(40, 40, shot.clone());
        let sel = rect(10.0, 10.0, 30.0, 30.0);
        overlay.selection = Some(sel);
        // A quarter turn clockwise around the region's centre, as by the handle.
        overlay.rotation = Rotation::NONE.turned(std::f32::consts::FRAC_PI_2, sel.center());
        let want = image::imageops::rotate90(&*image::imageops::crop_imm(&shot, 10, 10, 20, 20));
        assert_eq!(overlay.cropped().unwrap(), want);
        // The eyedropper sees the same.
        let [r, g, b, _] = want.get_pixel(3, 5).0;
        assert_eq!(overlay.pixel_at(Pos2::new(13.5, 15.5)), Some(Color32::from_rgb(r, g, b)));
        // Turned so that some of the region comes from off the capture: transparent.
        overlay.selection = Some(rect(0.0, 0.0, 40.0, 40.0));
        overlay.rotation = Rotation::NONE.turned(0.6, Pos2::new(20.0, 20.0));
        let img = overlay.cropped().unwrap();
        assert_eq!(img.get_pixel(0, 0).0[3], 0);
        assert_eq!(img.get_pixel(20, 20).0[3], 255);
    }

    #[test]
    fn the_handle_turns_the_capture_before_anything_is_drawn() {
        let mut overlay = plain_overlay(400, 300, RgbaImage::new(400, 300));
        let sel = rect(100.0, 100.0, 300.0, 200.0);
        overlay.selection = Some(sel);
        let knob = overlay.rotate_handle(sel, GRAB_TOLERANCE).unwrap();
        assert_eq!(knob, Pos2::new(200.0, 100.0 - ROTATE_HANDLE_GAP));
        let action = overlay.pick_action(knob, GRAB_TOLERANCE);
        assert!(matches!(action, Action::Rotate { .. }));

        // Dragged round to nearly a quarter turn clockwise: stays there, no snapping.
        overlay.begin_action(action);
        let bounds = overlay.bounds();
        let centre = sel.center();
        let nearly = 88f32.to_radians() - std::f32::consts::FRAC_PI_2;
        overlay.apply_action(centre + vec2(nearly.cos(), nearly.sin()) * 80.0, Modifiers::NONE, bounds);
        assert_eq!(overlay.rotation.degrees(), 88);
        assert!((overlay.rotation.angle - 88f32.to_radians()).abs() < 1e-4);
        // Shift: 15-degree steps. 37 degrees becomes 30.
        let a = 37f32.to_radians() - std::f32::consts::FRAC_PI_2;
        overlay.apply_action(centre + vec2(a.cos(), a.sin()) * 80.0, Modifiers::SHIFT, bounds);
        assert_eq!(overlay.rotation.degrees(), 30);
        // A small turn stays small.
        let a = 1f32.to_radians() - std::f32::consts::FRAC_PI_2;
        overlay.apply_action(centre + vec2(a.cos(), a.sin()) * 80.0, Modifiers::NONE, bounds);
        assert_eq!(overlay.rotation.degrees(), 1);
        // Back where it started (Shift makes that easy): no turn at all.
        overlay.apply_action(centre + vec2(0.0, -80.0), Modifiers::SHIFT, bounds);
        assert!(overlay.rotation.is_none());
        overlay.end_action();

        // With a region at the very top, the handle goes below it.
        assert_eq!(overlay.rotate_handle(rect(100.0, 0.0, 300.0, 50.0), GRAB_TOLERANCE), Some(Pos2::new(200.0, 50.0 + ROTATE_HANDLE_GAP)));
        // Likewise when the hotkey strip covers the spot above.
        overlay.strip_px = Some(rect(0.0, 0.0, 400.0, 80.0));
        assert_eq!(overlay.rotate_handle(sel, GRAB_TOLERANCE), Some(Pos2::new(200.0, 200.0 + ROTATE_HANDLE_GAP)));
        // With the toolbar covering the spot below too, beside the region.
        overlay.toolbar_px = Some(rect(50.0, 208.0, 300.0, 250.0));
        assert_eq!(overlay.rotate_handle(sel, GRAB_TOLERANCE), Some(Pos2::new(300.0 + ROTATE_HANDLE_GAP, 150.0)));
        // A region filling the screen, the strip over its top: just inside the bottom.
        overlay.toolbar_px = Some(rect(250.0, 250.0, 392.0, 292.0));
        let full = rect(0.0, 0.0, 400.0, 300.0);
        assert_eq!(overlay.rotate_handle(full, GRAB_TOLERANCE), Some(Pos2::new(200.0, 300.0 - ROTATE_HANDLE_GAP)));
        overlay.strip_px = None;
        overlay.toolbar_px = None;

        // With things drawn, the handle stays, for small corrections; turning leaves
        // the drawings where they are.
        let mark = Annotation::stroked(ShapeKind::Rect, sel.min + vec2(20.0, 20.0), sel.center(), Color32::RED, 2.0, Dash::Solid);
        overlay.annotations.push(mark.clone());
        let action = overlay.pick_action(knob, GRAB_TOLERANCE);
        assert!(matches!(action, Action::Rotate { .. }));
        overlay.begin_action(action);
        let a = 4f32.to_radians() - std::f32::consts::FRAC_PI_2;
        overlay.apply_action(centre + vec2(a.cos(), a.sin()) * 80.0, Modifiers::NONE, bounds);
        overlay.end_action();
        assert_eq!(overlay.rotation.degrees(), 4);
        assert_eq!(overlay.annotations, vec![mark]);
    }

    /// With the reading panel open, moving the region or turning the capture reads it
    /// again; a click that changes nothing does not.
    #[test]
    fn moving_or_turning_reads_again() {
        let mut overlay = plain_overlay(400, 300, RgbaImage::new(400, 300));
        let sel = rect(100.0, 100.0, 300.0, 200.0);
        overlay.selection = Some(sel);
        let reading = || Reading {
            kind: read::Kind::Qr,
            origin: Pos2::new(100.0, 100.0),
            pending: None,
            outcome: Some(Err("No QR code found in the region.".to_owned())),
            picked: HashSet::new(),
            text: String::new(),
        };
        overlay.reading = Some(reading());
        let bounds = overlay.bounds();

        overlay.begin_action(overlay.pick_action(sel.center(), GRAB_TOLERANCE));
        overlay.end_action();
        assert!(!overlay.reread, "a click is not a move");

        overlay.begin_action(overlay.pick_action(sel.center(), GRAB_TOLERANCE));
        overlay.apply_action(sel.center() + vec2(20.0, 5.0), Modifiers::NONE, bounds);
        overlay.end_action();
        assert!(std::mem::take(&mut overlay.reread));

        let knob = overlay.rotate_handle(overlay.selection.unwrap(), GRAB_TOLERANCE).unwrap();
        overlay.begin_action(overlay.pick_action(knob, GRAB_TOLERANCE));
        overlay.apply_action(knob + vec2(15.0, 0.0), Modifiers::NONE, bounds);
        overlay.end_action();
        assert!(std::mem::take(&mut overlay.reread));

        // Without the panel open, nothing is read.
        overlay.reading = None;
        overlay.begin_action(overlay.pick_action(knob, GRAB_TOLERANCE));
        overlay.apply_action(knob + vec2(-15.0, 0.0), Modifiers::NONE, bounds);
        overlay.end_action();
        assert!(!overlay.reread);
    }

    #[test]
    fn right_click_on_the_handle_resets_the_turn() {
        let mut overlay = plain_overlay(400, 300, RgbaImage::new(400, 300));
        overlay.selection = Some(rect(100.0, 100.0, 300.0, 200.0));
        overlay.rotation = Rotation::NONE.turned(0.3, Pos2::new(200.0, 150.0));
        overlay.reset_rotation();
        assert!(overlay.rotation.is_none() && !overlay.reread);
        // With the reading panel open, the region is read again.
        overlay.rotation = Rotation::NONE.turned(0.3, Pos2::new(200.0, 150.0));
        overlay.reading = Some(Reading {
            kind: read::Kind::Text,
            origin: Pos2::ZERO,
            pending: None,
            outcome: None,
            picked: HashSet::new(),
            text: String::new(),
        });
        overlay.reset_rotation();
        assert!(overlay.rotation.is_none() && overlay.reread);
    }

    /// Right-click backs out one step at a time: a panel, then the selection, then the
    /// tool. It never removes anything or closes the capture.
    #[test]
    fn right_click_backs_out_one_step_at_a_time() {
        let ctx = egui::Context::default();
        let mut overlay = plain_overlay(400, 300, RgbaImage::new(400, 300));
        overlay.selection = Some(rect(100.0, 100.0, 300.0, 200.0));
        overlay.annotations.push(Annotation::stroked(ShapeKind::Rect, Pos2::new(120.0, 120.0), Pos2::new(160.0, 160.0), Color32::RED, 3.0, Dash::Solid));
        overlay.set_tool(Tool::Shape(ShapeKind::Arrow));
        overlay.selected = Some(0);
        overlay.save_open = true;
        overlay.back_out(&ctx);
        assert!(!overlay.save_open && overlay.selected == Some(0) && overlay.tool == Tool::Shape(ShapeKind::Arrow));
        overlay.back_out(&ctx);
        assert!(overlay.selected.is_none() && overlay.tool == Tool::Shape(ShapeKind::Arrow));
        overlay.back_out(&ctx);
        assert_eq!(overlay.tool, Tool::Select);
        overlay.back_out(&ctx);
        assert_eq!(overlay.tool, Tool::Select);
        assert_eq!(overlay.annotations.len(), 1);
        assert!(overlay.selection.is_some());
    }

    /// A blur region blurs what is under it after a turn, not what was there before.
    #[test]
    fn blur_follows_the_turned_capture() {
        // Left half black, right half white.
        let shot = RgbaImage::from_fn(100, 100, |x, _| image::Rgba(if x < 50 { [0, 0, 0, 255] } else { [255; 4] }));
        let mut overlay = plain_overlay(100, 100, shot);
        overlay.selection = Some(rect(0.0, 0.0, 100.0, 100.0));
        let mut blur = Annotation::stroked(ShapeKind::Blur, Pos2::new(10.0, 40.0), Pos2::new(30.0, 60.0), Color32::BLACK, 4.0, Dash::Solid);
        blur.effect = Effect::Pixelate;
        overlay.annotations.push(blur.clone());
        assert_eq!(overlay.blurred(&blur).unwrap().0.get_pixel(5, 5).0, [0, 0, 0, 255]);
        // Half a turn: the white half is now on the left, under the blur.
        overlay.rotation = Rotation::NONE.turned(std::f32::consts::PI, Pos2::new(50.0, 50.0));
        assert_eq!(overlay.blurred(&blur).unwrap().0.get_pixel(5, 5).0, [255; 4]);
        assert_eq!(overlay.output().unwrap().get_pixel(20, 50).0, [255; 4]);
    }

    #[test]
    fn disabled_tools_leave_the_toolbar() {
        let mut settings = Settings::default();
        settings.set_tool_enabled("blur", false);
        settings.set_tool_enabled("eyedropper", false);
        let desktop = Desktop::new(
            vec![RgbaImage::new(10, 10)],
            RECT { left: 0, top: 0, right: 10, bottom: 10 },
            vec![rect(0.0, 0.0, 10.0, 10.0)],
            0,
            Snap::default(),
        );
        let overlay = Overlay::with_desktop(desktop, None, &settings);
        let ids: Vec<&str> = overlay.tools.iter().map(|(t, _)| t.id).collect();
        assert_eq!(ids.len(), TOOLS.len() - 2);
        assert!(!ids.contains(&"blur") && !ids.contains(&"eyedropper") && ids.contains(&"pen"));
        // Every tool but the eyedropper and the readers is one you draw or select with.
        let actions = [Icon::Eyedropper, Icon::ReadText, Icon::ReadQr];
        assert!(TOOLS.iter().all(|t| tool_for(t.icon).is_some() == !actions.contains(&t.icon)));
    }

    #[test]
    fn custom_tool_keys_and_emoji_stamps() {
        let mut settings = Settings::default();
        settings.set_tool_key(&TOOLS[1], Some(Key::K));
        let desktop = Desktop::new(
            vec![RgbaImage::new(10, 10)],
            RECT { left: 0, top: 0, right: 10, bottom: 10 },
            vec![rect(0.0, 0.0, 10.0, 10.0)],
            0,
            Snap::default(),
        );
        let overlay = Overlay::with_desktop(desktop, None, &settings);
        let key_of = |id: &str| overlay.tools.iter().find(|(t, _)| t.id == id).map(|(_, k)| *k);
        assert_eq!((key_of("rect"), key_of("pen")), (Some(Key::K), Some(Key::P)));
        // The strip shows the chosen keys.
        let mut with_region = Overlay::with_desktop(
            Desktop::new(
                vec![RgbaImage::new(10, 10)],
                RECT { left: 0, top: 0, right: 10, bottom: 10 },
                vec![rect(0.0, 0.0, 10.0, 10.0)],
                0,
                Snap::default(),
            ),
            None,
            &settings,
        );
        with_region.selection = Some(rect(0.0, 0.0, 10.0, 10.0));
        let strip = with_region.strip_entries(false);
        let keys: Vec<&str> = strip[0].iter().map(|e| e.keys[0].as_str()).collect();
        assert_eq!(&keys[..3], ["V", "K", "E"]);

        if !crate::emoji::available() {
            return;
        }
        let mut overlay = plain_overlay(200, 200, RgbaImage::new(200, 200));
        overlay.selection = Some(rect(0.0, 0.0, 200.0, 200.0));
        overlay.set_tool(Tool::Shape(ShapeKind::Emoji));
        assert_eq!(overlay.width, EMOJI_SIZE);
        overlay.emoji = "🔥".into();
        overlay.begin_action(Action::NewShape { anchor: Pos2::new(50.0, 50.0) });
        overlay.apply_action(Pos2::new(80.0, 70.0), Modifiers::NONE, overlay.bounds());
        overlay.end_action();
        // A click places the stamp where the pointer was let go, and leaves it
        // unselected, so the next emoji or size is for the next stamp.
        let stamp = &overlay.annotations[0];
        assert_eq!((stamp.text.as_str(), stamp.a), ("🔥", Pos2::new(80.0, 70.0)));
        assert_eq!(overlay.selected, None);
        overlay.width = 90.0;
        overlay.choose_emoji("❌".into());
        overlay.restyle_selected();
        assert_eq!((overlay.annotations[0].text.as_str(), overlay.annotations[0].width), ("🔥", EMOJI_SIZE));
        // The preview is the stamp as it would be placed: same emoji, size and look.
        let (_, area) = overlay.emoji_preview_layer().unwrap();
        let placed = Annotation { text: "❌".into(), width: 90.0, ..overlay.annotations[0].clone() };
        let (_, placed_area) = placed.layer(Rect::EVERYTHING).unwrap();
        assert_eq!(area.translate(Pos2::new(80.0, 70.0).to_vec2()), placed_area);
        // With the Select tool, it changes the selected stamp.
        overlay.set_tool(Tool::Select);
        overlay.select(0);
        overlay.choose_emoji("👍🏽".into());
        overlay.restyle_selected();
        assert_eq!(overlay.annotations[0].text, "👍🏽");
        // Picker icons are cropped to what the emoji draws, so none is cut off.
        let ctx = egui::Context::default();
        for emoji in crate::emoji::FAVOURITES {
            let texture = emoji_texture(&ctx, emoji, 40, Color32::WHITE).unwrap();
            let [w, h] = texture.size();
            // Narrow glyphs (❗) can be a few pixels wide, depending on the emoji font.
            assert!(w > 2 && h > 10 && w <= 80 && h <= 80, "{emoji}: {w}x{h}");
        }
        assert!(crate::emoji::is_emoji("👍🏽") && !crate::emoji::is_emoji("ab") && !crate::emoji::is_emoji(""));

        // Placing an emoji puts it first among the recent ones, pushing the oldest out;
        // kaomoji and symbols are not recorded.
        let before = overlay.recent_emoji.clone();
        assert_eq!(before.len(), crate::emoji::FAVOURITES.len());
        overlay.emoji = "🦀".into();
        overlay.note_recent();
        assert_eq!(overlay.recent_emoji[0], "🦀");
        assert_eq!(overlay.recent_emoji.len(), before.len());
        assert_eq!(overlay.recent_emoji.last(), before.get(before.len() - 2));
        overlay.emoji = before[3].clone();
        overlay.note_recent();
        assert_eq!((overlay.recent_emoji[0].as_str(), overlay.recent_emoji[1].as_str()), (before[3].as_str(), "🦀"));
        assert_eq!(overlay.recent_emoji.iter().filter(|e| **e == before[3]).count(), 1);
        overlay.emoji = "(^_^)".into();
        overlay.note_recent();
        assert_eq!(overlay.recent_emoji[0], before[3]);

        // The picker draws: recent, a group, and a search, without picking anything.
        let ctx = egui::Context::default();
        for (tab, search) in [(0, ""), (3, ""), (0, "cat")] {
            overlay.emoji_tab = tab;
            overlay.emoji_field = search.into();
            let mut out = ctx.run_pass(Default::default(), |ctx| {
                let shown = egui::Area::new(egui::Id::unique("picker")).show(ctx, |ui| {
                    assert_eq!(overlay.emoji_panel(ui, false), None);
                });
                // Nine columns of emoji wide, no more.
                assert!(shown.response.rect.width() < 300.0, "{:?}", shown.response.rect);
            });
            out.textures_delta.clear();
        }
        assert!(overlay.emoji_groups.contains_key(&2) && overlay.emoji_groups.len() == 1);
        assert!(overlay.emoji_search.as_ref().is_some_and(|(q, found)| q == "cat" && !found.is_empty()));
    }

    fn key(key: Key) -> egui::Event {
        egui::Event::Key {
            key,
            physical_key: None,
            pressed: true,
            repeat: false,
            modifiers: Modifiers::NONE,
        }
    }

    #[test]
    fn typing_and_editing_text() {
        let mut overlay = plain_overlay(400, 300, RgbaImage::new(400, 300));
        overlay.selection = Some(rect(0.0, 0.0, 400.0, 300.0));
        overlay.set_tool(Tool::Shape(ShapeKind::Text));
        assert_eq!(overlay.width, TEXT_SIZE);

        overlay.start_text(Pos2::new(50.0, 50.0));
        for event in [
            egui::Event::Text("Hello".into()),
            egui::Event::Key { key: Key::Enter, physical_key: None, pressed: true, repeat: false, modifiers: Modifiers::SHIFT },
            egui::Event::Text("wrld".into()),
            key(Key::ArrowLeft),
            key(Key::ArrowLeft),
            key(Key::ArrowLeft),
            egui::Event::Text("o".into()),
            key(Key::ArrowUp),
            key(Key::End),
            egui::Event::Text("!".into()),
            egui::Event::Paste("\r\nbye".into()),
            key(Key::Backspace),
        ] {
            assert!(!overlay.text_event(&event));
        }
        assert_eq!(overlay.annotations[0].text, "Hello!\nby\nworld");
        assert!(overlay.text_event(&key(Key::Enter)), "Enter finishes");
        overlay.finish_editing();

        // The whole edit is one undo step.
        assert!(overlay.editing.is_none());
        overlay.undo();
        assert!(overlay.annotations.is_empty());
        overlay.redo();
        assert_eq!(overlay.annotations.len(), 1);

        // A text left empty disappears without an undo step.
        overlay.start_text(Pos2::new(50.0, 200.0));
        overlay.text_event(&egui::Event::Text("   ".into()));
        overlay.finish_editing();
        assert_eq!(overlay.annotations.len(), 1);
        overlay.undo();
        assert!(overlay.annotations.is_empty());
    }

    #[test]
    fn closing_asks_only_when_there_is_something_to_lose() {
        let mut settings = Settings::default();
        let draw = |overlay: &mut Overlay| {
            overlay.selection = Some(rect(0.0, 0.0, 100.0, 60.0));
            overlay.set_tool(Tool::Shape(ShapeKind::Rect));
            overlay.begin_action(Action::NewShape { anchor: Pos2::new(20.0, 20.0) });
            overlay.apply_action(Pos2::new(60.0, 40.0), Modifiers::NONE, overlay.bounds());
            overlay.end_action();
        };
        let desktop = || Desktop::new(
            vec![RgbaImage::new(100, 60)],
            RECT { left: 0, top: 0, right: 100, bottom: 60 },
            vec![rect(0.0, 0.0, 100.0, 60.0)],
            0,
            Snap::default(),
        );

        // Off by default: Esc closes at once, annotations or not.
        let mut overlay = Overlay::with_desktop(desktop(), None, &settings);
        draw(&mut overlay);
        assert!(overlay.request_close() && !overlay.confirm_open);

        // Switched on: nothing to lose closes at once; an annotation asks first.
        settings.confirm_discard = true;
        let mut overlay = Overlay::with_desktop(desktop(), None, &settings);
        assert!(overlay.request_close() && !overlay.confirm_open);
        draw(&mut overlay);
        overlay.save_open = true;
        assert!(!overlay.request_close());
        assert!(overlay.confirm_open && !overlay.save_open);
        // Undoing the annotation leaves nothing to ask about.
        overlay.confirm_open = false;
        overlay.undo();
        assert!(overlay.request_close());
    }

    #[test]
    fn border_and_shadow_surround_the_output() {
        let blue = image::Rgba([0, 0, 255, 255]);
        let mut settings = Settings::default();
        settings.frame.border = true;
        settings.frame.border_width = 3;
        settings.frame.border_color = "#FF0000".into();
        let desktop = Desktop::new(
            vec![RgbaImage::from_pixel(100, 60, blue)],
            RECT { left: 0, top: 0, right: 100, bottom: 60 },
            vec![rect(0.0, 0.0, 100.0, 60.0)],
            0,
            Snap::default(),
        );
        let mut overlay = Overlay::with_desktop(desktop, None, &settings);
        overlay.selection = Some(rect(10.0, 10.0, 50.0, 30.0));
        let out = overlay.output().unwrap();
        assert_eq!(out.dimensions(), (46, 26));
        assert_eq!(out.get_pixel(0, 0).0, [255, 0, 0, 255]);
        assert_eq!(out.get_pixel(3, 3), &blue);
        assert_eq!(out.get_pixel(42, 22), &blue);
        assert_eq!(out.get_pixel(43, 23).0, [255, 0, 0, 255]);

        // Switched off for this capture: the plain image again.
        overlay.border_on = false;
        assert_eq!(overlay.output().unwrap().dimensions(), (40, 20));

        // With a shadow the image keeps its transparent margin (the clipboard adds a
        // copy on white for programs without transparency; see `clipboard`).
        overlay.shadow = true;
        assert_eq!(overlay.output().unwrap().get_pixel(0, 0).0[3], 0);

        // A see-through border stays see-through.
        overlay.shadow = false;
        overlay.border_on = true;
        overlay.border.color = Color32::from_rgba_unmultiplied(255, 0, 0, 128);
        let [r, _, b, a] = overlay.output().unwrap().get_pixel(0, 0).0;
        assert!((120..=136).contains(&a) && r > 200 && b == 0, "{:?}", [r, b, a]);
    }

    #[test]
    fn picking_lines_and_words() {
        let quad = |x: f32, y: f32, w: f32| read::Quad::from_rect(rect(x, y, x + w, y + 12.0));
        let word = |text: &str, x: f32, y: f32| read::Word { text: text.into(), quad: quad(x, y, 40.0) };
        let lines = vec![
            read::Line {
                text: "alpha beta".into(),
                words: vec![word("alpha", 0.0, 0.0), word("beta", 50.0, 0.0)],
                quad: quad(0.0, 0.0, 90.0),
            },
            read::Line { text: "gamma".into(), words: vec![word("gamma", 0.0, 30.0)], quad: quad(0.0, 30.0, 40.0) },
        ];
        let mut reading = Reading {
            kind: read::Kind::Text,
            origin: Pos2::new(100.0, 200.0),
            pending: None,
            outcome: Some(Ok(lines)),
            picked: HashSet::new(),
            text: String::new(),
        };
        assert_eq!(reading.picked_text(), "alpha beta\ngamma");
        // Hits are in desktop pixels, offset by where the region was.
        assert_eq!(reading.hit(Pos2::new(160.0, 205.0)), Some((0, 1)));
        assert_eq!(reading.hit(Pos2::new(105.0, 235.0)), Some((1, 0)));
        assert_eq!(reading.hit(Pos2::new(105.0, 260.0)), None);

        reading.toggle((1, 0), false);
        assert_eq!(reading.text, "gamma");
        reading.toggle((0, 1), true);
        assert_eq!(reading.text, "beta\ngamma");
        // A click on a partly picked line picks all of it; again lets it go.
        reading.toggle((0, 0), false);
        assert_eq!(reading.text, "alpha beta\ngamma");
        reading.toggle((0, 0), false);
        assert_eq!(reading.text, "gamma");
        reading.toggle((1, 0), false);
        assert_eq!(reading.text, "alpha beta\ngamma"); // nothing picked: everything
    }

    #[test]
    fn blur_regions_apply_to_the_output() {
        // Alternating black and white columns.
        let shot = RgbaImage::from_fn(100, 60, |x, _| {
            if x % 2 == 0 { image::Rgba([0, 0, 0, 255]) } else { image::Rgba([255, 255, 255, 255]) }
        });
        let mut overlay = plain_overlay(100, 60, shot);
        overlay.selection = Some(rect(10.0, 10.0, 90.0, 50.0));
        overlay.set_tool(Tool::Shape(ShapeKind::Blur));
        overlay.begin_action(Action::NewShape { anchor: Pos2::new(20.0, 20.0) });
        overlay.apply_action(Pos2::new(60.0, 40.0), Modifiers::NONE, overlay.bounds());
        overlay.end_action();
        assert_eq!(overlay.annotations[0].kind, ShapeKind::Blur);

        let out = overlay.output().unwrap();
        // Inside the blur region (desktop 20..60): blocks average to mid grey.
        let inside = out.get_pixel(20, 15).0;
        assert!((100..=155).contains(&inside[0]) && inside[0] == inside[1], "{inside:?}");
        // Outside it the stripes are untouched (output x 0 is desktop x 10, a black column).
        assert_eq!(out.get_pixel(0, 0).0, [0, 0, 0, 255]);
        assert_eq!(out.get_pixel(1, 0).0, [255, 255, 255, 255]);
        assert_eq!(out.get_pixel(75, 35).0, [255, 255, 255, 255]); // desktop x 85

        // Redact: a solid, opaque fill in the shape's colour (black by default),
        // ignoring the strength.
        assert_eq!(overlay.annotations[0].color, Color32::BLACK);
        overlay.annotations[0].effect = Effect::Redact;
        overlay.annotations[0].color = Color32::from_rgba_unmultiplied(10, 20, 30, 40);
        let out = overlay.output().unwrap();
        // The colour's own translucency is ignored: a redaction is always opaque.
        let [r, g, b, _] = overlay.annotations[0].color.to_srgba_unmultiplied();
        for (x, y) in [(10, 10), (30, 20), (49, 29)] {
            assert_eq!(out.get_pixel(x, y).0, [r, g, b, 255]);
        }
        assert_ne!(out.get_pixel(51, 20).0, [r, g, b, 255]); // desktop x 61: outside
    }

    #[test]
    fn hsl_values() {
        assert_eq!(hsl(Color32::from_rgb(0, 0, 0)), (0, 0, 0));
        assert_eq!(hsl(Color32::from_rgb(255, 255, 255)), (0, 0, 100));
        assert_eq!(hsl(Color32::from_rgb(255, 0, 0)), (0, 100, 50));
        assert_eq!(hsl(Color32::from_rgb(0, 255, 0)), (120, 100, 50));
        assert_eq!(hsl(Color32::from_rgb(0, 0, 255)), (240, 100, 50));
        assert_eq!(hsl(Color32::from_rgb(255, 0, 128)), (330, 100, 50));
        assert_eq!(hsl(Color32::from_rgb(0x33, 0x66, 0x99)), (210, 50, 40));
    }

    #[test]
    fn redactions_stay_solid_under_later_blurs() {
        let shot = RgbaImage::from_fn(100, 60, |x, _| {
            if x % 2 == 0 { image::Rgba([0, 0, 0, 255]) } else { image::Rgba([255, 255, 255, 255]) }
        });
        let mut overlay = plain_overlay(100, 60, shot);
        overlay.selection = Some(rect(0.0, 0.0, 100.0, 60.0));
        overlay.set_tool(Tool::Shape(ShapeKind::Blur));
        // A redaction, then a pixelate and a Gaussian region drawn over part of it.
        for (effect, from, to) in [
            (Effect::Redact, Pos2::new(10.0, 10.0), Pos2::new(50.0, 50.0)),
            (Effect::Pixelate, Pos2::new(30.0, 20.0), Pos2::new(80.0, 40.0)),
            (Effect::Gaussian, Pos2::new(5.0, 5.0), Pos2::new(25.0, 25.0)),
        ] {
            overlay.effect = effect;
            overlay.begin_action(Action::NewShape { anchor: from });
            overlay.apply_action(to, Modifiers::NONE, overlay.bounds());
            overlay.end_action();
        }
        assert_eq!(overlay.annotations.len(), 3);
        assert_eq!(overlay.annotations[0].effect, Effect::Redact);
        let out = overlay.output().unwrap();
        for y in 10..50 {
            for x in 10..50 {
                assert_eq!(out.get_pixel(x, y).0, [0, 0, 0, 255], "({x}, {y})");
            }
        }
        // The pixelate region still applies outside the redaction.
        let outside = out.get_pixel(60, 30).0;
        assert!((100..=155).contains(&outside[0]), "{outside:?}");
    }

    #[test]
    fn crop_stitches_across_monitors() {
        // Two 4x4 monitors side by side: left is red, right is blue and sits 2 px lower.
        let solid = |c: [u8; 4]| RgbaImage::from_pixel(4, 4, image::Rgba(c));
        let mut overlay = Overlay::with_desktop(
            Desktop::new(
                vec![solid([255, 0, 0, 255]), solid([0, 0, 255, 255])],
                RECT { left: -4, top: 0, right: 4, bottom: 6 },
                vec![rect(0.0, 0.0, 4.0, 4.0), rect(4.0, 2.0, 8.0, 6.0)],
                0,
                Snap::default(),
            ),
            None,
            &Settings::default(),
        );
        overlay.selection = Some(rect(3.0, 1.0, 6.0, 4.0));
        let out = overlay.cropped().unwrap();
        assert_eq!(out.dimensions(), (3, 3));
        assert_eq!(out.get_pixel(0, 0).0, [255, 0, 0, 255]); // left monitor
        assert_eq!(out.get_pixel(1, 0).0, [0, 0, 0, 0]); // above the right monitor: no screen there
        assert_eq!(out.get_pixel(1, 1).0, [0, 0, 255, 255]); // right monitor
        assert_eq!(out.get_pixel(2, 2).0, [0, 0, 255, 255]);
    }

    #[test]
    fn ctrl_drag_moves_a_shape_with_any_tool() {
        let mut overlay = plain_overlay(200, 200, RgbaImage::new(200, 200));
        overlay.selection = Some(rect(0.0, 0.0, 200.0, 200.0));
        overlay.set_tool(Tool::Shape(ShapeKind::Rect));
        overlay.width = 9.0;
        overlay.begin_action(Action::NewShape { anchor: Pos2::new(20.0, 20.0) });
        overlay.apply_action(Pos2::new(60.0, 60.0), Modifiers::NONE, overlay.bounds());
        overlay.end_action();
        overlay.set_tool(Tool::Shape(ShapeKind::Arrow));
        let arrow_width = overlay.width;
        let edge = Pos2::new(20.0, 40.0);

        // Without Ctrl the arrow tool draws a new arrow there.
        assert!(matches!(overlay.pick_action(edge, GRAB_TOLERANCE), Action::NewShape { .. }));

        overlay.mods = Modifiers::CTRL;
        overlay.begin_action(overlay.pick_action(edge, GRAB_TOLERANCE));
        overlay.apply_action(edge + vec2(10.0, 5.0), Modifiers::CTRL, overlay.bounds());
        overlay.end_action();
        assert_eq!(overlay.annotations.len(), 1);
        assert_eq!(overlay.annotations[0].rect(), rect(30.0, 25.0, 70.0, 65.0));
        // The rectangle's style did not become the arrow's.
        assert_eq!(overlay.width, arrow_width);
        assert_eq!(overlay.selected, None);
        // Undo puts it back.
        overlay.undo();
        assert_eq!(overlay.annotations[0].rect(), rect(20.0, 20.0, 60.0, 60.0));
    }
}

#[cfg(test)]
mod view_tests {
    use super::*;

    const MONITOR: RECT = RECT { left: 0, top: 0, right: 2560, bottom: 1440 };

    #[test]
    fn small_opened_images_get_a_window_of_their_own() {
        let small = Desktop::open_on(RgbaImage::new(300, 300), Opened::default(), MONITOR);
        let b = small.bounds;
        let (w, h) = (b.right - b.left, b.bottom - b.top);
        assert!(w < 2560 && h < 1440 && w >= 960 && h >= 640, "{w}x{h}");
        assert!((b.left * 2 + w - 2560).abs() <= 1 && (b.top * 2 + h - 1440).abs() <= 1, "centred");
        assert_eq!(small.monitors[0].size(), vec2(300.0, 300.0));
        assert!(Rect::from_min_size(Pos2::ZERO, small.space).contains_rect(small.monitors[0]));
        let big = Desktop::open_on(RgbaImage::new(4000, 3000), Opened::default(), MONITOR);
        assert_eq!((big.bounds.left, big.bounds.top, big.bounds.right, big.bounds.bottom), (0, 0, 2560, 1440));
    }

    #[test]
    fn ctrl_wheel_zooms_around_the_pointer_and_ctrl_0_fits() {
        let monitor = RECT { left: 0, top: 0, right: 1280, bottom: 720 };
        let desktop = Desktop::open_on(RgbaImage::new(4000, 3000), Opened::default(), monitor);
        let mut overlay = Overlay::with_desktop(desktop, None, &Settings::default());
        let screen = Rect::from_min_size(Pos2::ZERO, vec2(1280.0, 720.0));
        let fit = overlay.bounds().width() / screen.width();
        let to_px = |o: &Overlay, p: Pos2| ((p - screen.min - o.pan) * (fit / o.zoom)).to_pos2();
        let ctrl = egui::Modifiers { ctrl: true, command: true, ..Default::default() };
        let ctx = egui::Context::default();
        let p = Pos2::new(300.0, 200.0);
        let before = to_px(&overlay, p);
        for _ in 0..3 {
            let input = egui::RawInput {
                screen_rect: Some(screen),
                events: vec![
                    egui::Event::ModifiersChanged(ctrl),
                    egui::Event::PointerMoved(p),
                    egui::Event::MouseWheel {
                        unit: egui::MouseWheelUnit::Line,
                        delta: vec2(0.0, 1.0),
                        phase: egui::TouchPhase::Move,
                        source: egui::MouseWheelSource::Wheel,
                        modifiers: ctrl,
                    },
                ],
                ..Default::default()
            };
            ctx.run_pass(input, |ctx| overlay.zoom_and_pan(ctx, screen, fit)).textures_delta.clear();
        }
        assert!(overlay.zoom > 1.9, "zoomed: {}", overlay.zoom);
        assert!(to_px(&overlay, p).distance(before) < 0.5, "the pixel under the pointer stays put");
        // Panned no further than the image's edges.
        assert!(overlay.pan.x <= 0.0 && overlay.pan.y <= 0.0);
        assert!(overlay.pan.x >= screen.width() * (1.0 - overlay.zoom));
        let input = egui::RawInput {
            screen_rect: Some(screen),
            events: vec![egui::Event::ModifiersChanged(ctrl), egui::Event::Key { key: Key::Num0, physical_key: None, pressed: true, repeat: false, modifiers: ctrl }],
            ..Default::default()
        };
        ctx.run_pass(input, |ctx| overlay.zoom_and_pan(ctx, screen, fit)).textures_delta.clear();
        assert_eq!((overlay.zoom, overlay.pan), (1.0, Vec2::ZERO));
    }

    #[test]
    fn extra_shortcuts_and_keep_minutes_are_checked() {
        let s = Settings::from_toml("clipboard_hotkey = \"Banana+Q\"\nreopen_hotkey = \"Ctrl+F10\"\nkeep_minutes = 999\n");
        assert_eq!(s.clipboard_hotkey, "");
        assert_eq!(s.reopen_hotkey, "Ctrl+F10");
        assert_eq!(s.keep_minutes, crate::settings::MAX_KEEP_MINUTES);
        assert_eq!(Settings::default().keep_minutes, 0, "off by default");
    }

    #[test]
    fn a_kept_capture_comes_back_as_it_was() {
        use super::tests::{plain_overlay, rect};
        let mut overlay = plain_overlay(40, 30, RgbaImage::from_fn(40, 30, |x, y| image::Rgba([x as u8, y as u8, 7, 255])));
        overlay.selection = Some(rect(2.0, 3.0, 30.0, 20.0));
        overlay.rotation = Rotation { angle: 0.25, offset: Vec2::new(1.0, -2.0) };
        overlay.set_tool(Tool::Shape(ShapeKind::Arrow));
        overlay.color = Color32::from_rgb(1, 2, 3);
        overlay.emoji = "🦀".into();
        overlay.border_on = true;
        overlay.shadow = true;
        let mut arrow = Annotation::stroked(ShapeKind::Arrow, Pos2::new(1.0, 1.0), Pos2::new(9.0, 9.0), Color32::RED, 4.0, Dash::Dot);
        arrow.points = vec![Pos2::new(3.0, 4.0)];
        overlay.history.record(&overlay.annotations, None);
        overlay.annotations.push(arrow);
        overlay.desktop.opened = Some(Opened { stem: Some("bild".into()), folder: Some("C:\\Bilder".into()), format: Some(Format::Jpeg) });

        let bytes = overlay.kept_bytes();
        let (desktop, kept) = read_kept(&bytes).unwrap();
        assert_eq!(desktop.shots, overlay.desktop.shots);
        assert_eq!((desktop.bounds.right, desktop.monitors.clone(), desktop.opened.clone()), (overlay.desktop.bounds.right, overlay.desktop.monitors.clone(), overlay.desktop.opened.clone()));
        let mut back = Overlay::with_desktop(desktop, None, &Settings::default());
        back.restore(kept);
        assert_eq!(back.selection, overlay.selection);
        assert_eq!(back.rotation, overlay.rotation);
        assert_eq!((back.tool, back.color, back.width), (overlay.tool, overlay.color, overlay.width));
        assert_eq!((back.emoji.as_str(), back.border_on, back.shadow), ("🦀", true, true));
        assert_eq!(back.annotations, overlay.annotations);
        assert_eq!(back.history.steps().0, overlay.history.steps().0);
        assert_eq!(back.styles, overlay.styles);
        assert_eq!(back.widths, overlay.widths);

        // Damaged or cut short: an error, never a panic.
        assert!(read_kept(&bytes[..bytes.len() - 1]).is_err());
        assert!(read_kept(b"nope").is_err());
        assert!(read_kept(&[]).is_err());
    }
}
