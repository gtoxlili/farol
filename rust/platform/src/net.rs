//! Wi-Fi and the Paseo link: the C components `app_net` and `app_link`
//! (`components/*/include/*.h`, this repo) on the board's side of
//! [`logic::sys::Sys`].
//!
//! Commands go out through calls that copy their arguments and return at once.
//! Reports come back through C callbacks on the components' own tasks
//! (`app_net`, `app_link_ws`, `app_link`); each converts its data there and
//! posts one boxed [`Event`] to the logic task's queue, which never blocks.
//! Nothing here touches the `App`.

use alloc::ffi::CString;
use alloc::string::String;
use alloc::vec::Vec;
use core::ffi::{c_char, c_int, c_void};

use logic::paseo::{Down, Out};
use logic::sys::{Ap, Event, Link, Net, NetState, WifiCmd};

use crate::{EventQueue, idf, post};

type EspErr = c_int;
const ESP_OK: EspErr = 0;

/// `APP_NET_SSID_MAX + 1` and `APP_NET_STATUS_SAVED_MAX` (`app_net.h`).
const SSID_BYTES: usize = 33;
const STATUS_SAVED_MAX: usize = 9;

/// `app_net_status_t`.
#[repr(C)]
struct NetStatus {
    state: c_int,
    ssid: [u8; SSID_BYTES],
    rssi: i8,
    ip: [u8; 16],
    saved_count: u8,
    saved: [[u8; SSID_BYTES]; STATUS_SAVED_MAX],
}

/// `app_net_ap_t`.
#[repr(C)]
struct ApRecord {
    ssid: [u8; SSID_BYTES],
    rssi: i8,
    secure: bool,
    saved: bool,
}

// app_net.c asserts the same sizes on the C side.
const _: () = assert!(core::mem::size_of::<NetStatus>() == 352);
const _: () = assert!(core::mem::size_of::<ApRecord>() == 36);

/// `app_link_down_t`.
const DOWN_DNS: c_int = 0;
const DOWN_TIMEOUT: c_int = 1;
const DOWN_TLS: c_int = 2;
const DOWN_REFUSED: c_int = 3;

type StatusCb = unsafe extern "C" fn(*const NetStatus, *mut c_void);
type ScanCb = unsafe extern "C" fn(*const ApRecord, usize, *mut c_void);
type OpenCb = unsafe extern "C" fn(*mut c_void);
type TextCb = unsafe extern "C" fn(*const c_char, usize, *mut c_void);
type CloseCb = unsafe extern "C" fn(c_int, u16, *mut c_void);

unsafe extern "C" {
    fn app_net_subscribe(
        on_status: Option<StatusCb>,
        on_scan: Option<ScanCb>,
        ctx: *mut c_void,
    ) -> EspErr;
    fn app_net_join(ssid: *const c_char, password: *const c_char) -> EspErr;
    fn app_net_scan() -> EspErr;
    fn app_net_retry() -> EspErr;
    fn app_net_forget(ssid: *const c_char) -> EspErr;

    fn app_link_subscribe(
        on_open: Option<OpenCb>,
        on_text: Option<TextCb>,
        on_close: Option<CloseCb>,
        ctx: *mut c_void,
    ) -> EspErr;
    fn app_link_open(url: *const c_char) -> EspErr;
    fn app_link_send(text: *const c_char, length: usize) -> EspErr;
    fn app_link_close() -> EspErr;
}

fn warn(message: &str) {
    idf::log_line(idf::LogLevel::Warn, c"net", message);
}

fn check(what: &str, err: EspErr) {
    if err != ESP_OK {
        warn(&alloc::format!("{what} failed (0x{err:x})"));
    }
}

/// Posts a report; a full queue loses it, which the log records.
fn deliver(queue: &EventQueue, event: Event, what: &str) {
    if !post(queue, event) {
        warn(&alloc::format!("{what} dropped: logic queue full"));
    }
}

/// A NUL-padded C string field as text.
fn text(bytes: &[u8]) -> String {
    let end = bytes.iter().position(|&b| b == 0).unwrap_or(bytes.len());
    String::from_utf8_lossy(&bytes[..end]).into_owned()
}

fn c_string(what: &str, value: &str) -> Option<CString> {
    let value = CString::new(value).ok();
    if value.is_none() {
        warn(&alloc::format!(
            "{what}: text contains a NUL byte; not sent"
        ));
    }
    value
}

/// Registers the logic's queue as the consumer of both components. Wi-Fi that
/// failed to start leaves the logic's state at [`NetState::Off`].
pub fn subscribe(queue: &'static EventQueue) {
    let ctx = (queue as *const EventQueue).cast_mut().cast::<c_void>();
    // SAFETY: the callbacks match the C signatures and the queue is leaked
    // for the rest of the boot.
    unsafe {
        check(
            "Wi-Fi subscribe",
            app_net_subscribe(Some(on_status), Some(on_scan), ctx),
        );
        check(
            "link subscribe",
            app_link_subscribe(Some(on_open), Some(on_text), Some(on_close), ctx),
        );
    }
}

// ---------------------------------------------------------------- commands

