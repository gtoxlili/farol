//! Haptics, chimes, the module's LEDs, display power and the board's sensors:
//! the C component `app_feedback` (`components/app_feedback/include/app_feedback.h`,
//! this repo) on the board's side of [`logic::sys::Sys`].
//!
//! Requests go out through calls that copy their argument and return at once;
//! app_feedback's own task carries them out. Reports (fuel gauge, AI key,
//! module hot-plug, PIR, light, IMU) come back through one C callback on that
//! task, which converts the report and posts one boxed [`Event`] to the logic
//! task's queue without blocking. Nothing here touches the `App`.

use core::ffi::{c_int, c_void};
use core::sync::atomic::Ordering;

use logic::sys::{Button, Chime, Display, Event, Glow, Haptic, Motion, Power};

use crate::{EventQueue, SCREEN_ON, idf, post};

type EspErr = c_int;
const ESP_OK: EspErr = 0;

/// `app_feedback_event_kind_t`.
const EVENT_POWER: c_int = 0;
const EVENT_BUTTON: c_int = 1;
const EVENT_MODULE: c_int = 2;
const EVENT_PRESENCE: c_int = 3;
const EVENT_AMBIENT: c_int = 4;
const EVENT_MOTION: c_int = 5;
const EVENT_LOUDNESS: c_int = 6;
/// `APP_FEEDBACK_MINUTES_UNKNOWN`.
const MINUTES_UNKNOWN: u16 = u16::MAX;

/// `app_feedback_power_t`.
#[repr(C)]
#[derive(Clone, Copy)]
struct PowerReport {
    percent: u8,
    charging: bool,
    known: bool,
    minutes: u16,
}

/// `app_feedback_event_t`.
#[repr(C)]
struct Report {
    kind: c_int,
    value: i32,
    power: PowerReport,
}

// app_feedback.c asserts the same size on the C side.
const _: () = assert!(core::mem::size_of::<Report>() == 16);

type EventCb = unsafe extern "C" fn(*const Report, *mut c_void);
type AudioCb = unsafe extern "C" fn(*const u8, usize, *mut c_void);

unsafe extern "C" {
    fn app_feedback_start(on_event: Option<EventCb>, ctx: *mut c_void) -> EspErr;
    fn app_feedback_haptic(haptic: c_int) -> EspErr;
    fn app_feedback_chime(chime: c_int) -> EspErr;
    fn app_feedback_glow(glow: c_int) -> EspErr;
    fn app_feedback_led_brightness(percent: u8) -> EspErr;
    fn app_feedback_display(on: bool, percent: u8) -> EspErr;
    fn app_feedback_on_audio(on_audio: Option<AudioCb>, ctx: *mut c_void) -> EspErr;
    fn app_feedback_listen(on: bool) -> EspErr;
}

fn check(what: &str, err: EspErr) {
    if err != ESP_OK {
        idf::log_line(
            idf::LogLevel::Warn,
            c"feedback",
            &alloc::format!("{what} failed (0x{err:x})"),
        );
    }
}

/// Starts app_feedback with the logic's queue as the consumer of its reports.
/// Without it the board runs silent and without sensors; the logic keeps its
/// defaults (battery hidden, no module).
pub fn start(queue: &'static EventQueue) {
    let ctx = (queue as *const EventQueue).cast_mut().cast::<c_void>();
    // SAFETY: the callback matches the C signature and the queue is leaked
    // for the rest of the boot.
    check("app_feedback start", unsafe {
        app_feedback_start(Some(on_report), ctx)
    });
    // SAFETY: as above; recordings arrive on the chime task.
    check("app_feedback audio", unsafe {
        app_feedback_on_audio(Some(on_audio), ctx)
    });
}

/// The `chime` task: recorded speech (or its end) to the logic.
unsafe extern "C" fn on_audio(pcm: *const u8, length: usize, ctx: *mut c_void) {
    // SAFETY: ctx is the leaked queue from start().
    let Some(queue) = (unsafe { ctx.cast::<EventQueue>().as_ref() }) else {
        return;
    };
    let event = if pcm.is_null() {
        Event::AudioEnd
    } else {
        // SAFETY: app_feedback passes `length` valid bytes for the call.
        Event::Audio(unsafe { core::slice::from_raw_parts(pcm, length) }.to_vec())
    };
    if !post(queue, event) {
        idf::log_line(
            idf::LogLevel::Warn,
            c"feedback",
            "audio dropped: logic queue full",
        );
    }
}

