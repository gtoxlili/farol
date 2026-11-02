//! Board platform for the logic: the ESP-IDF allocator and panic handler, the
//! FreeRTOS and esp_timer FFI, and the `logic` task that owns the [`App`].
//!
//! Data flow: GSP calls [`on_event`], [`on_component`] and [`on_input`] on its
//! render task; they decode, box the [`Event`] and post the pointer to a
//! FreeRTOS queue without blocking. The `logic` task waits on that queue until
//! the app's next deadline, then runs [`App::handle`] and [`App::poll`], whose
//! setters GSP applies asynchronously before its next frame. Only the `logic`
//! task touches the `App`; the render task reads one atomic, [`SCREEN_ON`].
#![no_std]

extern crate alloc;

mod board;
mod heap;
mod idf;
mod panic;

use alloc::boxed::Box;
use core::ffi::c_void;
use core::sync::atomic::{AtomicBool, Ordering};

use gsp_sys as gsp;
use logic::App;
use logic::paseo::{Config, Out};
use logic::sys::{Chime, Display, Event, Glow, Haptic, Prefs, Sys, WifiCmd};
use logic::ui::GspUi;

/// Events waiting for the logic task; a full queue drops the newest input, as
/// a busy UI would.
const QUEUE_LENGTH: u32 = 24;
/// Stack of the logic task, in internal RAM: JSON decoding, text layout and
/// formatting run here. The health line reports what stays unused.
const TASK_STACK_BYTES: u32 = 16 * 1024;
/// Above GSP's render task (priority 4), so a tap's setters are queued before
/// the render task takes its next frame.
const TASK_PRIORITY: u32 = 5;
/// How often the logic task logs its stack and the heaps.
const HEALTH_PERIOD_MS: u64 = 60_000;

/// Boxed events: they carry strings (link frames, scanned codes).
type EventQueue = idf::Queue<*mut Event>;

static STARTED: AtomicBool = AtomicBool::new(false);
/// Whether the panel shows anything. Written by the logic task, read by the
/// input interceptor: a touch on a dark panel only wakes it.
static SCREEN_ON: AtomicBool = AtomicBool::new(true);
/// The last pointer sample was pressed (render task only).
static PRESSED: AtomicBool = AtomicBool::new(false);
/// The current press began on a dark panel and is swallowed (render task only).
static SWALLOWING: AtomicBool = AtomicBool::new(false);

/// Effects on the board. The link, Wi-Fi, camera, haptics, sound and LEDs
/// arrive with their components (app_link, app_net, app_camera,
/// app_feedback); until then those effects are logged.
struct Board {
    display: Option<Display>,
}

impl Board {
    fn later(what: &str) {
        idf::log_line(idf::LogLevel::Info, c"logic", what);
    }
}

impl Sys for Board {
    fn link(&mut self, out: Out) {
        if !matches!(out, Out::Send(_)) {
            Self::later(&alloc::format!("link {out:?} (no app_link yet)"));
        }
    }
    fn haptic(&mut self, _haptic: Haptic) {}
    fn chime(&mut self, _chime: Chime) {}
    fn glow(&mut self, _glow: Glow) {}
    fn display(&mut self, display: Display) {
        if self.display == Some(display) {
            return;
        }
        match display {
            Display::On(level) => {
                board::set_brightness(level);
                if self.display.is_none_or(|d| d == Display::Off) {
                    board::set_display_enabled(true);
                }
                SCREEN_ON.store(true, Ordering::Release);
            }
            Display::Off => {
                SCREEN_ON.store(false, Ordering::Release);
                board::set_display_enabled(false);
            }
        }
        self.display = Some(display);
    }
    fn save_config(&mut self, config: Option<&Config>) {
        board::save_config(config);
    }
    fn save_prefs(&mut self, prefs: &Prefs) {
        board::save_prefs(prefs);
    }
    fn wifi(&mut self, cmd: WifiCmd) {
        Self::later(&alloc::format!("wifi {cmd:?} (no app_net yet)"));
    }
    fn camera(&mut self, on: bool) {
        Self::later(&alloc::format!("camera {on} (no app_camera yet)"));
    }
}

/// State of the logic task, its single owner.
struct LogicTask {
    app: App,
    ui: GspUi,
    board: Board,
    queue: &'static EventQueue,
    next_health: u64,
}

impl LogicTask {
    fn run(&mut self) -> ! {
        let (config, prefs) = board::load();
        let now = idf::now_ms();
        self.app.handle(
            now,
            Event::Restored { config, prefs },
            &mut self.ui,
            &mut self.board,
        );
        if board::demo_build() {
            self.app.start_demo(0);
        }
        loop {
            let deadline = self.app.next_deadline_ms().min(self.next_health);
            let wait_ms = u32::try_from(deadline.saturating_sub(idf::now_ms())).unwrap_or(u32::MAX);
            if let Some(event) = self.queue.recv(wait_ms) {
                // SAFETY: every queued pointer is a Box leaked by post().
                let event = unsafe { Box::from_raw(event) };
                self.app
                    .handle(idf::now_ms(), *event, &mut self.ui, &mut self.board);
            }
            let now = idf::now_ms();
            if now >= self.app.next_deadline_ms() {
                self.app.poll(now, &mut self.ui, &mut self.board);
            }
            if let Some(code) = self.app.take_first_rejection() {
                let message = alloc::format!(
                    "GSP rejected a setter (0x{code:x}); further rejections are only counted"
                );
                idf::log_line(idf::LogLevel::Warn, c"logic", &message);
            }
            if now >= self.next_health {
                self.next_health = now + HEALTH_PERIOD_MS;
                let line = alloc::format!(
                    "{} rejected={}",
                    board::health_line(),
                    self.app.rejections()
                );
                idf::log_line(idf::LogLevel::Info, c"logic", &line);
            }
        }
    }
}

