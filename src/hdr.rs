//! Capturing monitors that are showing HDR.
//!
//! The ordinary capture reads 8-bit pixels through GDI. On a monitor with HDR switched
//! on, Windows composes the desktop in a wider, brighter format and what GDI hands back
//! looks washed out. So such a monitor is read in that format instead (linear light as
//! 16-bit floats, through DXGI desktop duplication) and brought back to ordinary sRGB
//! here: ordinary (SDR) content comes out exactly as it was drawn, and anything brighter
//! than SDR white is clipped to white.
//!
//! Monitors without HDR never come this way, and finding out that there are none costs
//! one cheap query, so capturing stays as fast as before on ordinary displays.

use image::RgbaImage;
use windows::core::Interface;
use windows::Win32::Devices::Display::{
    DisplayConfigGetDeviceInfo, GetDisplayConfigBufferSizes, QueryDisplayConfig,
    DISPLAYCONFIG_DEVICE_INFO_GET_ADVANCED_COLOR_INFO, DISPLAYCONFIG_DEVICE_INFO_GET_SDR_WHITE_LEVEL,
    DISPLAYCONFIG_DEVICE_INFO_GET_SOURCE_NAME, DISPLAYCONFIG_DEVICE_INFO_HEADER,
    DISPLAYCONFIG_GET_ADVANCED_COLOR_INFO, DISPLAYCONFIG_MODE_INFO, DISPLAYCONFIG_PATH_INFO,
    DISPLAYCONFIG_SDR_WHITE_LEVEL, DISPLAYCONFIG_SOURCE_DEVICE_NAME, QDC_ONLY_ACTIVE_PATHS,
};
use windows::Win32::Foundation::{ERROR_SUCCESS, HMODULE};
use windows::Win32::Graphics::Direct3D::D3D_DRIVER_TYPE_UNKNOWN;
use windows::Win32::Graphics::Direct3D11::{
    D3D11CreateDevice, ID3D11Device, ID3D11DeviceContext, ID3D11Texture2D, D3D11_CPU_ACCESS_READ,
    D3D11_CREATE_DEVICE_FLAG, D3D11_MAPPED_SUBRESOURCE, D3D11_MAP_READ, D3D11_SDK_VERSION,
    D3D11_TEXTURE2D_DESC, D3D11_USAGE_STAGING,
};
use windows::Win32::Graphics::Dxgi::Common::{
    DXGI_COLOR_SPACE_RGB_FULL_G2084_NONE_P2020, DXGI_FORMAT_R16G16B16A16_FLOAT, DXGI_MODE_ROTATION_IDENTITY,
    DXGI_MODE_ROTATION_UNSPECIFIED,
};
use windows::Win32::Graphics::Dxgi::{
    CreateDXGIFactory1, IDXGIAdapter1, IDXGIFactory1, IDXGIOutput, IDXGIOutput5, IDXGIOutput6,
    DXGI_ERROR_WAIT_TIMEOUT, DXGI_OUTDUPL_FRAME_INFO, DXGI_OUTPUT_DESC1,
};

/// In the format HDR monitors are read in, 1.0 is this many nits.
const UNIT_NITS: f32 = 80.0;
/// SDR white when Windows will not say: its usual default.
const FALLBACK_WHITE_NITS: f32 = 200.0;

/// The monitors with HDR switched on.
pub struct Hdr {
    /// Each one's GDI device name (`\\.\DISPLAY1`) and the brightness of SDR white on
    /// it in nits (the "SDR content brightness" slider in Windows' HDR settings).
    monitors: Vec<([u16; 32], f32)>,
}

impl Hdr {
    /// `None` when no monitor has HDR on, which is the usual case.
    pub fn detect() -> Option<Self> {
        let monitors = unsafe { hdr_monitors() };
        (!monitors.is_empty()).then_some(Self { monitors })
    }

    /// Capture the monitor whose top-left corner is at (`x`, `y`) on the desktop, if
    /// it is one showing HDR. `None` (use the ordinary capture) if it is not, or if
    /// anything about reading it fails.
    pub fn capture(&self, x: i32, y: i32) -> Option<RgbaImage> {
        unsafe {
            capture_at(x, y, |output| {
                // Only when the output really is in HDR: in any other mode the floats
                // it hands over are not linear light, and the conversion would be wrong.
                let hdr = output.ColorSpace == DXGI_COLOR_SPACE_RGB_FULL_G2084_NONE_P2020;
                self.monitors.iter().find(|m| hdr && m.0 == output.DeviceName).map(|m| m.1)
            })
        }
    }
}