pub fn link(out: Out) {
    match out {
        Out::Open(url) => {
            if let Some(url) = c_string("link open", &url) {
                // SAFETY: a valid C string; app_link copies it.
                check("link open", unsafe { app_link_open(url.as_ptr()) });
            }
        }
        // SAFETY: pointer and length of a live str; app_link copies it.
        Out::Send(frame) => check("link send", unsafe {
            app_link_send(frame.as_ptr().cast(), frame.len())
        }),
        Out::Close => check("link close", unsafe { app_link_close() }),
    }
}

pub fn wifi(cmd: WifiCmd) {
    match cmd {
        WifiCmd::Join { ssid, password } => {
            if let (Some(ssid), Some(password)) = (
                c_string("Wi-Fi join", &ssid),
                c_string("Wi-Fi join", &password),
            ) {
                // SAFETY: valid C strings; app_net copies them.
                check("Wi-Fi join", unsafe {
                    app_net_join(ssid.as_ptr(), password.as_ptr())
                });
            }
        }
        WifiCmd::Scan => check("Wi-Fi scan", unsafe { app_net_scan() }),
        WifiCmd::Retry => check("Wi-Fi retry", unsafe { app_net_retry() }),
        WifiCmd::Forget { ssid } => {
            if let Some(ssid) = c_string("Wi-Fi forget", &ssid) {
                // SAFETY: a valid C string; app_net copies it.
                check("Wi-Fi forget", unsafe { app_net_forget(ssid.as_ptr()) });
            }
        }
    }
}

// ---------------------------------------------------------------- reports

/// # Safety
/// `ctx` is the queue passed to [`subscribe`]; the pointers are valid for the call.
unsafe fn queue<'a>(ctx: *mut c_void) -> Option<&'a EventQueue> {
    unsafe { ctx.cast::<EventQueue>().as_ref() }
}

/// app_net task: the station's status changed.
unsafe extern "C" fn on_status(status: *const NetStatus, ctx: *mut c_void) {
    // SAFETY: app_net passes a valid status and the subscribed ctx.
    let (Some(status), Some(queue)) = (unsafe { status.as_ref() }, unsafe { queue(ctx) }) else {
        return;
    };
    let state = match status.state {
        1 => NetState::NoSaved,
        2 => NetState::NotFound,
        3 => NetState::Connecting,
        4 => NetState::WrongPassword,
        5 => NetState::NoAddress,
        6 => NetState::Online,
        _ => NetState::Off,
    };
    let count = usize::from(status.saved_count).min(STATUS_SAVED_MAX);
    let net = Net {
        state,
        ssid: text(&status.ssid),
        rssi: status.rssi,
        saved: status.saved[..count]
            .iter()
            .map(|name| text(name))
            .collect(),
        ip: text(&status.ip),
    };
    deliver(queue, Event::Net(net), "Wi-Fi status");
}

/// app_net task: a scan finished.
unsafe extern "C" fn on_scan(aps: *const ApRecord, count: usize, ctx: *mut c_void) {
    // SAFETY: the subscribed ctx.
    let Some(queue) = (unsafe { queue(ctx) }) else {
        return;
    };
    let records: &[ApRecord] = if aps.is_null() || count == 0 {
        &[]
    } else {
        // SAFETY: app_net passes `count` records, valid for the call.
        unsafe { core::slice::from_raw_parts(aps, count) }
    };
    let networks: Vec<Ap> = records
        .iter()
        .map(|ap| Ap {
            ssid: text(&ap.ssid),
            rssi: ap.rssi,
            secure: ap.secure,
            saved: ap.saved,
        })
        .collect();
    deliver(queue, Event::Networks(networks), "scan result");
}

/// app_link_ws task: the WebSocket upgrade succeeded.
unsafe extern "C" fn on_open(ctx: *mut c_void) {
    // SAFETY: the subscribed ctx.
    if let Some(queue) = unsafe { queue(ctx) } {
        deliver(queue, Event::Link(Link::Opened), "link open");
    }
}

/// app_link_ws task: one complete text message.
unsafe extern "C" fn on_text(text: *const c_char, length: usize, ctx: *mut c_void) {
    // SAFETY: the subscribed ctx.
    let Some(queue) = (unsafe { queue(ctx) }) else {
        return;
    };
    let bytes: &[u8] = if text.is_null() || length == 0 {
        &[]
    } else {
        // SAFETY: app_link passes `length` bytes, valid for the call.
        unsafe { core::slice::from_raw_parts(text.cast(), length) }
    };
    let message = String::from_utf8_lossy(bytes).into_owned();
    deliver(queue, Event::Link(Link::Text(message)), "link message");
}

/// app_link_ws or app_link task: the connection ended or never opened.
unsafe extern "C" fn on_close(why: c_int, code: u16, ctx: *mut c_void) {
    // SAFETY: the subscribed ctx.
    let Some(queue) = (unsafe { queue(ctx) }) else {
        return;
    };
    let down = match why {
        DOWN_DNS => Down::Dns,
        DOWN_TIMEOUT => Down::Timeout,
        DOWN_TLS => Down::Tls,
        DOWN_REFUSED => Down::Refused(code),
        _ => Down::Closed(code),
    };
    deliver(queue, Event::Link(Link::Closed(down)), "link close");
}
