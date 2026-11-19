//! Private operating-system boundary for Duskshot's editor.
//!
//! The editor operates on frozen RGBA images in a shared desktop coordinate space.
//! Platform code is responsible for obtaining those images and reporting where each
//! output sits in that space; it does not leak native window or compositor handles
//! into the editor.

use image::RgbaImage;

use crate::AnyResult;

#[cfg(target_os = "linux")]
mod linux;
#[cfg(windows)]
mod windows;

/// Platform-owned native window reference for the editor overlay.  The editor never
/// needs to know whether this is an HWND, a layer-shell surface, or something else.
#[cfg(windows)]
pub(crate) use windows::OverlayHandle;

#[cfg(not(windows))]
#[derive(Clone, Copy)]
pub(crate) struct OverlayHandle;

/// Input/paint measurements used only by `--trace`.
#[derive(Clone, Copy, Default)]
pub(crate) struct OverlayStats {
    pub(crate) moves: u32,
    pub(crate) moves_forwarded: u32,
    pub(crate) other: u32,
    pub(crate) forced_paints: u32,
    pub(crate) slowest: (u32, u32),
    pub(crate) paint_us: u32,
}

/// A rectangle in global desktop pixels. This is deliberately not a native windowing
/// type: captures, saved state, and editor layout all use the same representation.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(crate) struct DesktopRect {
    pub(crate) left: i32,
    pub(crate) top: i32,
    pub(crate) right: i32,
    pub(crate) bottom: i32,
}

#[cfg(windows)]
impl From<windows_sys::Win32::Foundation::RECT> for DesktopRect {
    fn from(rect: windows_sys::Win32::Foundation::RECT) -> Self {
        Self {
            left: rect.left,
            top: rect.top,
            right: rect.right,
            bottom: rect.bottom,
        }
    }
}

/// One frozen output image and its top-left desktop coordinate in physical pixels.
pub(crate) struct Output {
    pub(crate) x: i32,
    pub(crate) y: i32,
    pub(crate) image: RgbaImage,
}

/// A single instant of the desktop, normalized for the platform-independent editor.
pub(crate) struct Capture {
    pub(crate) outputs: Vec<Output>,
    /// Pointer position in the same coordinate space as the outputs, if available.
    pub(crate) pointer: Option<(i32, i32)>,
}

/// Process and window setup required before showing the native editor surface.
#[cfg(windows)]
pub(crate) fn prepare_editor_process() {
    windows::prepare_editor_process();
}

#[cfg(not(windows))]
pub(crate) fn prepare_editor_process() {}

#[cfg(windows)]
pub(crate) fn editor_scale() -> f32 {
    windows::editor_scale()
}

#[cfg(not(windows))]
pub(crate) fn editor_scale() -> f32 {
    1.0
}

/// Initial native window size in physical pixels. Windows retains its full-desktop
/// overlay; Linux deliberately opens a normal, fit-sized editor window instead.
#[cfg(windows)]
pub(crate) fn editor_window_size(bounds: DesktopRect) -> (f32, f32) {
    (
        (bounds.right - bounds.left) as f32,
        (bounds.bottom - bounds.top) as f32,
    )
}

#[cfg(target_os = "linux")]
pub(crate) fn editor_window_size(bounds: DesktopRect) -> (f32, f32) {
    const MAX_WIDTH: f32 = 1_600.0;
    const MAX_HEIGHT: f32 = 1_000.0;
    let width = (bounds.right - bounds.left).max(1) as f32;
    let height = (bounds.bottom - bounds.top).max(1) as f32;
    let fit = (MAX_WIDTH / width).min(MAX_HEIGHT / height).min(1.0);
    (width * fit, height * fit)
}

#[cfg(not(any(windows, target_os = "linux")))]
pub(crate) fn editor_window_size(bounds: DesktopRect) -> (f32, f32) {
    (
        (bounds.right - bounds.left) as f32,
        (bounds.bottom - bounds.top) as f32,
    )
}

/// The monitor on which a file or clipboard image should open.  Wayland output
/// discovery will replace the Linux placeholder when the layer-shell backend lands.
#[cfg(windows)]
pub(crate) fn editor_monitor() -> DesktopRect {
    windows::editor_monitor()
}

#[cfg(target_os = "linux")]
pub(crate) fn editor_monitor() -> DesktopRect {
    DesktopRect {
        left: 0,
        top: 0,
        right: 1920,
        bottom: 1080,
    }
}

#[cfg(not(any(windows, target_os = "linux")))]
pub(crate) fn editor_monitor() -> DesktopRect {
    DesktopRect {
        left: 0,
        top: 0,
        right: 1920,
        bottom: 1080,
    }
}

#[cfg(windows)]
pub(crate) fn overlay_handle(
    window: &impl raw_window_handle::HasWindowHandle,
) -> Option<OverlayHandle> {
    windows::overlay_handle(window)
}

#[cfg(not(windows))]
pub(crate) fn overlay_handle(
    _window: &impl raw_window_handle::HasWindowHandle,
) -> Option<OverlayHandle> {
    None
}