/// Ask Windows which active displays have HDR on, and how bright SDR white is on each.
unsafe fn hdr_monitors() -> Vec<([u16; 32], f32)> {
    let mut found = Vec::new();
    unsafe {
        let (mut path_count, mut mode_count) = (0u32, 0u32);
        if GetDisplayConfigBufferSizes(QDC_ONLY_ACTIVE_PATHS, &mut path_count, &mut mode_count) != ERROR_SUCCESS {
            return found;
        }
        let mut paths: Vec<DISPLAYCONFIG_PATH_INFO> = vec![std::mem::zeroed(); path_count as usize];
        let mut modes: Vec<DISPLAYCONFIG_MODE_INFO> = vec![std::mem::zeroed(); mode_count as usize];
        let queried = QueryDisplayConfig(
            QDC_ONLY_ACTIVE_PATHS,
            &mut path_count,
            paths.as_mut_ptr(),
            &mut mode_count,
            modes.as_mut_ptr(),
            None,
        );
        if queried != ERROR_SUCCESS {
            return found;
        }
        paths.truncate(path_count as usize);

        for path in &paths {
            let target = |kind, size: usize| DISPLAYCONFIG_DEVICE_INFO_HEADER {
                r#type: kind,
                size: size as u32,
                adapterId: path.targetInfo.adapterId,
                id: path.targetInfo.id,
            };
            let mut color: DISPLAYCONFIG_GET_ADVANCED_COLOR_INFO = std::mem::zeroed();
            color.header = target(DISPLAYCONFIG_DEVICE_INFO_GET_ADVANCED_COLOR_INFO, std::mem::size_of_val(&color));
            // Bit 1 of the flags: advanced colour (HDR) is enabled.
            if DisplayConfigGetDeviceInfo(&mut color.header) != 0 || color.Anonymous.value & 2 == 0 {
                continue;
            }

            let mut source: DISPLAYCONFIG_SOURCE_DEVICE_NAME = std::mem::zeroed();
            source.header = DISPLAYCONFIG_DEVICE_INFO_HEADER {
                r#type: DISPLAYCONFIG_DEVICE_INFO_GET_SOURCE_NAME,
                size: std::mem::size_of_val(&source) as u32,
                adapterId: path.sourceInfo.adapterId,
                id: path.sourceInfo.id,
            };
            if DisplayConfigGetDeviceInfo(&mut source.header) != 0 {
                continue;
            }

            let mut white: DISPLAYCONFIG_SDR_WHITE_LEVEL = std::mem::zeroed();
            white.header = target(DISPLAYCONFIG_DEVICE_INFO_GET_SDR_WHITE_LEVEL, std::mem::size_of_val(&white));
            // Reported in thousandths of 80 nits.
            let nits = if DisplayConfigGetDeviceInfo(&mut white.header) == 0 && white.SDRWhiteLevel > 0 {
                white.SDRWhiteLevel as f32 / 1000.0 * UNIT_NITS
            } else {
                FALLBACK_WHITE_NITS
            };
            found.push((source.viewGdiDeviceName, nits));
        }
    }
    found
}

/// Find the display output at (`x`, `y`) and capture it if `white_of` says to
/// (returning the brightness of SDR white on it, in nits).
unsafe fn capture_at(x: i32, y: i32, white_of: impl Fn(&DXGI_OUTPUT_DESC1) -> Option<f32>) -> Option<RgbaImage> {
    unsafe {
        let factory: IDXGIFactory1 = CreateDXGIFactory1().ok()?;
        for adapter in (0..).map_while(|i| factory.EnumAdapters1(i).ok()) {
            for output in (0..).map_while(|i| adapter.EnumOutputs(i).ok()) {
                let Ok(desc) = output.cast::<IDXGIOutput6>().and_then(|o| o.GetDesc1()) else { continue };
                if (desc.DesktopCoordinates.left, desc.DesktopCoordinates.top) != (x, y) {
                    continue;
                }
                let white = white_of(&desc)?;
                // A rotated monitor is handed over unrotated: leave it to the ordinary
                // capture rather than turn it here.
                let upright = [DXGI_MODE_ROTATION_IDENTITY, DXGI_MODE_ROTATION_UNSPECIFIED].contains(&desc.Rotation);
                if !upright {
                    return None;
                }
                return duplicate(&adapter, &output, white).ok().flatten();
            }
        }
        None
    }
}

