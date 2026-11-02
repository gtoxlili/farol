//! Board platform for the logic: the ESP-IDF allocator and panic handler, the
//! FreeRTOS and esp_timer FFI, and the `logic` task that owns the [`App`].
//!
//! Data flow: GSP calls [`on_event`] on its render task; it decodes the event
//! and posts it to a FreeRTOS queue without blocking. The `logic` task waits on
//! that queue until the app's next deadline, then runs [`App::handle`] and
//! [`App::poll`], whose setters GSP applies asynchronously before its next
//! frame. Only the `logic` task touches the `App`.
#![no_std]

extern crate alloc;

mod heap;
mod idf;
mod panic;

use alloc::boxed::Box;
use core::ffi::c_void;
use core::sync::atomic::{AtomicBool, Ordering};

use gsp_sys as gsp;
use logic::ui::GspUi;
use logic::{App, Event};

/// Events waiting for the logic task; a full queue drops the newest tap, as a
/// busy UI would.
const QUEUE_LENGTH: u32 = 16;
/// Stack of the logic task, in internal RAM. Measure before growing the logic.
const TASK_STACK_BYTES: u32 = 4096;
/// Above GSP's render task (priority 4), so a tap's setters are queued before
/// the render task takes its next frame.
const TASK_PRIORITY: u32 = 5;

type EventQueue = idf::Queue<Event>;

static STARTED: AtomicBool = AtomicBool::new(false);

/// State of the logic task, its single owner.
struct LogicTask {
    app: App,
    ui: GspUi,
    queue: &'static EventQueue,
}

impl LogicTask {
    fn run(&mut self) -> ! {
        loop {
            let wait_ms = self.app.next_deadline_ms().saturating_sub(idf::now_ms());
            let wait_ms = u32::try_from(wait_ms).unwrap_or(u32::MAX);
            if let Some(event) = self.queue.recv(wait_ms) {
                self.app.handle(event, &mut self.ui);
            }
            self.app.poll(idf::now_ms(), &mut self.ui);
            if let Some(code) = self.app.take_first_rejection() {
                let message = alloc::format!(
                    "GSP rejected a setter (0x{code:x}); further rejections are only counted"
                );
                idf::log_line(idf::LogLevel::Warn, c"logic", &message);
            }
        }
    }
}

/// GSP event callback (render task): decode, post, return. Never blocks.
unsafe extern "C" fn on_event(
    _ui: gsp::esp_gsp_handle_t,
    event: *const gsp::esp_gsp_event_t,
    ctx: *mut c_void,
) {
    // SAFETY: GSP passes a valid event; ctx is the leaked queue from app_logic_start.
    let (Some(event), Some(queue)) = (unsafe { event.as_ref() }, unsafe {
        ctx.cast::<EventQueue>().as_ref()
    }) else {
        return;
    };
    if let Some(event) = Event::from_gsp(event) {
        let _ = queue.try_send(event);
    }
}

unsafe extern "C" fn logic_task(arg: *mut c_void) {
    // SAFETY: app_logic_start leaked this Box for the task, its only owner.
    let task = unsafe { &mut *arg.cast::<LogicTask>() };
    task.run()
}

/// Starts the logic for a running GSP UI. Returns 0, or a negative step code:
/// -1 null handle or already started, -2 queue, -3 event callback, -4 task.
/// Call once from app_main after `esp_gsp_esp_lcd_start`.
///
/// # Safety
/// `handle` is a started GSP instance that stays alive for the rest of the
/// boot (the task and callback never detach).
#[unsafe(no_mangle)]
pub unsafe extern "C" fn app_logic_start(handle: gsp::esp_gsp_handle_t) -> i32 {
    if handle.is_null() || STARTED.swap(true, Ordering::AcqRel) {
        return -1;
    }
    let Some(queue) = EventQueue::new(QUEUE_LENGTH) else {
        return -2;
    };
    // The queue outlives everything: GSP keeps its pointer as callback context.
    let queue: &'static EventQueue = Box::leak(Box::new(queue));
    // SAFETY: the caller guarantees a started GSP instance alive until reboot.
    let ui = unsafe { GspUi::new(handle) };
    let task = Box::new(LogicTask {
        app: App::new(idf::now_ms()),
        ui,
        queue,
    });
    let ctx = (queue as *const EventQueue).cast_mut().cast::<c_void>();
    // SAFETY: on_event matches esp_gsp_event_cb_t and ctx lives forever.
    let err = unsafe { gsp::esp_gsp_on_event(handle, Some(on_event), ctx) };
    if err != gsp::ESP_GSP_OK as gsp::esp_gsp_err_t {
        return -3;
    }
    let arg = Box::into_raw(task).cast::<c_void>();
    if !idf::spawn(c"logic", TASK_STACK_BYTES, TASK_PRIORITY, logic_task, arg) {
        // The callback stays registered on the leaked queue; taps are dropped
        // by a queue nobody drains.
        drop(unsafe { Box::from_raw(arg.cast::<LogicTask>()) });
        return -4;
    }
    idf::log_line(idf::LogLevel::Info, c"logic", "started");
    0
}