#[cfg(windows)]
pub(crate) fn prepare_overlay(handle: OverlayHandle) {
    windows::prepare_overlay(handle);
}

#[cfg(not(windows))]
pub(crate) fn prepare_overlay(_handle: OverlayHandle) {}

#[cfg(windows)]
pub(crate) fn focus_overlay(handle: OverlayHandle) {
    windows::focus_overlay(handle);
}

#[cfg(not(windows))]
pub(crate) fn focus_overlay(_handle: OverlayHandle) {}

#[cfg(windows)]
pub(crate) fn overlay_has_focus(handle: OverlayHandle) -> bool {
    windows::overlay_has_focus(handle)
}

#[cfg(not(windows))]
pub(crate) fn overlay_has_focus(_handle: OverlayHandle) -> bool {
    true
}

#[cfg(windows)]
pub(crate) fn cover_desktop(handle: OverlayHandle, bounds: DesktopRect, force: bool) -> bool {
    windows::cover_desktop(handle, bounds, force)
}

#[cfg(not(windows))]
pub(crate) fn cover_desktop(_handle: OverlayHandle, _bounds: DesktopRect, _force: bool) -> bool {
    false
}

#[cfg(windows)]
pub(crate) fn hide_overlay(handle: OverlayHandle) {
    windows::hide_overlay(handle);
}

#[cfg(not(windows))]
pub(crate) fn hide_overlay(_handle: OverlayHandle) {}

#[cfg(windows)]
pub(crate) fn trace_pointer() -> Option<(i32, i32)> {
    windows::trace_pointer()
}

#[cfg(not(windows))]
pub(crate) fn trace_pointer() -> Option<(i32, i32)> {
    None
}

#[cfg(windows)]
pub(crate) fn take_overlay_stats() -> OverlayStats {
    windows::take_overlay_stats()
}

#[cfg(not(windows))]
pub(crate) fn take_overlay_stats() -> OverlayStats {
    OverlayStats::default()
}

/// Copy plain text without placing it in operating-system clipboard history where
/// that platform supports an exclusion mechanism.
#[cfg(windows)]
pub(crate) fn copy_text(text: &str) -> AnyResult<()> {
    use arboard::SetExtWindows;
    arboard::Clipboard::new()?
        .set()
        .exclude_from_history()
        .exclude_from_cloud()
        .text(text)?;
    Ok(())
}

#[cfg(target_os = "linux")]
pub(crate) fn copy_text(text: &str) -> AnyResult<()> {
    use arboard::SetExtLinux;
    arboard::Clipboard::new()?
        .set()
        .exclude_from_history()
        .text(text)?;
    Ok(())
}

#[cfg(not(any(windows, target_os = "linux")))]
pub(crate) fn copy_text(text: &str) -> AnyResult<()> {
    arboard::Clipboard::new()?.set_text(text)?;
    Ok(())
}

/// Capture the visible desktop. Platform implementations must return unscaled RGBA
/// pixels and one entry for every captured output.
#[cfg(windows)]
pub(crate) fn capture(include_cursor: bool) -> AnyResult<Capture> {
    use windows_sys::Win32::Foundation::POINT;
    use windows_sys::Win32::UI::WindowsAndMessaging::GetCursorPos;

    let mut cursor = POINT { x: 0, y: 0 };
    unsafe { GetCursorPos(&mut cursor) };
    // Read first, so it is the cursor as it was when the capture was asked for.
    let pointer = include_cursor
        .then(crate::cursor::Cursor::current)
        .flatten();

    // Monitors showing HDR are read in their own format and converted (see `hdr`);
    // the ordinary capture of one looks washed out.
    let hdr = crate::hdr::Hdr::detect();
    let mut outputs = Vec::new();
    for monitor in xcap::Monitor::all()? {
        let (x, y) = (monitor.x()?, monitor.y()?);
        let image = match hdr.as_ref().and_then(|hdr| hdr.capture(x, y)) {
            Some(image) => image,
            None => monitor.capture_image()?,
        };
        outputs.push(Output { x, y, image });
    }
    if let Some(pointer) = pointer {
        let mut images: Vec<_> = outputs
            .drain(..)
            .map(|output| (output.x, output.y, output.image))
            .collect();
        pointer.draw(&mut images);
        outputs = images
            .into_iter()
            .map(|(x, y, image)| Output { x, y, image })
            .collect();
    }
    Ok(Capture {
        outputs,
        pointer: Some((cursor.x, cursor.y)),
    })
}

/// The wlroots screencopy implementation is introduced behind this same boundary.
/// Keeping the error here makes unsupported Linux sessions fail plainly until then,
/// rather than silently falling back to X11 or an unrelated capture backend.
#[cfg(target_os = "linux")]
pub(crate) fn capture(include_cursor: bool) -> AnyResult<Capture> {
    linux::capture(include_cursor)
}

#[cfg(not(any(windows, target_os = "linux")))]
pub(crate) fn capture(_include_cursor: bool) -> AnyResult<Capture> {
    Err("screen capture is not supported on this platform".into())
}