/// Read one frame of `output` as linear 16-bit floats and convert it to sRGB.
/// `Ok(None)` if no usable frame arrived.
unsafe fn duplicate(adapter: &IDXGIAdapter1, output: &IDXGIOutput, white_nits: f32) -> windows::core::Result<Option<RgbaImage>> {
    unsafe {
        let mut device: Option<ID3D11Device> = None;
        let mut context: Option<ID3D11DeviceContext> = None;
        D3D11CreateDevice(
            adapter,
            D3D_DRIVER_TYPE_UNKNOWN,
            HMODULE::default(),
            D3D11_CREATE_DEVICE_FLAG(0),
            None,
            D3D11_SDK_VERSION,
            Some(&mut device),
            None,
            Some(&mut context),
        )?;
        let (Some(device), Some(context)) = (device, context) else { return Ok(None) };
        let duplication = output.cast::<IDXGIOutput5>()?.DuplicateOutput1(&device, 0, &[DXGI_FORMAT_R16G16B16A16_FLOAT])?;

        // The first frame normally arrives at once and holds the whole desktop. A frame
        // with no present time carries no picture (only a mouse move): try again.
        for _ in 0..4 {
            let mut info = DXGI_OUTDUPL_FRAME_INFO::default();
            let mut resource = None;
            match duplication.AcquireNextFrame(60, &mut info, &mut resource) {
                Ok(()) => {}
                Err(e) if e.code() == DXGI_ERROR_WAIT_TIMEOUT => continue,
                Err(e) => return Err(e),
            }
            let Some(resource) = resource.filter(|_| info.LastPresentTime != 0) else {
                duplication.ReleaseFrame()?;
                continue;
            };
            let texture: ID3D11Texture2D = resource.cast()?;
            let mut desc = D3D11_TEXTURE2D_DESC::default();
            texture.GetDesc(&mut desc);
            if desc.Format != DXGI_FORMAT_R16G16B16A16_FLOAT {
                duplication.ReleaseFrame()?;
                return Ok(None);
            }
            // A copy the CPU can read.
            desc.Usage = D3D11_USAGE_STAGING;
            desc.BindFlags = 0;
            desc.CPUAccessFlags = D3D11_CPU_ACCESS_READ.0 as u32;
            desc.MiscFlags = 0;
            let mut staging = None;
            device.CreateTexture2D(&desc, None, Some(&mut staging))?;
            let Some(staging) = staging else { return Ok(None) };
            context.CopyResource(&staging, &texture);
            let mut mapped = D3D11_MAPPED_SUBRESOURCE::default();
            context.Map(&staging, 0, D3D11_MAP_READ, 0, Some(&mut mapped))?;

            let (w, h) = (desc.Width as usize, desc.Height as usize);
            let table = tone_table(white_nits);
            let mut pixels = vec![255u8; w * h * 4];
            for (row, out) in pixels.chunks_exact_mut(w * 4).enumerate() {
                let line = mapped.pData.cast::<u8>().add(row * mapped.RowPitch as usize).cast::<[u16; 4]>();
                let line = std::slice::from_raw_parts(line, w);
                for (dst, src) in out.chunks_exact_mut(4).zip(line) {
                    for c in 0..3 {
                        dst[c] = table[src[c] as usize];
                    }
                }
            }
            context.Unmap(&staging, 0);
            duplication.ReleaseFrame()?;
            return Ok(RgbaImage::from_raw(w as u32, h as u32, pixels));
        }
        Ok(None)
    }
}

/// For every 16-bit float, by its bits: the sRGB value it becomes when SDR white is
/// `white_nits` bright. One table lookup per channel then converts a whole screen.
fn tone_table(white_nits: f32) -> Vec<u8> {
    let gain = UNIT_NITS / white_nits.max(1.0);
    (0..=u16::MAX)
        .map(|bits| {
            let linear = half(bits) * gain;
            // Darker than black (colours outside sRGB) and not-a-number become black;
            // brighter than SDR white becomes white.
            let linear = if linear.is_nan() { 0.0 } else { linear.clamp(0.0, 1.0) };
            let encoded = if linear <= 0.003_130_8 { linear * 12.92 } else { 1.055 * linear.powf(1.0 / 2.4) - 0.055 };
            (encoded * 255.0).round() as u8
        })
        .collect()
}

