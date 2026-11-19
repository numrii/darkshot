//! Windows implementation details for the private editor platform boundary.

use std::cell::Cell;
use std::time::{Duration, Instant};

use raw_window_handle::{HasWindowHandle, RawWindowHandle};
use windows_sys::Win32::Foundation::{HWND, LPARAM, LRESULT, POINT, WPARAM};
use windows_sys::Win32::Graphics::Dwm::{DwmSetWindowAttribute, DWMWA_TRANSITIONS_FORCEDISABLED};
use windows_sys::Win32::Graphics::Gdi::{RedrawWindow, RDW_INTERNALPAINT, RDW_UPDATENOW};
use windows_sys::Win32::UI::HiDpi::{
    GetDpiForSystem, SetProcessDpiAwarenessContext, DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE_V2,
};
use windows_sys::Win32::UI::Shell::{DefSubclassProc, RemoveWindowSubclass, SetWindowSubclass};
use windows_sys::Win32::UI::WindowsAndMessaging::{
    GetCursorPos, GetForegroundWindow, GetWindowRect, KillTimer, SetForegroundWindow, SetTimer,
    SetWindowPos, ShowWindow, HWND_TOPMOST, SW_HIDE, WM_MOUSEFIRST, WM_MOUSELAST, WM_MOUSEMOVE,
    WM_NCDESTROY, WM_PAINT, WM_TIMER,
};

use super::{DesktopRect, OverlayStats};

pub(crate) type OverlayHandle = HWND;

pub(crate) fn prepare_editor_process() {
    unsafe { SetProcessDpiAwarenessContext(DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE_V2) };
}

pub(crate) fn editor_scale() -> f32 {
    (unsafe { GetDpiForSystem() }) as f32 / 96.0
}

pub(crate) fn editor_monitor() -> DesktopRect {
    use std::mem::size_of;
    use windows_sys::Win32::Graphics::Gdi::{
        GetMonitorInfoW, MonitorFromPoint, MONITORINFO, MONITOR_DEFAULTTONEAREST,
    };

    let mut cursor = POINT { x: 0, y: 0 };
    let mut info: MONITORINFO = unsafe { std::mem::zeroed() };
    info.cbSize = size_of::<MONITORINFO>() as u32;
    unsafe {
        GetCursorPos(&mut cursor);
        GetMonitorInfoW(
            MonitorFromPoint(cursor, MONITOR_DEFAULTTONEAREST),
            &mut info,
        );
    }
    info.rcMonitor.into()
}

pub(crate) fn overlay_handle(window: &impl HasWindowHandle) -> Option<OverlayHandle> {
    match window.window_handle().ok()?.as_raw() {
        RawWindowHandle::Win32(handle) => Some(handle.hwnd.get() as HWND),
        _ => None,
    }
}

const MOUSE_MOVE_INTERVAL: Duration = Duration::from_millis(2);
const MAX_PAINT_DELAY: Duration = Duration::from_millis(5);
const MOUSE_THROTTLE_ID: usize = 0xDA5C_5407;

thread_local! {
    static LAST_FORWARDED: Cell<Option<Instant>> = const { Cell::new(None) };
    static LAST_PAINT: Cell<Option<Instant>> = const { Cell::new(None) };
    static PENDING: Cell<Option<(WPARAM, LPARAM)>> = const { Cell::new(None) };
    static STATS: Cell<OverlayStats> = const { Cell::new(OverlayStats {
        moves: 0, moves_forwarded: 0, other: 0, forced_paints: 0, slowest: (0, 0), paint_us: 0,
    }) };
}

fn bump(f: impl FnOnce(&mut OverlayStats)) {
    let mut stats = STATS.get();
    f(&mut stats);
    STATS.set(stats);
}

/// Window subclass that prevents a high-polling mouse from starving paints.
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
                if LAST_PAINT
                    .get()
                    .is_none_or(|last| now.duration_since(last) >= MAX_PAINT_DELAY)
                {
                    bump(|s| s.forced_paints += 1);
                    RedrawWindow(
                        hwnd,
                        std::ptr::null(),
                        std::ptr::null_mut(),
                        RDW_INTERNALPAINT | RDW_UPDATENOW,
                    );
                }
                return result;
            }
            WM_PAINT => {
                let start = Instant::now();
                LAST_PAINT.set(Some(start));
                let result = DefSubclassProc(hwnd, msg, wparam, lparam);
                bump(|s| s.paint_us = start.elapsed().as_micros() as u32);
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
            WM_MOUSEFIRST..=WM_MOUSELAST => {
                if let Some((w, _)) = PENDING.take() {
                    DefSubclassProc(hwnd, WM_MOUSEMOVE, w, lparam);
                }
            }
            WM_NCDESTROY => RemoveWindowSubclass(hwnd, Some(pump_guard), MOUSE_THROTTLE_ID),
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

pub(crate) fn prepare_overlay(hwnd: OverlayHandle) {
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

pub(crate) fn focus_overlay(hwnd: OverlayHandle) {
    unsafe { SetForegroundWindow(hwnd) };
}

pub(crate) fn overlay_has_focus(hwnd: OverlayHandle) -> bool {
    unsafe { GetForegroundWindow() == hwnd }
}

pub(crate) fn cover_desktop(hwnd: OverlayHandle, bounds: DesktopRect, force: bool) -> bool {
    unsafe {
        let mut now = std::mem::zeroed();
        let in_place = GetWindowRect(hwnd, &mut now) != 0
            && (now.left, now.top, now.right, now.bottom)
                == (bounds.left, bounds.top, bounds.right, bounds.bottom);
        if force || !in_place {
            SetWindowPos(
                hwnd,
                HWND_TOPMOST,
                bounds.left,
                bounds.top,
                bounds.right - bounds.left,
                bounds.bottom - bounds.top,
                0,
            );
        }
        !in_place
    }
}

pub(crate) fn hide_overlay(hwnd: OverlayHandle) {
    unsafe { ShowWindow(hwnd, SW_HIDE) };
}

pub(crate) fn trace_pointer() -> Option<(i32, i32)> {
    let mut point = POINT { x: 0, y: 0 };
    (unsafe { GetCursorPos(&mut point) } != 0).then_some((point.x, point.y))
}

pub(crate) fn take_overlay_stats() -> OverlayStats {
    STATS.take()
}
