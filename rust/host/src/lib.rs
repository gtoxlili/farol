//! Simulator backend: the same [`App`] as on the board, driven by GSP's
//! simulator through sim_bridge (`components/app_logic/host`).
//!
//! sim_bridge runs every callback and timer on one backend thread, so here the
//! callbacks call the app directly instead of going through a queue: the
//! bridge thread plays the board's `logic` task. A GSP timer polls the app every
//! [`POLL_PERIOD_MS`]; GSP setters go over the bridge to the simulator, which
//! renders and runs the motion.

use std::ffi::c_void;
use std::ptr;
use std::sync::atomic::{AtomicPtr, Ordering};
use std::time::Instant;

use gsp_sys as gsp;
use logic::ui::GspUi;
use logic::{App, Event};

/// Poll granularity for time-driven work; well under the app's one-second grid.
const POLL_PERIOD_MS: u32 = 50;

struct Backend {
    app: App,
    ui: GspUi,
    epoch: Instant,
    timer: *mut c_void,
}

impl Backend {
    fn now_ms(&self) -> u64 {
        u64::try_from(self.epoch.elapsed().as_millis()).unwrap_or(u64::MAX)
    }

    fn poll(&mut self) {
        let now = self.now_ms();
        self.app.poll(now, &mut self.ui);
        if let Some(code) = self.app.take_first_rejection() {
            eprintln!(
                "app_logic host: GSP rejected a setter ({code}); further rejections are only counted"
            );
        }
    }
}

/// The running backend, owned between start and stop; bridge thread only.
static BACKEND: AtomicPtr<Backend> = AtomicPtr::new(ptr::null_mut());

unsafe extern "C" fn on_event(
    _ui: gsp::esp_gsp_handle_t,
    event: *const gsp::esp_gsp_event_t,
    ctx: *mut c_void,
) {
    // SAFETY: ctx is the Backend from app_logic_host_start, alive until stop,
    // and only this thread touches it.
    let (Some(event), Some(backend)) = (unsafe { event.as_ref() }, unsafe {
        ctx.cast::<Backend>().as_mut()
    }) else {
        return;
    };
    if let Some(event) = Event::from_gsp(event) {
        backend.app.handle(event, &mut backend.ui);
        backend.poll();
    }
}

unsafe extern "C" fn on_timer(_ui: gsp::esp_gsp_handle_t, ctx: *mut c_void) {
    // SAFETY: as in on_event.
    if let Some(backend) = unsafe { ctx.cast::<Backend>().as_mut() } {
        backend.poll();
    }
}

/// Starts the logic on the bridge thread (from `gsp_bridge_app_init`).
/// Returns `ESP_GSP_OK` or a GSP error code; on error nothing stays registered.
///
/// # Safety
/// `handle` is the bridge's GSP handle, valid until [`app_logic_host_stop`].
#[unsafe(no_mangle)]
pub unsafe extern "C" fn app_logic_host_start(handle: gsp::esp_gsp_handle_t) -> gsp::esp_gsp_err_t {
    let invalid = gsp::ESP_GSP_ERR_INVALID_STATE as gsp::esp_gsp_err_t;
    if handle.is_null() || !BACKEND.load(Ordering::Acquire).is_null() {
        return invalid;
    }
    let epoch = Instant::now();
    let backend = Box::into_raw(Box::new(Backend {
        app: App::new(0),
        // SAFETY: the bridge keeps the handle valid until deinit.
        ui: unsafe { GspUi::new(handle) },
        epoch,
        timer: ptr::null_mut(),
    }));
    let ctx = backend.cast::<c_void>();
    // SAFETY: callbacks match the GSP signatures; ctx lives until stop.
    let err = unsafe { gsp::esp_gsp_on_event(handle, Some(on_event), ctx) };
    if err != gsp::ESP_GSP_OK as gsp::esp_gsp_err_t {
        drop(unsafe { Box::from_raw(backend) });
        return err;
    }
    let timer = unsafe { gsp::esp_gsp_timer_create(handle, POLL_PERIOD_MS, Some(on_timer), ctx) };
    if timer.is_null() {
        unsafe { gsp::esp_gsp_on_event(handle, None, ptr::null_mut()) };
        drop(unsafe { Box::from_raw(backend) });
        return invalid;
    }
    // SAFETY: no callback runs until this function returns (single bridge thread).
    unsafe {
        (*backend).timer = timer;
        (*backend).poll();
    }
    BACKEND.store(backend, Ordering::Release);
    eprintln!("app_logic host: started");
    gsp::ESP_GSP_OK as gsp::esp_gsp_err_t
}

/// Stops the logic (from `gsp_bridge_app_deinit`, after the bridge stopped
/// dispatching callbacks) and prints a one-line summary to stderr. The bridge
/// calls it on an orderly simulator exit, not when the runner kills the
/// simulator after `--duration`.
///
/// # Safety
/// `handle` is the handle passed to [`app_logic_host_start`], still valid.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn app_logic_host_stop(handle: gsp::esp_gsp_handle_t) {
    let backend = BACKEND.swap(ptr::null_mut(), Ordering::AcqRel);
    if backend.is_null() {
        return;
    }
    // SAFETY: start leaked this Box; no callback can run any more. GSP calls
    // may be refused during bridge shutdown, which leaves nothing to undo.
    let backend = unsafe { Box::from_raw(backend) };
    unsafe {
        gsp::esp_gsp_timer_delete(handle, backend.timer);
        gsp::esp_gsp_on_event(handle, None, ptr::null_mut());
    }
    eprintln!(
        "app_logic host: taps={} uptime_s={} rejected={}",
        backend.app.taps(),
        backend.now_ms() / 1000,
        backend.app.rejections()
    );
}
