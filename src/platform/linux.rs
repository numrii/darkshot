//! Minimal wlroots screencopy implementation for the Linux editor path.
//!
//! This intentionally obtains one output and then opens the ordinary egui editor.
//! It does not create an overlay, layer-shell surface, daemon, or global shortcut.

use std::fs::{self, File, OpenOptions};
use std::io::{Read, Seek, SeekFrom};
use std::os::fd::AsFd;
use std::path::PathBuf;
use std::sync::atomic::{AtomicU64, Ordering};

use image::RgbaImage;
use wayland_client::{
    delegate_noop,
    globals::{registry_queue_init, GlobalListContents},
    protocol::{
        wl_buffer::WlBuffer,
        wl_output::WlOutput,
        wl_registry::{self, WlRegistry},
        wl_shm::{self, WlShm},
        wl_shm_pool::WlShmPool,
    },
    Connection, Dispatch, QueueHandle, WEnum,
};
use wayland_protocols_wlr::screencopy::v1::client::{
    zwlr_screencopy_frame_v1::{self, ZwlrScreencopyFrameV1},
    zwlr_screencopy_manager_v1::ZwlrScreencopyManagerV1,
};

use super::{Capture, Output};
use crate::AnyResult;

#[derive(Clone, Copy)]
struct BufferSpec {
    format: wl_shm::Format,
    width: u32,
    height: u32,
    stride: u32,
}

#[derive(Default)]
struct State {
    buffer: Option<BufferSpec>,
    buffer_done: bool,
    ready: bool,
    failed: bool,
    y_invert: bool,
}

impl Dispatch<WlRegistry, GlobalListContents> for State {
    fn event(
        _: &mut Self,
        _: &WlRegistry,
        _: wl_registry::Event,
        _: &GlobalListContents,
        _: &Connection,
        _: &QueueHandle<Self>,
    ) {
    }
}

impl Dispatch<ZwlrScreencopyFrameV1, ()> for State {
    fn event(
        state: &mut Self,
        _: &ZwlrScreencopyFrameV1,
        event: zwlr_screencopy_frame_v1::Event,
        _: &(),
        _: &Connection,
        _: &QueueHandle<Self>,
    ) {
        match event {
            zwlr_screencopy_frame_v1::Event::Buffer {
                format: WEnum::Value(format),
                width,
                height,
                stride,
            } => {
                state.buffer = Some(BufferSpec {
                    format,
                    width,
                    height,
                    stride,
                });
            }
            zwlr_screencopy_frame_v1::Event::Buffer { .. } => {}
            zwlr_screencopy_frame_v1::Event::BufferDone => state.buffer_done = true,
            zwlr_screencopy_frame_v1::Event::Ready { .. } => state.ready = true,
            zwlr_screencopy_frame_v1::Event::Failed => state.failed = true,
            zwlr_screencopy_frame_v1::Event::Flags {
                flags: WEnum::Value(flags),
            } => {
                state.y_invert = flags.bits() & 1 != 0;
            }
            _ => {}
        }
    }
}

delegate_noop!(State: ignore WlOutput);
delegate_noop!(State: ignore WlShm);
delegate_noop!(State: ignore WlShmPool);
delegate_noop!(State: ignore WlBuffer);
delegate_noop!(State: ignore ZwlrScreencopyManagerV1);

/// Capture the first compositor output via `zwlr_screencopy_manager_v1`.
pub(super) fn capture(include_cursor: bool) -> AnyResult<Capture> {
    let connection =
        Connection::connect_to_env().map_err(|e| format!("could not connect to Wayland: {e}"))?;
    let (globals, mut queue) = registry_queue_init::<State>(&connection)?;
    let handle = queue.handle();

    let output_global = globals
        .contents()
        .clone_list()
        .into_iter()
        .find(|global| global.interface == "wl_output")
        .ok_or("the Wayland compositor did not advertise an output")?;
    let output = globals.registry().bind::<WlOutput, _, _>(
        output_global.name,
        output_global.version.min(4),
        &handle,
        (),
    );
    let manager = globals
        .bind::<ZwlrScreencopyManagerV1, _, _>(&handle, 1..=3, ())
        .map_err(|_| "this Wayland compositor does not support wlroots screencopy")?;
    let shm = globals
        .bind::<WlShm, _, _>(&handle, 1..=1, ())
        .map_err(|_| "this Wayland compositor does not support shared-memory capture buffers")?;

    let frame = manager.capture_output(i32::from(include_cursor), &output, &handle, ());
    let mut state = State::default();
    while !state.buffer_done && !state.failed {
        queue.blocking_dispatch(&mut state)?;
    }
    if state.failed {
        return Err("the compositor rejected the screen capture".into());
    }
    let spec = state
        .buffer
        .ok_or("the compositor supplied no shared-memory screen format")?;
    if !matches!(
        spec.format,
        wl_shm::Format::Argb8888 | wl_shm::Format::Xrgb8888 | wl_shm::Format::Bgr888
    ) {
        return Err(format!("unsupported wlroots screen format: {:?}", spec.format).into());
    }
    let bytes = usize::try_from(u64::from(spec.stride) * u64::from(spec.height))?;
    let mut file = shared_file(bytes)?;
    let pool = shm.create_pool(file.as_fd(), i32::try_from(bytes)?, &handle, ());
    let buffer = pool.create_buffer(
        0,
        i32::try_from(spec.width)?,
        i32::try_from(spec.height)?,
        i32::try_from(spec.stride)?,
        spec.format,
        &handle,
        (),
    );
    frame.copy(&buffer);
    while !state.ready && !state.failed {
        queue.blocking_dispatch(&mut state)?;
    }
    if state.failed {
        return Err("the compositor could not copy the screen into the shared buffer".into());
    }

    let mut raw = vec![0; bytes];
    file.seek(SeekFrom::Start(0))?;
    file.read_exact(&mut raw)?;
    let image = rgba_from_bgra(&raw, spec, state.y_invert)?;
    Ok(Capture {
        outputs: vec![Output { x: 0, y: 0, image }],
        pointer: None,
    })
}

