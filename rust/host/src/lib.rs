//! Simulator backend: the same [`App`] as on the board, driven by GSP's
//! simulator through sim_bridge (`components/app_logic/host`).
//!
//! sim_bridge runs every callback and timer on one backend thread, so the
//! callbacks call the app directly instead of going through a queue: the
//! bridge thread plays the board's `logic` task. A GSP timer polls the app every
//! [`POLL_PERIOD_MS`]. The simulator has no Paseo link, Wi-Fi or peripherals:
//! by default it runs the built-in demo fleet (`FAROL_SIM=welcome` shows the
//! first-boot page instead), and effects are printed to stderr.

use std::ffi::c_void;
use std::ptr;
use std::sync::atomic::{AtomicPtr, Ordering};
use std::time::Instant;

use gsp_sys as gsp;
use logic::App;
use logic::paseo::{Config, Out};
use logic::sys::{
    Ap, Chime, Display, Event, Glow, Haptic, Net, NetState, Power, Prefs, Sys, WifiCmd,
};
use logic::ui::GspUi;

/// Poll granularity for time-driven work.
const POLL_PERIOD_MS: u32 = 50;

/// Effects become stderr lines; the scenario logs show what the board would do.
/// Wi-Fi is emulated: a scan finds a few sample networks, and a join succeeds
/// unless the password is "wrong". Those answers wait in `pending`.
#[derive(Default)]
struct SimSys {
    pending: Vec<Event>,
}

fn sample_networks() -> Vec<Event> {
    let ap = |ssid: &str, rssi, secure, saved| Ap {
        ssid: ssid.into(),
        rssi,
        secure,
        saved,
    };
    vec![Event::Networks(vec![
        ap("sim", -48, true, true),
        ap("example-home-5f", -61, true, false),
        ap("example-guest", -70, false, false),
        ap("example-lab", -82, true, false),
    ])]
}

impl Sys for SimSys {
    fn link(&mut self, out: Out) {
        eprintln!("farol sim: link {out:?}");
    }
    fn haptic(&mut self, haptic: Haptic) {
        eprintln!("farol sim: haptic {haptic:?}");
    }
    fn chime(&mut self, chime: Chime) {
        eprintln!("farol sim: chime {chime:?}");
    }
    fn glow(&mut self, glow: Glow) {
        eprintln!("farol sim: glow {glow:?}");
    }
    fn led_brightness(&mut self, percent: u8) {
        eprintln!("farol sim: led brightness {percent}");
    }
    fn display(&mut self, display: Display) {
        eprintln!("farol sim: display {display:?}");
    }
    fn save_config(&mut self, config: Option<&Config>) {
        eprintln!("farol sim: save config {:?}", config.map(Config::describe));
    }
    fn random(&mut self, out: &mut [u8]) {
        // The simulator's link only logs, but its keys still come from the OS.
        use std::io::Read;
        if std::fs::File::open("/dev/urandom")
            .and_then(|mut f| f.read_exact(out))
            .is_err()
        {
            out.fill(0);
        }
    }
    fn listen(&mut self, on: bool) {
        // The demo answers voice itself; nothing records here.
        eprintln!("farol sim: listen {on}");
    }
    fn save_prefs(&mut self, prefs: &Prefs) {
        eprintln!("farol sim: save prefs {prefs:?}");
    }
    fn wifi(&mut self, cmd: WifiCmd) {
        eprintln!("farol sim: wifi {cmd:?}");
        match cmd {
            WifiCmd::Scan => self.pending.extend(sample_networks()),
            WifiCmd::Join { ssid, password } => {
                let state = if password == "wrong" {
                    NetState::WrongPassword
                } else {
                    NetState::Online
                };
                self.pending.push(Event::Net(Net {
                    state,
                    ssid,
                    rssi: -52,
                    saved: vec!["sim".into()],
                    ip: "192.0.2.10".into(),
                }));
            }
            WifiCmd::Retry | WifiCmd::Forget { .. } => {}
        }
    }
    fn camera(&mut self, on: bool) {
        eprintln!("farol sim: camera {on}");
    }
}

struct Backend {
    app: App,
    ui: GspUi,
    sys: SimSys,
    epoch: Instant,
    timer: *mut c_void,
}

impl Backend {
    fn now_ms(&self) -> u64 {
        u64::try_from(self.epoch.elapsed().as_millis()).unwrap_or(u64::MAX)
    }

    fn handle(&mut self, event: Event) {
        let now = self.now_ms();
        self.app.handle(now, event, &mut self.ui, &mut self.sys);
        // Emulated Wi-Fi answers arrive as their own events, as on the board.
        while !self.sys.pending.is_empty() {
            let event = self.sys.pending.remove(0);
            self.app.handle(now, event, &mut self.ui, &mut self.sys);
        }
        self.poll();
    }