/// Posts an event to the logic task; never blocks. Dropped when the queue is full.
fn post(queue: &EventQueue, event: Event) {
    let event = Box::into_raw(Box::new(event));
    if !queue.try_send(event) {
        // SAFETY: the queue refused the pointer, so it is still ours.
        drop(unsafe { Box::from_raw(event) });
    }
}

/// GSP event callback (render task): a `callback` action fired.
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
    if event.type_ == gsp::ESP_GSP_EVENT_CALL {
        post(
            queue,
            Event::Call {
                action: u32::from(event.action_id),
                arg: event.arg as i32,
            },
        );
    }
}

/// GSP component callback (render task): a page flow or drawer settled.
unsafe extern "C" fn on_component(
    _ui: gsp::esp_gsp_handle_t,
    event: *const gsp::esp_gsp_component_event_t,
    ctx: *mut c_void,
) {
    // SAFETY: as in on_event.
    let (Some(event), Some(queue)) = (unsafe { event.as_ref() }, unsafe {
        ctx.cast::<EventQueue>().as_ref()
    }) else {
        return;
    };
    let settled =
        gsp::ESP_GSP_COMPONENT_EVENT_MOTION_FINISHED | gsp::ESP_GSP_COMPONENT_EVENT_VALUE_CHANGED;
    if u32::from(event.flags) & settled != 0 && !event.state.dragging && !event.state.settling {
        post(
            queue,
            Event::Settled {
                key: event.key,
                value: event.state.value,
            },
        );
    }
}

/// GSP input interceptor (render task): reports each new press, and swallows
/// a whole press that starts on a dark panel so it only wakes the screen.
unsafe extern "C" fn on_input(
    _ui: gsp::esp_gsp_handle_t,
    _x: i32,
    _y: i32,
    pressed: bool,
    ctx: *mut c_void,
) -> bool {
    // SAFETY: as in on_event.
    let Some(queue) = (unsafe { ctx.cast::<EventQueue>().as_ref() }) else {
        return false;
    };
    let was_pressed = PRESSED.swap(pressed, Ordering::Relaxed);
    if pressed && !was_pressed {
        SWALLOWING.store(!SCREEN_ON.load(Ordering::Acquire), Ordering::Relaxed);
        post(queue, Event::Touch);
    }
    let swallow = SWALLOWING.load(Ordering::Relaxed);
    if !pressed {
        SWALLOWING.store(false, Ordering::Relaxed);
    }
    swallow
}

unsafe extern "C" fn logic_task(arg: *mut c_void) {
    // SAFETY: app_logic_start leaked this Box for the task, its only owner.
    let task = unsafe { &mut *arg.cast::<LogicTask>() };
    task.run()
}

/// Starts the logic for a running GSP UI. Returns 0, or a negative step code:
/// -1 null handle or already started, -2 queue, -3 callbacks, -4 task.
/// Call once from app_main after `esp_gsp_esp_lcd_start`.
///
/// # Safety
/// `handle` is a started GSP instance that stays alive for the rest of the
/// boot (the task and callbacks never detach).
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
    let device = board::device_id();
    // SAFETY: the caller guarantees a started GSP instance alive until reboot.
    let ui = unsafe { GspUi::new(handle) };
    let task = Box::new(LogicTask {
        app: App::new(idf::now_ms(), &device),
        ui,
        board: Board { display: None },
        queue,
        next_health: idf::now_ms() + HEALTH_PERIOD_MS,
    });
    let ctx = (queue as *const EventQueue).cast_mut().cast::<c_void>();
    let ok = gsp::ESP_GSP_OK as gsp::esp_gsp_err_t;
    // SAFETY: the callbacks match the GSP signatures and ctx lives forever.
    let registered = unsafe {
        gsp::esp_gsp_on_event(handle, Some(on_event), ctx) == ok
            && gsp::esp_gsp_on_component_event(handle, Some(on_component), ctx) == ok
            && gsp::esp_gsp_set_input_interceptor(handle, Some(on_input), ctx) == ok
    };
    if !registered {
        return -3;
    }
    let arg = Box::into_raw(task).cast::<c_void>();
    if !idf::spawn(c"logic", TASK_STACK_BYTES, TASK_PRIORITY, logic_task, arg) {
        // The callbacks stay registered on the leaked queue, which nobody
        // drains: it keeps up to QUEUE_LENGTH events and drops the rest.
        drop(unsafe { Box::from_raw(arg.cast::<LogicTask>()) });
        return -4;
    }
    idf::log_line(
        idf::LogLevel::Info,
        c"logic",
        &alloc::format!("started as farol-{device}"),
    );
    0
}