/// wl_shm ARGB/XRGB pixels are little-endian BGRA/BGRX bytes; BGR888 is three
/// BGR bytes. Convert either while dropping compositor-selected row padding.
fn rgba_from_bgra(raw: &[u8], spec: BufferSpec, y_invert: bool) -> AnyResult<RgbaImage> {
    let width = usize::try_from(spec.width)?;
    let height = usize::try_from(spec.height)?;
    let stride = usize::try_from(spec.stride)?;
    let bytes_per_pixel = if spec.format == wl_shm::Format::Bgr888 {
        3
    } else {
        4
    };
    let row = width
        .checked_mul(bytes_per_pixel)
        .ok_or("capture image is too wide")?;
    if stride < row
        || raw.len()
            < stride
                .checked_mul(height)
                .ok_or("capture image is too tall")?
    {
        return Err("invalid shared-memory screen buffer dimensions".into());
    }
    let rgba_row = width.checked_mul(4).ok_or("capture image is too wide")?;
    let mut rgba = vec![
        0;
        rgba_row
            .checked_mul(height)
            .ok_or("capture image is too large")?
    ];
    for y in 0..height {
        let source_y = if y_invert { height - 1 - y } else { y };
        for x in 0..width {
            let src = source_y * stride + x * bytes_per_pixel;
            let dst = (y * width + x) * 4;
            rgba[dst..dst + 3].copy_from_slice(&[raw[src + 2], raw[src + 1], raw[src]]);
            rgba[dst + 3] = if spec.format == wl_shm::Format::Argb8888 {
                raw[src + 3]
            } else {
                255
            };
        }
    }
    RgbaImage::from_raw(spec.width, spec.height, rgba)
        .ok_or_else(|| "invalid capture image data".into())
}

fn shared_file(bytes: usize) -> std::io::Result<File> {
    static NEXT: AtomicU64 = AtomicU64::new(0);
    let directory = std::env::var_os("XDG_RUNTIME_DIR")
        .map(PathBuf::from)
        .unwrap_or_else(std::env::temp_dir);
    for _ in 0..16 {
        let path = directory.join(format!(
            "duskshot-screencopy-{}-{}",
            std::process::id(),
            NEXT.fetch_add(1, Ordering::Relaxed)
        ));
        match OpenOptions::new()
            .read(true)
            .write(true)
            .create_new(true)
            .open(&path)
        {
            Ok(file) => {
                file.set_len(bytes as u64)?;
                fs::remove_file(path)?;
                return Ok(file);
            }
            Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => continue,
            Err(e) => return Err(e),
        }
    }
    Err(std::io::Error::new(
        std::io::ErrorKind::AlreadyExists,
        "could not allocate a Wayland capture buffer",
    ))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn converts_bgra_and_stride_to_editor_rgba() {
        let spec = BufferSpec {
            format: wl_shm::Format::Argb8888,
            width: 2,
            height: 2,
            stride: 12,
        };
        // Two BGRA pixels plus four padding bytes on each row.
        let raw = [
            3, 2, 1, 4, 7, 6, 5, 8, 0, 0, 0, 0, 11, 10, 9, 12, 15, 14, 13, 16, 0, 0, 0, 0,
        ];
        let image = rgba_from_bgra(&raw, spec, false).unwrap();
        assert_eq!(
            image.as_raw(),
            &[1, 2, 3, 4, 5, 6, 7, 8, 9, 10, 11, 12, 13, 14, 15, 16]
        );
        let flipped = rgba_from_bgra(&raw, spec, true).unwrap();
        assert_eq!(flipped.get_pixel(0, 0).0, [9, 10, 11, 12]);
    }

    #[test]
    fn converts_24_bit_bgr() {
        let spec = BufferSpec {
            format: wl_shm::Format::Bgr888,
            width: 2,
            height: 1,
            stride: 8,
        };
        let image = rgba_from_bgra(&[3, 2, 1, 7, 6, 5, 0, 0], spec, false).unwrap();
        assert_eq!(image.as_raw(), &[1, 2, 3, 255, 5, 6, 7, 255]);
    }
}
