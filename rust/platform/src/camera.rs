//! The camera: the C component `app_camera`
//! (`components/app_camera/include/app_camera.h`, this repo) on the board's
//! side of [`logic::sys::Sys::camera`].
//!
//! Start and stop return at once; app_camera's own task opens the module and
//! pushes the preview straight to the scene's `scan_preview` Canvas, without
//! the logic. Each distinct QR payload of a session comes back through a C
//! callback on app_camera's decoder task, which copies it into a boxed
//! [`Event::Scanned`] for the logic task's queue without blocking; a start
//! that fails comes back once as [`Event::NoCamera`].

use alloc::string::String;
use core::ffi::{c_char, c_int, c_void};

use gsp_sys as gsp;
use logic::sys::Event;

use crate::{EventQueue, idf, post};

type EspErr = c_int;
const ESP_OK: EspErr = 0;

type CodeCb = unsafe extern "C" fn(*const c_char, usize, *mut c_void);
type FailedCb = unsafe extern "C" fn(*mut c_void);

unsafe extern "C" {
    fn app_camera_subscribe(
        on_code: Option<CodeCb>,
        on_failed: Option<FailedCb>,
        ctx: *mut c_void,
    ) -> EspErr;
    fn app_camera_start(preview_bind: u16) -> EspErr;
    fn app_camera_stop() -> EspErr;
}

fn warn(message: &str) {
    idf::log_line(idf::LogLevel::Warn, c"camera", message);
}

/// Registers the logic's queue as the consumer of codes and failed starts.
pub fn subscribe(queue: &'static EventQueue) {
    let ctx = (queue as *const EventQueue).cast_mut().cast::<c_void>();
    // SAFETY: the callback matches the C signature and the queue is leaked
    // for the rest of the boot.
    let err = unsafe { app_camera_subscribe(Some(on_code), Some(on_failed), ctx) };
    if err != ESP_OK {
        warn(&alloc::format!("camera subscribe failed (0x{err:x})"));
    }
}

/// Opens (true) or closes (false) a scan session.
pub fn set(on: bool) {
    // The scene's Canvas image; binds fit u16 (GSP's bind type).
    const PREVIEW: u16 = gsp::GSP_BIND_SCAN_PREVIEW as u16;
    // SAFETY: plain calls; app_camera only records the request.
    let err = unsafe {
        if on {
            app_camera_start(PREVIEW)
        } else {
            app_camera_stop()
        }
    };
    if err != ESP_OK {
        warn(&alloc::format!("camera {on} failed (0x{err:x})"));
    }
}

/// app_camera's decoder task: a payload not seen before in this session.
unsafe extern "C" fn on_code(payload: *const c_char, length: usize, ctx: *mut c_void) {
    // SAFETY: ctx is the queue passed to subscribe().
    let Some(queue) = (unsafe { ctx.cast::<EventQueue>().as_ref() }) else {
        return;
    };
    if payload.is_null() {
        return;
    }
    // SAFETY: app_camera passes `length` readable bytes for the call.
    let bytes = unsafe { core::slice::from_raw_parts(payload.cast::<u8>(), length) };
    let text = String::from_utf8_lossy(bytes).into_owned();
    if !post(queue, Event::Scanned(text)) {
        warn("scanned code dropped: logic queue full");
    }
}

/// app_camera's session task (or the logic task, when the session task could
/// not be created): this start failed.
unsafe extern "C" fn on_failed(ctx: *mut c_void) {
    // SAFETY: ctx is the queue passed to subscribe().
    let Some(queue) = (unsafe { ctx.cast::<EventQueue>().as_ref() }) else {
        return;
    };
    if !post(queue, Event::NoCamera) {
        warn("camera failure dropped: logic queue full");
    }
}