/// Start or stop recording; the end always arrives as [`Event::AudioEnd`].
pub fn listen(on: bool) {
    check("app_feedback listen", unsafe { app_feedback_listen(on) });
}

/// The `feedback` task: one report to one event.
unsafe extern "C" fn on_report(report: *const Report, ctx: *mut c_void) {
    // SAFETY: app_feedback passes a valid report; ctx is the leaked queue.
    let (Some(report), Some(queue)) = (unsafe { report.as_ref() }, unsafe {
        ctx.cast::<EventQueue>().as_ref()
    }) else {
        return;
    };
    let Some(event) = decode(report) else {
        return;
    };
    if !post(queue, event) {
        idf::log_line(
            idf::LogLevel::Warn,
            c"feedback",
            "report dropped: logic queue full",
        );
    }
}

fn decode(report: &Report) -> Option<Event> {
    let value = report.value;
    Some(match report.kind {
        EVENT_POWER => {
            let p = report.power;
            Event::Power(Power {
                percent: p.percent.min(100),
                charging: p.charging,
                minutes: (p.minutes != MINUTES_UNKNOWN).then_some(p.minutes),
                known: p.known,
            })
        }
        EVENT_BUTTON => Event::Button(match value {
            0 => Button::AiClick,
            1 => Button::AiLong,
            2 => Button::Prev,
            3 => Button::Next,
            4 => Button::AiRelease,
            _ => return None,
        }),
        EVENT_MODULE => Event::Module(value != 0),
        EVENT_PRESENCE => Event::Presence(value != 0),
        EVENT_AMBIENT => Event::Ambient(value.clamp(0, 100) as u8),
        EVENT_LOUDNESS => Event::Loudness(value.clamp(0, 100) as u8),
        EVENT_MOTION => Event::Motion(match value {
            0 => Motion::Lifted,
            1 => Motion::FaceDown,
            2 => Motion::FaceUp,
            _ => return None,
        }),
        _ => return None,
    })
}

// ---------------------------------------------------------------- requests

pub fn haptic(haptic: Haptic) {
    let code = match haptic {
        Haptic::Tick => 0,
        Haptic::Confirm => 1,
        Haptic::Call => 2,
        Haptic::Fail => 3,
    };
    check("haptic", unsafe { app_feedback_haptic(code) });
}

pub fn chime(chime: Chime) {
    let code = match chime {
        Chime::Call => 0,
        Chime::Done => 1,
        Chime::Fail => 2,
    };
    check("chime", unsafe { app_feedback_chime(code) });
}

pub fn glow(glow: Glow) {
    let code = match glow {
        Glow::Off => 0,
        Glow::Work => 1,
        Glow::Call => 2,
        Glow::Fail => 3,
        Glow::Flash => 4,
    };
    check("glow", unsafe { app_feedback_glow(code) });
}

pub fn led_brightness(percent: u8) {
    check("LED brightness", unsafe {
        app_feedback_led_brightness(percent.min(100))
    });
}

/// Panel power and brightness, carried out on app_feedback's task. `shown` is
/// the last request; repeats are skipped. [`SCREEN_ON`] follows the logic's
/// decision at once: dark before the panel goes off, lit once it is asked on,
/// so a touch that lands while the panel is (going) dark only wakes it.
pub fn display(shown: &mut Option<Display>, display: Display) {
    if *shown == Some(display) {
        return;
    }
    match display {
        Display::On(level) => {
            check("display on", unsafe {
                app_feedback_display(true, level.min(100))
            });
            SCREEN_ON.store(true, Ordering::Release);
        }
        Display::Off => {
            SCREEN_ON.store(false, Ordering::Release);
            check("display off", unsafe { app_feedback_display(false, 0) });
        }
    }
    *shown = Some(display);
}