    fn poll(&mut self) {
        let now = self.now_ms();
        if now >= self.app.next_deadline_ms() {
            self.app.poll(now, &mut self.ui, &mut self.sys);
        }
        if let Some(r) = self.app.take_first_rejection() {
            eprintln!(
                "farol sim: GSP rejected a setter (0x{:x}, {} {}); further rejections are only counted",
                r.code, r.what, r.target
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
    if event.type_ == gsp::ESP_GSP_EVENT_CALL {
        backend.handle(Event::Call {
            action: u32::from(event.action_id),
            arg: event.arg as i32,
        });
    }
}

unsafe extern "C" fn on_component(
    _ui: gsp::esp_gsp_handle_t,
    event: *const gsp::esp_gsp_component_event_t,
    ctx: *mut c_void,
) {
    // SAFETY: as in on_event.
    let (Some(event), Some(backend)) = (unsafe { event.as_ref() }, unsafe {
        ctx.cast::<Backend>().as_mut()
    }) else {
        return;
    };
    let settled =
        gsp::ESP_GSP_COMPONENT_EVENT_MOTION_FINISHED | gsp::ESP_GSP_COMPONENT_EVENT_VALUE_CHANGED;
    if u32::from(event.flags) & settled != 0 && !event.state.dragging && !event.state.settling {
        backend.handle(Event::Settled {
            key: event.key,
            value: event.state.value,
        });
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
    let mut app = App::new(0, "5im0");
    // FAROL_SIM_LANG=en shows the board in English (the demo data too).
    if std::env::var("FAROL_SIM_LANG").is_ok_and(|l| l == "en") {
        app.set_lang(logic::lang::Lang::En);
    }
    let mode = std::env::var("FAROL_SIM").unwrap_or_default();
    if mode != "welcome" {
        // FAROL_SIM_T=<ms> starts the demo script that far in (checks jump to a moment).
        let offset = std::env::var("FAROL_SIM_T")
            .ok()
            .and_then(|t| t.parse().ok())
            .unwrap_or(0);
        app.start_demo(offset);
    }
    let backend = Box::into_raw(Box::new(Backend {
        app,
        // SAFETY: the bridge keeps the handle valid until deinit.
        ui: unsafe { GspUi::new(handle) },
        sys: SimSys::default(),
        epoch: Instant::now(),
        timer: ptr::null_mut(),
    }));
    let ctx = backend.cast::<c_void>();
    // SAFETY: callbacks match the GSP signatures; ctx lives until stop.
    let err = unsafe { gsp::esp_gsp_on_event(handle, Some(on_event), ctx) };
    if err != gsp::ESP_GSP_OK as gsp::esp_gsp_err_t {
        drop(unsafe { Box::from_raw(backend) });
        return err;
    }
    let err = unsafe { gsp::esp_gsp_on_component_event(handle, Some(on_component), ctx) };
    if err != gsp::ESP_GSP_OK as gsp::esp_gsp_err_t {
        unsafe { gsp::esp_gsp_on_event(handle, None, ptr::null_mut()) };
        drop(unsafe { Box::from_raw(backend) });
        return err;
    }
    let timer = unsafe { gsp::esp_gsp_timer_create(handle, POLL_PERIOD_MS, Some(on_timer), ctx) };
    if timer.is_null() {
        unsafe {
            gsp::esp_gsp_on_component_event(handle, None, ptr::null_mut());
            gsp::esp_gsp_on_event(handle, None, ptr::null_mut());
        }
        drop(unsafe { Box::from_raw(backend) });
        return invalid;
    }
    // SAFETY: no callback runs until this function returns (single bridge thread).
    unsafe {
        (*backend).timer = timer;
        let b = &mut *backend;
        // The simulator stands in for a board that is on Wi-Fi and plugged in.
        b.handle(Event::Net(Net {
            state: NetState::Online,
            ssid: "sim".into(),
            rssi: -52,
            saved: vec!["sim".into()],
            ip: "192.0.2.10".into(),
        }));
        b.handle(Event::Power(Power {
            percent: 87,
            charging: false,
            minutes: Some(372),
            known: true,
        }));
    }
    BACKEND.store(backend, Ordering::Release);
    eprintln!(
        "farol sim: started ({})",
        if mode == "welcome" { "welcome" } else { "demo" }
    );
    gsp::ESP_GSP_OK as gsp::esp_gsp_err_t
}

/// Stops the logic (from `gsp_bridge_app_deinit`, after the bridge stopped
/// dispatching callbacks) and prints a one-line summary to stderr.
///
/// # Safety
/// `handle` is the handle passed to [`app_logic_host_start`], still valid.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn app_logic_host_stop(handle: gsp::esp_gsp_handle_t) {
    let backend = BACKEND.swap(ptr::null_mut(), Ordering::AcqRel);
    if backend.is_null() {
        return;
    }
    // SAFETY: start leaked this Box; no callback can run any more.
    let backend = unsafe { Box::from_raw(backend) };
    unsafe {
        gsp::esp_gsp_timer_delete(handle, backend.timer);
        gsp::esp_gsp_on_component_event(handle, None, ptr::null_mut());
        gsp::esp_gsp_on_event(handle, None, ptr::null_mut());
    }
    eprintln!(
        "farol sim: uptime_s={} rejected={}",
        backend.now_ms() / 1000,
        backend.app.rejections()
    );
}
