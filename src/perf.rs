//! Resource log for `--trace`: memory, CPU, handles and GPU memory, one line per sample,
//! appended to `duskshot-perf.log` in the temp folder.
//!
//! Made for comparing versions and finding leaks, not for watching the app live, so it
//! samples at moments that can be compared from run to run: the tray at start, after each
//! capture window closes, when a kept capture is let go, and once a minute while idle;
//! the capture window at start, once it is on screen, every 10 seconds while open, and as
//! it closes. Each sample is a handful of system calls (well under a millisecond; the
//! `sample_us` column says how long) and goes straight to the file, so the log keeps
//! nothing in memory.
//!
//! The tray never asks for GPU memory: it holds no GPU resources, and asking would load
//! the graphics libraries into it and change what is being measured.
//!
//! Only builds with the `trace` feature, and debug builds, can log at all; the
//! restrictive build writes no diagnostic files. `--trace-cycles N` (see `daemon::cycles`)
//! opens and closes the capture window N times for numbers that compare across versions;
//! `scripts/perf-summary.ps1` sums a log up.

use std::fs::File;
use std::io::Write;
use std::sync::atomic::{AtomicU32, AtomicU64, Ordering};
use std::sync::{Mutex, OnceLock};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

const HEADER: &str = "time_ms,pid,role,event,private_kb,working_set_kb,peak_working_set_kb,\
                      cpu_ms,handles,gdi_objects,user_objects,gpu_local_kb,gpu_shared_kb,sample_us";

/// How often the capture window samples while it is open.
const OPEN_EVERY: Duration = Duration::from_secs(10);
/// How often the tray samples while idle.
pub const IDLE_EVERY: Duration = Duration::from_secs(60);

/// True when this run logs resources: `--trace` or `--trace-cycles`, in a build that can.
pub fn enabled() -> bool {
    if !cfg!(any(feature = "trace", debug_assertions)) {
        return false;
    }
    static ON: OnceLock<bool> = OnceLock::new();
    *ON.get_or_init(|| std::env::args().any(|a| a == "--trace" || a == "--trace-cycles"))
}

/// True when the capture window should close by itself once it is on screen
/// (`--trace-cycles` starts it so).
pub fn autoclose() -> bool {
    enabled() && std::env::args().any(|a| a == "--trace-autoclose")
}

fn sink() -> Option<&'static Mutex<File>> {
    static SINK: OnceLock<Option<Mutex<File>>> = OnceLock::new();
    SINK.get_or_init(|| {
        let path = std::env::temp_dir().join("duskshot-perf.log");
        // Appended, not replaced: the tray and every capture window write to the same
        // log, and a log that spans versions is the point.
        let mut file = std::fs::OpenOptions::new().create(true).append(true).open(path).ok()?;
        if file.metadata().ok()?.len() == 0 {
            let _ = writeln!(file, "{HEADER}");
        }
        Some(Mutex::new(file))
    })
    .as_ref()
}

/// Which process this is, from its flags.
fn role() -> &'static str {
    static ROLE: OnceLock<&str> = OnceLock::new();
    ROLE.get_or_init(|| {
        let args: Vec<String> = std::env::args().skip(1).collect();
        let has = |f: &str| args.iter().any(|a| a == f);
        if has("--settings") {
            "settings"
        } else if has("--trace-cycles") && !has("--trace-autoclose") {
            "cycles"
        } else if ["--capture", "--reopen", "--clipboard", "--open"].iter().any(|f| has(f))
            || args.iter().any(|a| !a.starts_with("--"))
        {
            "overlay"
        } else {
            "tray"
        }
    })
}

/// Log one sample, labelled `event` (no commas), when logging is on.
pub fn sample(event: &str) {
    if !enabled() {
        return;
    }
    let Some(sink) = sink() else { return };
    let began = Instant::now();
    let p = process();
    let (gpu_local, gpu_shared) = if role() == "tray" { (0, 0) } else { gpu_memory() };
    let sample_us = began.elapsed().as_micros();
    let time_ms = SystemTime::now().duration_since(UNIX_EPOCH).map_or(0, |d| d.as_millis());
    let line = format!(
        "{time_ms},{},{},{event},{},{},{},{},{},{},{},{},{},{sample_us}\n",
        std::process::id(),
        role(),
        p.private / 1024,
        p.working_set / 1024,
        p.peak_working_set / 1024,
        p.cpu_ms,
        p.handles,
        p.gdi,
        p.user,
        gpu_local / 1024,
        gpu_shared / 1024,
    );
    // One write per line: with the file opened to append, Windows puts each write whole
    // at the end, so lines from the tray and a capture window never interleave.
    let _ = sink.lock().unwrap().write_all(line.as_bytes());
}

