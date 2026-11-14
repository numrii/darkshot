//! Reopen last capture (opt-in): an overlay started from the tray hides when it is
//! done instead of quitting, keeping its capture, selection and annotations in memory
//! only, never on disk. The tray can bring it back. It quits after the time set in the
//! settings, and the tray ends it when the next capture starts.
//!
//! The two processes share two named events: one the overlay sets while it is hidden
//! and keeping a capture, and one the tray sets to bring it back.

use windows_sys::Win32::Foundation::WAIT_OBJECT_0;
use windows_sys::Win32::System::Threading::{CreateEventW, ResetEvent, SetEvent, WaitForSingleObject};

/// A named event. The handle is kept as a number so it can cross threads, and never
/// closed: it lives as long as the process.
#[derive(Clone, Copy, Debug)]
pub struct Event(usize);

impl Event {
    fn open(name: &str, manual_reset: bool) -> Option<Self> {
        let wide: Vec<u16> = name.encode_utf16().chain(Some(0)).collect();
        let handle = unsafe { CreateEventW(std::ptr::null(), i32::from(manual_reset), 0, wide.as_ptr()) };
        (!handle.is_null()).then_some(Self(handle as usize))
    }

    pub fn set(self) {
        unsafe { SetEvent(self.0 as _) };
    }

    pub fn reset(self) {
        unsafe { ResetEvent(self.0 as _) };
    }

    pub fn is_set(self) -> bool {
        self.wait(0)
    }

    /// Wait up to `ms` milliseconds for it to be set. Returns whether it was.
    pub fn wait(self, ms: u32) -> bool {
        unsafe { WaitForSingleObject(self.0 as _, ms) == WAIT_OBJECT_0 }
    }
}

/// Set while an overlay is hidden and keeping its capture.
pub fn kept() -> Option<Event> {
    Event::open("Local\\DarkshotKept", true)
}

/// Set by the tray to bring a kept overlay back.
pub fn reopen() -> Option<Event> {
    Event::open("Local\\DarkshotReopen", false)
}
