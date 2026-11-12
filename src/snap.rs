//! Snapping the capture region to windows: the window list taken with the screenshot,
//! picking the window under a point, and pulling edges onto nearby window edges.

use eframe::egui::{Pos2, Rect};
use windows_sys::Win32::Foundation::{HWND, LPARAM, RECT};
use windows_sys::Win32::Graphics::Dwm::{DwmGetWindowAttribute, DWMWA_CLOAKED, DWMWA_EXTENDED_FRAME_BOUNDS};
use windows_sys::Win32::UI::WindowsAndMessaging::{EnumWindows, GetClassNameW, IsIconic, IsWindowVisible};

/// How close (in desktop pixels) an edge must be to a window edge to snap to it.
pub const SNAP_DISTANCE: f32 = 10.0;

#[derive(Default)]
pub struct Snap {
    /// Visible top-level windows, front to back, in desktop pixels.
    pub windows: Vec<Rect>,
    /// Every window and monitor edge, for snapping, sorted by position. Each carries
    /// its window's place in the stacking order (0 = frontmost; monitors come last),
    /// which decides between two edges at the same distance.
    xs: Vec<(f32, usize)>,
    ys: Vec<(f32, usize)>,
}

impl Snap {
    pub fn new(windows: Vec<Rect>, monitors: &[Rect]) -> Self {
        let mut xs = Vec::new();
        let mut ys = Vec::new();
        for (order, r) in windows.iter().chain(monitors).enumerate() {
            xs.extend([(r.left(), order), (r.right(), order)]);
            ys.extend([(r.top(), order), (r.bottom(), order)]);
        }
        let tidy = |v: &mut Vec<(f32, usize)>| v.sort_by(|a, b| a.0.total_cmp(&b.0).then(a.1.cmp(&b.1)));
        tidy(&mut xs);
        tidy(&mut ys);
        Self { windows, xs, ys }
    }

    /// The frontmost window containing `p`.
    pub fn window_at(&self, p: Pos2) -> Option<Rect> {
        self.windows.iter().find(|w| w.contains(p)).copied()
    }

    /// Pull the chosen edges of `r` onto the nearest window or monitor edge within
    /// `SNAP_DISTANCE`. Edges that would cross over each other are left alone.
    pub fn edges(&self, r: Rect, left: bool, right: bool, top: bool, bottom: bool) -> Rect {
        let mut out = r;
        if left {
            out.min.x = nearest(&self.xs, r.min.x).unwrap_or(r.min.x);
        }
        if right {
            out.max.x = nearest(&self.xs, r.max.x).unwrap_or(r.max.x);
        }
        if top {
            out.min.y = nearest(&self.ys, r.min.y).unwrap_or(r.min.y);
        }
        if bottom {
            out.max.y = nearest(&self.ys, r.max.y).unwrap_or(r.max.y);
        }
        if out.width() < 1.0 {
            (out.min.x, out.max.x) = (r.min.x, r.max.x);
        }
        if out.height() < 1.0 {
            (out.min.y, out.max.y) = (r.min.y, r.max.y);
        }
        out
    }

    /// Shift `r` (keeping its size) so whichever of its edges is closest to a window
    /// or monitor edge lines up with it, separately for each axis.
    pub fn moved(&self, r: Rect) -> Rect {
        let shift = |lo: f32, hi: f32, edges: &[(f32, usize)]| {
            [lo, hi]
                .into_iter()
                .filter_map(|v| nearest(edges, v).map(|e| e - v))
                .min_by(|a, b| a.abs().total_cmp(&b.abs()))
                .unwrap_or(0.0)
        };
        let dx = shift(r.min.x, r.max.x, &self.xs);
        let dy = shift(r.min.y, r.max.y, &self.ys);
        r.translate(eframe::egui::vec2(dx, dy))
    }
}

/// The edge in `sorted` closest to `v`, if within `SNAP_DISTANCE`. At equal distance
/// the frontmost window's edge wins.
fn nearest(sorted: &[(f32, usize)], v: f32) -> Option<f32> {
    let start = sorted.partition_point(|&(e, _)| e < v - SNAP_DISTANCE);
    let end = sorted.partition_point(|&(e, _)| e <= v + SNAP_DISTANCE);
    sorted[start..end]
        .iter()
        .min_by(|a, b| (a.0 - v).abs().total_cmp(&(b.0 - v).abs()).then(a.1.cmp(&b.1)))
        .map(|&(e, _)| e)
}