/// Called at the start of every capture-window frame. Samples once the capture is on
/// screen (the second frame: the first one uploads it to the GPU) and every
/// `OPEN_EVERY` after. Returns true when the window should now close (`autoclose`).
pub fn frame(ctx: &eframe::egui::Context) -> bool {
    if !enabled() {
        return false;
    }
    static FRAMES: AtomicU32 = AtomicU32::new(0);
    static LAST_MS: AtomicU64 = AtomicU64::new(0);
    static START: OnceLock<Instant> = OnceLock::new();
    let now_ms = START.get_or_init(Instant::now).elapsed().as_millis() as u64;
    let frame = FRAMES.fetch_add(1, Ordering::Relaxed) + 1;
    match frame {
        1 => ctx.request_repaint(), // draw the second frame even without input
        2 => {
            sample("shown");
            LAST_MS.store(now_ms, Ordering::Relaxed);
            return autoclose();
        }
        _ if now_ms - LAST_MS.load(Ordering::Relaxed) >= OPEN_EVERY.as_millis() as u64 => {
            sample("open");
            LAST_MS.store(now_ms, Ordering::Relaxed);
        }
        _ => {}
    }
    ctx.request_repaint_after(OPEN_EVERY);
    false
}

#[derive(Default)]
struct Process {
    private: usize,
    working_set: usize,
    peak_working_set: usize,
    cpu_ms: u64,
    handles: u32,
    gdi: u32,
    user: u32,
}

fn process() -> Process {
    use windows_sys::Win32::Foundation::FILETIME;
    use windows_sys::Win32::System::ProcessStatus::{K32GetProcessMemoryInfo, PROCESS_MEMORY_COUNTERS_EX};
    use windows_sys::Win32::System::Threading::{
        GetCurrentProcess, GetGuiResources, GetProcessHandleCount, GetProcessTimes, GR_GDIOBJECTS, GR_USEROBJECTS,
    };
    let mut p = Process::default();
    unsafe {
        let me = GetCurrentProcess();
        let mut mem: PROCESS_MEMORY_COUNTERS_EX = std::mem::zeroed();
        mem.cb = size_of::<PROCESS_MEMORY_COUNTERS_EX>() as u32;
        if K32GetProcessMemoryInfo(me, (&raw mut mem).cast(), mem.cb) != 0 {
            p.private = mem.PrivateUsage;
            p.working_set = mem.WorkingSetSize;
            p.peak_working_set = mem.PeakWorkingSetSize;
        }
        let zero = FILETIME { dwLowDateTime: 0, dwHighDateTime: 0 };
        let (mut created, mut exited, mut kernel, mut user) = (zero, zero, zero, zero);
        if GetProcessTimes(me, &mut created, &mut exited, &mut kernel, &mut user) != 0 {
            let ticks = |t: FILETIME| (u64::from(t.dwHighDateTime) << 32) | u64::from(t.dwLowDateTime);
            p.cpu_ms = (ticks(kernel) + ticks(user)) / 10_000; // 100 ns ticks
        }
        GetProcessHandleCount(me, &mut p.handles);
        p.gdi = GetGuiResources(me, GR_GDIOBJECTS);
        p.user = GetGuiResources(me, GR_USEROBJECTS);
    }
    p
}

/// This process's GPU memory in bytes, summed over the adapters: (local, shared).
/// Local is the graphics card's own memory; shared is system memory the GPU uses (all
/// of it, on integrated graphics).
fn gpu_memory() -> (u64, u64) {
    use windows::core::Interface;
    use windows::Win32::Graphics::Dxgi::{
        CreateDXGIFactory1, IDXGIAdapter3, IDXGIFactory1, DXGI_MEMORY_SEGMENT_GROUP_LOCAL,
        DXGI_MEMORY_SEGMENT_GROUP_NON_LOCAL, DXGI_QUERY_VIDEO_MEMORY_INFO,
    };
    let (mut local, mut shared) = (0, 0);
    unsafe {
        let Ok(factory) = CreateDXGIFactory1::<IDXGIFactory1>() else { return (0, 0) };
        let mut i = 0;
        while let Ok(adapter) = factory.EnumAdapters1(i) {
            i += 1;
            let Ok(adapter) = adapter.cast::<IDXGIAdapter3>() else { continue };
            let mut info = DXGI_QUERY_VIDEO_MEMORY_INFO::default();
            if adapter.QueryVideoMemoryInfo(0, DXGI_MEMORY_SEGMENT_GROUP_LOCAL, &mut info).is_ok() {
                local += info.CurrentUsage;
            }
            if adapter.QueryVideoMemoryInfo(0, DXGI_MEMORY_SEGMENT_GROUP_NON_LOCAL, &mut info).is_ok() {
                shared += info.CurrentUsage;
            }
        }
    }
    (local, shared)
}