/// The value of a 16-bit float, given its bits.
fn half(bits: u16) -> f32 {
    let sign = if bits & 0x8000 == 0 { 1.0 } else { -1.0 };
    let fraction = f32::from(bits & 0x3FF);
    let value = match (bits >> 10) & 0x1F {
        0 => fraction * 2f32.powi(-24),
        31 if fraction == 0.0 => f32::INFINITY,
        31 => f32::NAN,
        exponent => (1.0 + fraction / 1024.0) * 2f32.powi(i32::from(exponent) - 15),
    };
    sign * value
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn halves() {
        assert_eq!(half(0x0000), 0.0);
        assert_eq!(half(0x3C00), 1.0);
        assert_eq!(half(0x3800), 0.5);
        assert_eq!(half(0x4000), 2.0);
        assert_eq!(half(0xBC00), -1.0);
        assert_eq!(half(0x0001), 2f32.powi(-24));
        assert_eq!(half(0x7C00), f32::INFINITY);
        assert!(half(0x7E00).is_nan());
    }

    #[test]
    fn sdr_white_becomes_white_whatever_its_brightness() {
        // SDR white at 80 nits is 1.0: white. Half of it is sRGB 188.
        let t = tone_table(80.0);
        assert_eq!((t[0x3C00], t[0x3800], t[0x0000]), (255, 188, 0));
        // Brighter than white, infinity: clipped. Negative and not-a-number: black.
        assert_eq!((t[0x4000], t[0x7C00], t[0xBC00], t[0x7E00], t[0xFC00]), (255, 255, 0, 0, 0));
        // With SDR white at 160 nits, white is 2.0 and 1.0 is half of it.
        let t = tone_table(160.0);
        assert_eq!((t[0x4000], t[0x3C00]), (255, 188));
        // The table never goes down as the value goes up.
        let positive = &t[..0x7C00];
        assert!(positive.windows(2).all(|w| w[0] <= w[1]));
    }

    /// Needs a real desktop, with nothing on it changing: run by hand with
    /// `cargo test hdr -- --ignored --nocapture`. Reads the main monitor both ways and
    /// compares them, which checks everything but the HDR conversion itself: without HDR
    /// the floats are the ordinary sRGB values, so converting them as if they were linear
    /// light must give the ordinary capture with that same conversion applied.
    #[test]
    #[ignore]
    fn the_main_monitor_reads_the_same_both_ways() {
        use windows_sys::Win32::UI::HiDpi::{SetProcessDpiAwarenessContext, DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE_V2};
        unsafe { SetProcessDpiAwarenessContext(DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE_V2) };
        let found = unsafe { hdr_monitors() };
        println!("monitors with HDR on: {}", found.len());
        for (name, nits) in &found {
            println!("  {} SDR white {nits} nits", String::from_utf16_lossy(name).trim_end_matches(' '));
        }
        if !found.is_empty() {
            println!("HDR is on: this check only means something with HDR off");
            return;
        }
        let main = xcap::Monitor::all().unwrap().into_iter().find(|m| m.is_primary().unwrap_or(false)).unwrap();
        let (x, y) = (main.x().unwrap(), main.y().unwrap());
        let plain = main.capture_image().unwrap();
        let wide = unsafe { capture_at(x, y, |_| Some(UNIT_NITS)) }.expect("the main monitor can be duplicated");
        assert_eq!(wide.dimensions(), plain.dimensions());
        let encode = |c: u8| {
            let v = f32::from(c) / 255.0;
            let v = if v <= 0.003_130_8 { v * 12.92 } else { 1.055 * v.powf(1.0 / 2.4) - 0.055 };
            (v * 255.0).round() as u8
        };
        let off = wide.pixels().zip(plain.pixels()).filter(|(a, b)| {
            a.0.iter().zip(&b.0[..3]).any(|(p, q)| p.abs_diff(encode(*q)) > 2)
        });
        let (off, total) = (off.count(), wide.pixels().len());
        println!("{off} of {total} pixels differ by more than 2 levels");
        assert!(off * 20 < total, "more than 5% of pixels differ");
    }
}