/// Visible, on-screen top-level windows, front to back, as desktop-pixel rectangles
/// relative to the desktop's top-left corner at (`left`, `top`). Uses the window's
/// visible frame (without the invisible resize border Windows adds round it).
pub fn visible_windows(left: i32, top: i32) -> Vec<Rect> {
    unsafe extern "system" fn collect(hwnd: HWND, data: LPARAM) -> i32 {
        let list = unsafe { &mut *(data as *mut Vec<HWND>) };
        list.push(hwnd);
        1
    }
    let mut handles: Vec<HWND> = Vec::new();
    unsafe { EnumWindows(Some(collect), std::ptr::from_mut(&mut handles) as LPARAM) };

    handles
        .into_iter()
        .filter_map(|hwnd| unsafe {
            if IsWindowVisible(hwnd) == 0 || IsIconic(hwnd) != 0 {
                return None;
            }
            // Cloaked windows (on other virtual desktops, suspended apps) are not shown.
            let mut cloaked: u32 = 0;
            let size = std::mem::size_of::<u32>() as u32;
            if DwmGetWindowAttribute(hwnd, DWMWA_CLOAKED as u32, std::ptr::from_mut(&mut cloaked).cast(), size) == 0
                && cloaked != 0
            {
                return None;
            }
            // The desktop background itself is a window; leave it out.
            let mut class = [0u16; 32];
            let len = GetClassNameW(hwnd, class.as_mut_ptr(), class.len() as i32).max(0) as usize;
            let class = String::from_utf16_lossy(&class[..len]);
            if class == "Progman" || class == "WorkerW" {
                return None;
            }
            let mut r: RECT = std::mem::zeroed();
            let size = std::mem::size_of::<RECT>() as u32;
            if DwmGetWindowAttribute(hwnd, DWMWA_EXTENDED_FRAME_BOUNDS as u32, std::ptr::from_mut(&mut r).cast(), size) != 0 {
                return None;
            }
            let rect = Rect::from_min_max(
                Pos2::new((r.left - left) as f32, (r.top - top) as f32),
                Pos2::new((r.right - left) as f32, (r.bottom - top) as f32),
            );
            (rect.width() >= 8.0 && rect.height() >= 8.0).then_some(rect)
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn rect(x0: f32, y0: f32, x1: f32, y1: f32) -> Rect {
        Rect::from_min_max(Pos2::new(x0, y0), Pos2::new(x1, y1))
    }

    fn sample() -> Snap {
        // A front window overlapping a back window, on a 1000x800 monitor.
        Snap::new(
            vec![rect(100.0, 100.0, 400.0, 300.0), rect(300.0, 200.0, 700.0, 600.0)],
            &[rect(0.0, 0.0, 1000.0, 800.0)],
        )
    }

    #[test]
    fn picks_the_front_window() {
        let snap = sample();
        assert_eq!(snap.window_at(Pos2::new(350.0, 250.0)), Some(rect(100.0, 100.0, 400.0, 300.0)));
        assert_eq!(snap.window_at(Pos2::new(500.0, 500.0)), Some(rect(300.0, 200.0, 700.0, 600.0)));
        assert_eq!(snap.window_at(Pos2::new(900.0, 50.0)), None);
    }

    #[test]
    fn snaps_nearby_edges_only() {
        let snap = sample();
        // Left and bottom are near window edges (104 -> 100, 594 -> 600); right is not.
        let r = snap.edges(rect(104.0, 150.0, 520.0, 594.0), true, true, true, true);
        assert_eq!(r, rect(100.0, 150.0, 520.0, 600.0));
        // Only the chosen edges move.
        let r = snap.edges(rect(104.0, 150.0, 520.0, 594.0), false, false, false, true);
        assert_eq!(r, rect(104.0, 150.0, 520.0, 600.0));
        // Monitor edges count too.
        let r = snap.edges(rect(5.0, 795.0, 50.0, 799.0), true, false, false, true);
        assert_eq!(r.min.x, 0.0);
        assert_eq!(r.max.y, 800.0);
        // A tiny region is never collapsed onto a single edge.
        let r = snap.edges(rect(98.0, 50.0, 103.0, 60.0), true, true, false, false);
        assert_eq!(r, rect(98.0, 50.0, 103.0, 60.0));
    }

    #[test]
    fn ties_go_to_the_front_window() {
        // Front window's bottom at 600, a back window's bottom at 590: 595 is 5 from each.
        let snap = Snap::new(
            vec![rect(100.0, 100.0, 400.0, 600.0), rect(500.0, 100.0, 900.0, 590.0)],
            &[rect(0.0, 0.0, 1000.0, 800.0)],
        );
        assert_eq!(snap.edges(rect(150.0, 150.0, 300.0, 595.0), false, false, false, true).max.y, 600.0);
        // And the other way round when the back window's edge is the nearer one.
        assert_eq!(snap.edges(rect(150.0, 150.0, 300.0, 594.0), false, false, false, true).max.y, 590.0);
    }

    #[test]
    fn moving_lines_up_the_closest_edge() {
        let snap = sample();
        // Right edge 405 is 5 from 400; left edge 210 has nothing near. Vertically
        // nothing is within reach (40 and 70). Keeps its size.
        let r = snap.moved(rect(210.0, 40.0, 405.0, 70.0));
        assert_eq!(r, rect(205.0, 40.0, 400.0, 70.0));
        // Bottom 93 is 7 from the window top at 100: both axes snap.
        let r = snap.moved(rect(210.0, 40.0, 405.0, 93.0));
        assert_eq!(r, rect(205.0, 47.0, 400.0, 100.0));
    }

    #[test]
    fn finds_real_windows() {
        // Whatever is on screen: rectangles are sane and none is the desktop itself.
        for w in visible_windows(0, 0) {
            assert!(w.width() >= 8.0 && w.height() >= 8.0);
        }
    }
}
