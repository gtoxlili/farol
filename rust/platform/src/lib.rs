//! Board platform for the logic: the ESP-IDF allocator and panic handler, the
//! FreeRTOS and esp_timer FFI, and the `logic` task that owns the [`App`].
//!
//! Data flow: GSP calls [`on_event`], [`on_component`] and [`on_input`] on its
//! render task; they decode, box the [`Event`] and post the pointer to a
//! FreeRTOS queue without blocking. The `logic` task waits on that queue until
//! the app's next deadline, then runs [`App::handle`] and [`App::poll`], whose
//! setters GSP applies asynchronously before its next frame. Wi-Fi and link
//! reports arrive the same way from the C components' tasks ([`net`],
//! [`feedback`]). Only
//! the `logic` task touches the `App`; the render task reads one atomic,
//! [`SCREEN_ON`].
#![no_std]

extern crate alloc;

mod board;
mod camera;
mod feedback;
mod heap;
mod idf;
mod net;
mod panic;

use alloc::boxed::Box;
use core::ffi::c_void;
use core::sync::atomic::{AtomicBool, AtomicPtr, Ordering};

use gsp_sys as gsp;
use logic::App;
use logic::paseo::{Config, Out};
use logic::sys::{Chime, Display, Event, Glow, Haptic, Prefs, Sys, WifiCmd};
use logic::ui::GspUi;

/// Events waiting for the logic task; a full queue drops the newest input, as
/// a busy UI would.
const QUEUE_LENGTH: u32 = 24;
/// Stack of the logic task, in internal RAM (it writes NVS): JSON decoding,
/// text layout and formatting run here. 16 KiB left 13.8 KiB unused on the
/// board; the health line reports what stays unused.
const TASK_STACK_BYTES: u32 = 8 * 1024;
/// Above GSP's render task (priority 4), so a tap's setters are queued before
/// the render task takes its next frame.
const TASK_PRIORITY: u32 = 5;
/// How often the logic task logs its stack and the heaps.
const HEALTH_PERIOD_MS: u64 = 60_000;

/// Boxed events: they carry strings (link frames, scanned codes).
type EventQueue = idf::Queue<*mut Event>;

static STARTED: AtomicBool = AtomicBool::new(false);
/// The logic task's queue once started, for C callers (the bench harness).
static QUEUE: AtomicPtr<EventQueue> = AtomicPtr::new(core::ptr::null_mut());
/// Whether the panel shows anything. Written by the logic task, read by the
/// input interceptor: a touch on a dark panel only wakes it.
static SCREEN_ON: AtomicBool = AtomicBool::new(true);
/// The last pointer sample was pressed (render task only).
static PRESSED: AtomicBool = AtomicBool::new(false);
/// The current press began on a dark panel and is swallowed (render task only).
static SWALLOWING: AtomicBool = AtomicBool::new(false);

/// Effects on the board. The link and Wi-Fi go to app_link and app_net
/// ([`net`]); haptics, sound, LEDs and the panel to app_feedback
/// ([`feedback`]); the camera to app_camera ([`camera`]).
struct Board {
    display: Option<Display>,
}

impl Sys for Board {
    fn link(&mut self, out: Out) {
        net::link(out);
    }
    fn haptic(&mut self, haptic: Haptic) {
        feedback::haptic(haptic);
    }
    fn chime(&mut self, chime: Chime) {
        feedback::chime(chime);
    }
    fn glow(&mut self, glow: Glow) {
        feedback::glow(glow);
    }
    fn led_brightness(&mut self, percent: u8) {
        feedback::led_brightness(percent);
    }
    fn display(&mut self, display: Display) {
        feedback::display(&mut self.display, display);
    }
    fn save_config(&mut self, config: Option<&Config>) {
        board::save_config(config);
    }
    fn save_prefs(&mut self, prefs: &Prefs) {
        board::save_prefs(prefs);
    }
    fn wifi(&mut self, cmd: WifiCmd) {
        net::wifi(cmd);
    }
    fn camera(&mut self, on: bool) {
        camera::set(on);
    }
    fn random(&mut self, out: &mut [u8]) {
        board::random(out);
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
            if let Some(r) = self.app.take_first_rejection() {
                let message = alloc::format!(
                    "GSP rejected a setter (0x{:x}, {} {}); retried, further rejections are only counted",
                    r.code,
                    r.what,
                    r.target
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
fn post(queue: &EventQueue, event: Event) -> bool {
    let event = Box::into_raw(Box::new(event));
    if queue.try_send(event) {
        return true;
    }
    // SAFETY: the queue refused the pointer, so it is still ours.
    drop(unsafe { Box::from_raw(event) });
    false
}

/// Restarts the demo fleet `offset_ms` into its script, on the home page
/// (any task, never blocks). False before the logic started or when its queue
/// is full. The bench harness calls it at the start of every run.
#[unsafe(no_mangle)]
pub extern "C" fn app_logic_restart_demo(offset_ms: u32) -> bool {
    // SAFETY: QUEUE is null or the queue leaked by app_logic_start.
    let Some(queue) = (unsafe { QUEUE.load(Ordering::Acquire).as_ref() }) else {
        return false;
    };
    post(
        queue,
        Event::Demo {
            offset_ms: u64::from(offset_ms),
        },
    )
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
    QUEUE.store((queue as *const EventQueue).cast_mut(), Ordering::Release);
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
    // Reports wait in the queue until the task below runs.
    net::subscribe(queue);
    feedback::start(queue);
    camera::subscribe(queue);
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
