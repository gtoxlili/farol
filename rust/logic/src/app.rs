//! The application's state machine: one owner (the board's `logic` task or
//! the simulator's bridge thread) feeds it [`Event`]s and calls [`App::poll`]
//! at [`App::next_deadline_ms`]; it answers with scene setters (through
//! [`crate::ui::Ui`]) and platform effects (through [`Sys`]).

use alloc::collections::{BTreeMap, BTreeSet};
use alloc::string::{String, ToString};
use alloc::vec::Vec;
use core::cell::Cell;

use gsp_sys as gsp;

use crate::demo::Demo;
use crate::model::{Act, ActKind, ActResult, Agent, BoardPrefs, Settings, Snapshot, State};
use crate::paseo::{Config, Down, Notice, Out, Session};
use crate::qr::{self, Code};
use crate::sys::{
    Ap, Button, Chime, Display, Event, Glow, Haptic, Link, Motion, Net, NetState, Power, Prefs,
    Sys, WifiCmd,
};
use crate::ui::{Painter, Ui};

/// Pages of the `screens` PageFlow; page 0 is usage.
pub const HOME: u16 = 1;
pub const DECK: u16 = 2;
/// Recycled pages in the `cards` PageFlow.
pub const SLOTS: usize = 3;

/// A press on a running card shows the stop ring after this long...
pub const HOLD_SHOW_MS: u64 = 250;
/// ...and stops the turn when held this much longer.
pub const HOLD_STOP_MS: u64 = 1200;
/// Wi-Fi lost for this long brings up the "no Wi-Fi" page.
const NO_WIFI_PAGE_MS: u64 = 10_000;
/// Paseo unreachable for this long shows the banner.
const BANNER_MS: u64 = 30_000;
/// An alert keeps the screen on this long even when nobody is around.
const ALERT_AWAKE_MS: u64 = 30_000;
/// Without the interaction module: dim after, then off after, no touch.
const DIM_AFTER_MS: u64 = 120_000;
const OFF_AFTER_MS: u64 = 600_000;
/// With the module: off this long after the last presence or touch.
const OFF_AFTER_ABSENT_MS: u64 = 180_000;
const DIM_PERCENT: u8 = 25;
/// Long-press on the AI key silences the board for this long.
const DND_MS: u64 = 3_600_000;
/// A read card stays in the deck, greyed, this long after it was read.
const SEEN_KEEP_MS: i64 = 3_600_000;
/// The deck order is re-checked this often (read cards leaving it).
const ORDER_REFRESH_MS: u64 = 60_000;
/// The scan page suggests another angle after this long without a code.
const SCAN_HINT_MS: u64 = 30_000;
/// "Forget this Paseo" needs a second tap within this long.
const FORGET_ARM_MS: u64 = 4_000;
/// The white pulse behind the lamp when a turn finishes, played once.
const PULSE_MS: u64 = 900;
/// Burn-in guard: once a minute the home page and the status bar glide to the
/// next offset of this loop, over DRIFT_GLIDE_MS.
const DRIFT_PATH: [(i32, i32); 8] = [
    (0, 0),
    (1, 1),
    (2, 0),
    (1, -1),
    (0, -2),
    (-1, -1),
    (-2, 0),
    (-1, 1),
];
const DRIFT_GLIDE_MS: u32 = 2_000;
/// After GSP refused writes (its queue was full), render again this soon.
const RENDER_RETRY_MS: u64 = 30;
/// Interaction-module LED brightness until Paseo says otherwise.
const LED_DEFAULT: u8 = 60;
/// A change of urgency glides the lamp to its new color this long. GSP cannot
/// tween the orb's color, so the logic steps it every [`FADE_STEP_MS`].
const LAMP_FADE_MS: u64 = 600;
/// Ripple strength (0..100) for a tool call and for a tap on an empty lamp.
const RIPPLE_TOOL: i32 = 55;
const RIPPLE_EMPTY: i32 = 95;
const FADE_STEP_MS: u64 = 33;

/// A color glide on the logic's clock.
#[derive(Clone, Copy)]
pub(crate) struct Fade {
    from: u32,
    to: u32,
    start: u64,
}

impl Fade {
    fn active(&self, now: u64) -> bool {
        self.from != self.to && now < self.start + LAMP_FADE_MS
    }

    /// The color at `now`, cubic ease-in-out per channel.
    fn at(&self, now: u64) -> u32 {
        let t = now.saturating_sub(self.start).min(LAMP_FADE_MS) as f32 / LAMP_FADE_MS as f32;
        let e = if t < 0.5 {
            4.0 * t * t * t
        } else {
            let u = 2.0 - 2.0 * t;
            1.0 - u * u * u / 2.0
        };
        let channel = |shift: u32| {
            let (a, b) = ((self.from >> shift) & 0xFF, (self.to >> shift) & 0xFF);
            let v = a as f32 + (b as f32 - a as f32) * e;
            ((v + 0.5) as u32).min(0xFF) << shift
        };
        channel(16) | channel(8) | channel(0)
    }
}
/// A one-line message on a card (an action failed) stays this long.
const NOTE_MS: u64 = 6_000;

/// What covers the three screens.
#[derive(Clone, Debug, PartialEq)]
pub enum Overlay {
    None,
    Welcome,
    NoWifi,
    NoPaseo,
    /// A scanned or typed address waits for "连接".
    Pair {
        config: Config,
        host: String,
    },
    Scan,
    /// Typing on the on-screen keyboard.
    Input {
        entry: Entry,
        error: Option<String>,
        /// A Wi-Fi password went out; waiting for the network to answer.
        joining: bool,
    },
    /// Networks in range, to pick one.
    Wifi,
    /// The network in use and the saved ones.
    NetInfo,
    /// The Paseo connection: address, latency, last sync.
    Link,
}

/// What the input page collects.
#[derive(Clone, Debug, PartialEq)]
pub enum Entry {
    /// Step 1 of 2: the daemon address.
    Address,
    /// Step 2 of 2: the daemon password for `url`.
    Password { url: String },
    /// The password of the Wi-Fi network `ssid`.
    WifiPassword { ssid: String },
}

#[derive(Clone, Debug)]
pub(crate) struct Press {
    pub agent: String,
    pub option: usize,
}

#[derive(Clone, Debug)]
pub(crate) struct Hold {
    pub agent: String,
    pub at: u64,
    pub shown: bool,
}

#[derive(Clone, Debug)]
struct Pending {
    agent: String,
    kind: ActKind,
}

/// A transient line on one agent's card.
#[derive(Clone, Debug)]
pub(crate) struct Note {
    pub agent: String,
    pub text: String,
    pub until: u64,
}

pub struct App {
    pub(crate) now: u64,
    /// Plugin epoch ms and the local clock when it was read.
    pub(crate) clock: Option<(i64, u64)>,
    pub(crate) tz: i32,
    pub(crate) device: String,
    pub(crate) session: Session,
    pub(crate) snapshot: Snapshot,
    pub(crate) has_snapshot: bool,
    /// Indices into `snapshot.agents`, most urgent first.
    pub(crate) order: Vec<usize>,
    /// Agents the user marked read on the board: id -> (`since` at the time,
    /// epoch ms of the mark).
    seen: BTreeMap<String, (Option<i64>, i64)>,
    pub(crate) screen: u16,
    /// Position in `order` of the card on screen.
    pub(crate) deck: usize,
    /// Which `cards` page shows it.
    pub(crate) slot: u16,
    /// Agent id each card page shows.
    pub(crate) slots: [Option<String>; SLOTS],
    pub(crate) overlay: Overlay,
    pub(crate) press: Option<Press>,
    pub(crate) hold: Option<Hold>,
    pending: BTreeMap<u32, Pending>,
    next_token: u32,
    /// Option index answered on the board, until the agent's ask changes.
    pub(crate) sent: BTreeMap<String, (String, usize)>,
    /// Agents stopped from this board: their end of turn is not announced.
    stopped_here: BTreeSet<String>,
    pub(crate) note: Option<Note>,
    pub(crate) net: Net,
    /// The last Wi-Fi scan, strongest first.
    pub(crate) networks: Vec<Ap>,
    /// A scan was asked for and has not answered yet.
    pub(crate) scanning: bool,
    /// When the scan page last showed or hinted (monotonic ms).
    scan_since: u64,
    /// "Forget this Paseo" is armed until then.
    pub(crate) forget_until: u64,
    /// The done pulse shows until then; `pulse_start` asks to play it.
    pub(crate) pulse_until: u64,
    pulse_start: bool,
    drift_step: usize,
    /// When the last snapshot arrived (monotonic ms).
    pub(crate) last_sync: Option<u64>,
    /// The Wi-Fi password page needs the keyboard again (a wrong password).
    keyboard_again: bool,
    /// Board settings last sent to (or taken from) Paseo; a difference is a
    /// change made on the board, which goes out with the next poll.
    synced: Option<BoardPrefs>,
    /// The `Settings::rev` applied last.
    settings_rev: Option<u64>,
    /// Quiet hours from Paseo, minutes after local midnight (equal = none).
    quiet: (u16, u16),
    /// Interaction-module LED brightness from Paseo, and what the board has.
    led: u8,
    led_sent: Option<u8>,
    net_down_since: Option<u64>,
    paseo_down_since: Option<u64>,
    pub(crate) banner_dismissed: bool,
    pub(crate) power: Power,
    pub(crate) prefs: Prefs,
    pub(crate) dnd_until: Option<u64>,
    face_down: bool,
    pub(crate) module: bool,
    presence: bool,
    ambient: Option<u8>,
    last_touch: u64,
    last_presence: u64,
    alert_until: u64,
    display: Option<Display>,
    glow: Option<Glow>,
    pub(crate) painter: Painter,
    pub(crate) host: String,
    demo: Option<Demo>,
    /// The lamp's color glide; the view advances it while drawing.
    pub(crate) lamp_fade: Cell<Option<Fade>>,
    /// A running agent started a new tool call since the last frame.
    ripple: bool,
    /// The last render had writes GSP refused; render again soon.
    render_retry: bool,
    /// Phase of the "connecting" Wi-Fi arcs, advanced by `poll`.
    pub(crate) wifi_phase: u8,
    last_second: u64,
}

impl App {
    /// `now_ms` is the platform's monotonic clock; `device` a short stable id.
    pub fn new(now_ms: u64, device: &str) -> Self {
        let mut app = Self {
            now: now_ms,
            clock: None,
            tz: 0,
            device: device.to_string(),
            session: Session::new(device),
            snapshot: Snapshot::default(),
            has_snapshot: false,
            order: Vec::new(),
            seen: BTreeMap::new(),
            screen: HOME,
            deck: 0,
            slot: 0,
            slots: [None, None, None],
            overlay: Overlay::Welcome,
            press: None,
            hold: None,
            pending: BTreeMap::new(),
            next_token: 0,
            sent: BTreeMap::new(),
            stopped_here: BTreeSet::new(),
            note: None,
            net: Net::default(),
            networks: Vec::new(),
            scanning: false,
            scan_since: 0,
            forget_until: 0,
            pulse_until: 0,
            pulse_start: false,
            drift_step: 0,
            last_sync: None,
            keyboard_again: false,
            synced: None,
            settings_rev: None,
            quiet: (0, 0),
            led: LED_DEFAULT,
            led_sent: None,
            net_down_since: None,
            paseo_down_since: None,
            banner_dismissed: false,
            power: Power::default(),
            prefs: Prefs::default(),
            dnd_until: None,
            face_down: false,
            module: false,
            presence: false,
            ambient: None,
            last_touch: now_ms,
            last_presence: now_ms,
            alert_until: 0,
            display: None,
            glow: None,
            painter: Painter::new(),
            host: String::new(),
            demo: None,
            lamp_fade: Cell::new(None),
            ripple: false,
            render_retry: false,
            wifi_phase: 0,
            last_second: now_ms / 1000,
        };
        app.update_overlay();
        app
    }

    /// Runs on built-in sample agents instead of Paseo (the simulator, and
    /// boards without a daemon). Actions on the cards change the samples.
    /// `offset_ms` starts the script that far in.
    pub fn start_demo(&mut self, offset_ms: u64) {
        let epoch = self.epoch().unwrap_or(crate::demo::EPOCH_MS);
        self.session.configure(None, self.now);
        self.demo = Some(Demo::new(self.now, epoch, offset_ms));
        self.reset_fleet();
        self.host = String::from("demo");
        self.overlay = Overlay::None;
        self.tick_demo(&mut NullSys);
    }

    fn reset_fleet(&mut self) {
        self.snapshot = Snapshot::default();
        self.has_snapshot = false;
        self.order.clear();
        self.seen.clear();
        self.sent.clear();
        self.stopped_here.clear();
        self.pending.clear();
        self.hold = None;
        self.press = None;
        self.note = None;
        self.deck = 0;
    }

    pub fn is_demo(&self) -> bool {
        self.demo.is_some()
    }

    pub fn rejections(&self) -> u32 {
        self.painter.rejections()
    }

    pub fn take_first_rejection(&mut self) -> Option<crate::ui::Rejection> {
        self.painter.take_first_rejection()
    }

    // ------------------------------------------------------------ events

    /// Handles one input event at `now_ms` (the clock [`App::poll`] uses).
    /// Timed gestures start from this time, so it must be the event's own time,
    /// not the last poll's.
    pub fn handle(&mut self, now_ms: u64, event: Event, ui: &mut impl Ui, sys: &mut impl Sys) {
        self.now = self.now.max(now_ms);
        match event {
            Event::Call { action, arg } => {
                self.touched();
                self.on_call(action, arg, ui, sys);
            }
            Event::Settled { key, value } => self.on_settled(key, value, sys),
            Event::Touch => self.touched(),
            Event::Link(link) => match link {
                Link::Opened => self.session.on_open(self.now),
                Link::Closed(why) => self.session.on_close(self.now, why),
                Link::Text(text) => self.session.on_text(self.now, &text),
            },
            Event::Net(net) => self.on_net(net),
            Event::Power(power) => self.power = power,
            Event::Button(button) => {
                self.touched();
                self.on_button(button, ui, sys);
            }
            Event::Module(attached) => {
                self.module = attached;
                if !attached {
                    self.presence = false;
                    self.ambient = None;
                    self.glow = None;
                }
            }
            Event::Presence(near) => {
                // Someone was there until now either way: the off timer starts
                // when they leave, not when they arrived.
                self.presence = near;
                self.last_presence = self.now;
            }
            Event::Ambient(level) => self.ambient = Some(level),
            Event::Motion(motion) => match motion {
                Motion::Lifted => self.touched(),
                Motion::FaceDown => self.face_down = true,
                Motion::FaceUp => {
                    self.face_down = false;
                    self.touched();
                }
            },
            Event::Scanned(payload) => self.on_scanned(&payload, sys),
            Event::NoCamera => {
                if self.overlay == Overlay::Scan {
                    sys.camera(false);
                    self.note_on("", "没有检测到摄像头模块：请改用手动输入");
                }
            }
            Event::Networks(networks) => {
                self.networks = networks;
                self.scanning = false;
            }
            Event::Demo { offset_ms } => {
                self.start_demo(offset_ms);
                self.deck = 0;
                self.fill_slots();
                self.screen = HOME;
                self.painter.page(ui, gsp::GSP_OBJ_KEY_SCREENS, HOME, false);
                self.painter
                    .run(ui.set_drawer(gsp::GSP_OBJ_KEY_CC, false, false));
                self.update_focus();
            }
            Event::Restored { config, prefs } => {
                self.prefs = prefs;
                if self.demo.is_none() {
                    self.session.configure(config, self.now);
                }
            }
        }
        self.after(ui, sys);
    }

    /// Time-driven work. Calling it early or repeatedly is harmless.
    pub fn poll(&mut self, now_ms: u64, ui: &mut impl Ui, sys: &mut impl Sys) {
        self.now = now_ms;
        if self.demo.is_none() {
            self.session.poll(now_ms);
        } else {
            self.tick_demo(sys);
        }
        if let Some(hold) = &mut self.hold {
            if !hold.shown && now_ms >= hold.at + HOLD_SHOW_MS {
                hold.shown = true;
                self.painter.run(ui.animate_value(
                    gsp::GSP_OBJ_KEY_HOLD_RING,
                    0,
                    100,
                    HOLD_STOP_MS as u32,
                    crate::ui::Ease::Linear,
                ));
            }
            if now_ms >= hold.at + HOLD_SHOW_MS + HOLD_STOP_MS {
                let agent = self.hold.take().map(|h| h.agent).unwrap_or_default();
                // Marked before sending: the demo answers within send().
                self.stopped_here.insert(agent.clone());
                if self.send(&agent, ActKind::Stop, None, None, sys) {
                    self.feel(Haptic::Confirm, sys);
                } else {
                    self.stopped_here.remove(&agent);
                }
            }
        }
        if self.note.as_ref().is_some_and(|n| now_ms >= n.until) {
            self.note = None;
        }
        if self.net.state == NetState::Connecting {
            self.wifi_phase = ((now_ms / 400) % 4) as u8;
        }
        if now_ms / ORDER_REFRESH_MS != self.last_second * 1000 / ORDER_REFRESH_MS {
            if self.has_snapshot {
                let current = self.current_agent().map(|a| a.id.clone());
                self.refresh_order(current);
            }
            self.drift(ui);
        }
        if self.overlay == Overlay::Scan && now_ms >= self.scan_since + SCAN_HINT_MS {
            self.scan_since = now_ms;
            self.note_on("", "认不出来：换个角度试试，或者改用手动输入");
        }
        self.last_second = now_ms / 1000;
        self.after(ui, sys);
    }

    /// When [`App::poll`] is next due, on the same clock.
    pub fn next_deadline_ms(&self) -> u64 {
        let mut next = (self.now / 1000 + 1) * 1000;
        next = next.min(self.session.next_deadline());
        for at in [self.pulse_until, self.forget_until] {
            if at > self.now {
                next = next.min(at);
            }
        }
        if self.overlay == Overlay::Scan {
            next = next.min(self.scan_since + SCAN_HINT_MS);
        }
        if self.render_retry {
            next = next.min(self.now + RENDER_RETRY_MS);
        }
        if self.lamp_fade.get().is_some_and(|f| f.active(self.now)) {
            next = next.min(self.now + FADE_STEP_MS);
        }
        if let Some(hold) = &self.hold {
            let at = if hold.shown {
                hold.at + HOLD_SHOW_MS + HOLD_STOP_MS
            } else {
                hold.at + HOLD_SHOW_MS
            };
            next = next.min(at);
        }
        if self.net.state == NetState::Connecting {
            next = next.min((self.now / 400 + 1) * 400);
        }
        if let Some(demo) = &self.demo {
            next = next.min(demo.next_change());
        }
        next.max(self.now + 1)
    }

    fn after(&mut self, ui: &mut impl Ui, sys: &mut impl Sys) {
        self.sync_prefs();
        self.pump_session(sys);
        self.update_overlay();
        self.apply_power(sys);
        self.apply_glow(sys);
        if core::mem::take(&mut self.pulse_start) {
            self.painter.run(ui.animate_i32(
                gsp::GSP_OBJ_KEY_LAMP_PULSE,
                gsp::GSP_PROP_KEY_PHASE,
                0,
                i32::from(u16::MAX),
                PULSE_MS as u32,
                crate::ui::Ease::Out,
            ));
        }
        if core::mem::take(&mut self.keyboard_again) {
            self.attach_keyboard(ui);
        }
        if core::mem::take(&mut self.ripple) {
            self.painter.run(ui.set_i32(
                gsp::GSP_OBJ_KEY_LAMP,
                gsp::GSP_PROP_KEY_RIPPLE,
                RIPPLE_TOOL,
            ));
        }
        self.render(ui);
        // GSP's command queue was full: render again shortly instead of
        // leaving part of the screen stale until the next second.
        self.render_retry = self.painter.take_failed();
    }

    fn pump_session(&mut self, sys: &mut impl Sys) {
        for out in self.session.take_out() {
            sys.link(out);
        }
        for notice in self.session.take_notices() {
            match notice {
                Notice::Ready { host } => {
                    self.host = host;
                    self.paseo_down_since = None;
                    self.banner_dismissed = false;
                    if self.overlay == Overlay::NoPaseo {
                        self.overlay = Overlay::None;
                    }
                }
                Notice::Snapshot(snapshot) => self.apply(snapshot, sys),
                Notice::Acted { token, result } => self.acted(token, &result, sys),
                Notice::Down(down) => {
                    if down == Down::None {
                        self.paseo_down_since = None;
                    } else if self.paseo_down_since.is_none() {
                        self.paseo_down_since = Some(self.now);
                    }
                }
            }
        }
    }

    fn on_call(&mut self, action: u32, arg: i32, ui: &mut impl Ui, sys: &mut impl Sys) {
        use gsp::*;
        let option = |press, long, release, cancel| -> Option<(usize, u8)> {
            for (j, ids) in [
                (
                    0,
                    [
                        GSP_ACT_ID_OPT0_PRESS,
                        GSP_ACT_ID_OPT0_LONG,
                        GSP_ACT_ID_OPT0_RELEASE,
                        GSP_ACT_ID_OPT0_CANCEL,
                    ],
                ),
                (
                    1,
                    [
                        GSP_ACT_ID_OPT1_PRESS,
                        GSP_ACT_ID_OPT1_LONG,
                        GSP_ACT_ID_OPT1_RELEASE,
                        GSP_ACT_ID_OPT1_CANCEL,
                    ],
                ),
                (
                    2,
                    [
                        GSP_ACT_ID_OPT2_PRESS,
                        GSP_ACT_ID_OPT2_LONG,
                        GSP_ACT_ID_OPT2_RELEASE,
                        GSP_ACT_ID_OPT2_CANCEL,
                    ],
                ),
                (
                    3,
                    [
                        GSP_ACT_ID_OPT3_PRESS,
                        GSP_ACT_ID_OPT3_LONG,
                        GSP_ACT_ID_OPT3_RELEASE,
                        GSP_ACT_ID_OPT3_CANCEL,
                    ],
                ),
            ] {
                for (what, id) in ids.iter().enumerate() {
                    if *id == action {
                        return Some((j, [press, long, release, cancel][what]));
                    }
                }
            }
            None
        };
        if let Some((j, what)) = option(0, 1, 2, 3) {
            match what {
                0 => self.option_pressed(j, sys),
                1 => self.option_held(j, sys),
                _ => {
                    if self.press.as_ref().is_some_and(|p| p.option == j) {
                        self.press = None;
                    }
                }
            }
            return;
        }
        match action {
            GSP_ACT_ID_LAMP_TAP => self.jump_to_urgent(ui, sys),
            GSP_ACT_ID_HOLD_PRESS => self.hold_pressed(sys),
            GSP_ACT_ID_HOLD_RELEASE | GSP_ACT_ID_HOLD_CANCEL => {
                if self.hold.take().is_some_and(|h| h.shown) {
                    self.painter.run(ui.set_value(GSP_OBJ_KEY_HOLD_RING, 0));
                }
            }
            GSP_ACT_ID_SEEN => self.mark_seen(sys),
            GSP_ACT_ID_SCAN_OPEN | GSP_ACT_ID_REPAIR => {
                self.overlay = Overlay::Scan;
                self.scan_since = self.now;
                self.note = None;
                sys.camera(true);
            }
            GSP_ACT_ID_SCAN_CANCEL => {
                sys.camera(false);
                self.overlay = Overlay::None;
            }
            GSP_ACT_ID_MANUAL_OPEN => {
                if self.overlay == Overlay::Scan {
                    sys.camera(false);
                }
                self.open_input(Entry::Address, ui);
            }
            GSP_ACT_ID_WIFI_PICK => {
                self.overlay = Overlay::Wifi;
                self.scan_networks(sys);
            }
            GSP_ACT_ID_WF_RESCAN => self.scan_networks(sys),
            GSP_ACT_ID_WF_ROW0 | GSP_ACT_ID_WF_ROW1 | GSP_ACT_ID_WF_ROW2 | GSP_ACT_ID_WF_ROW3
            | GSP_ACT_ID_WF_ROW4 | GSP_ACT_ID_WF_ROW5 => {
                let row = (action - GSP_ACT_ID_WF_ROW0) as usize;
                self.pick_network(row, ui, sys);
            }
            GSP_ACT_ID_KB_KEY => self.input_done(ui, sys),
            GSP_ACT_ID_IN_CANCEL | GSP_ACT_ID_WF_CANCEL | GSP_ACT_ID_NI_CLOSE
            | GSP_ACT_ID_LK_CLOSE => {
                self.overlay = Overlay::None;
            }
            GSP_ACT_ID_LK_FORGET => self.forget_paseo(sys),
            GSP_ACT_ID_CC_WIFI => {
                self.painter
                    .run(ui.set_drawer(gsp::GSP_OBJ_KEY_CC, false, true));
                self.overlay = Overlay::NetInfo;
                self.feel(Haptic::Tick, sys);
            }
            GSP_ACT_ID_PAIR_CANCEL => self.overlay = Overlay::None,
            GSP_ACT_ID_PAIR_CONNECT => {
                if let Overlay::Pair { config, .. } =
                    core::mem::replace(&mut self.overlay, Overlay::None)
                {
                    sys.save_config(Some(&config));
                    self.has_snapshot = false;
                    self.session.configure(Some(config), self.now);
                    self.feel(Haptic::Confirm, sys);
                }
            }
            GSP_ACT_ID_RETRY => {
                self.session.retry_now(self.now);
                if self.net.state != NetState::Online {
                    sys.wifi(WifiCmd::Retry);
                }
            }
            GSP_ACT_ID_BANNER_TAP => self.overlay = Overlay::NoPaseo,
            GSP_ACT_ID_CC_PASEO => {
                self.painter
                    .run(ui.set_drawer(gsp::GSP_OBJ_KEY_CC, false, true));
                self.overlay = Overlay::Link;
                self.forget_until = 0;
                self.feel(Haptic::Tick, sys);
            }
            GSP_ACT_ID_CC_DND => {
                self.dnd_until = if self.dnd() {
                    None
                } else {
                    Some(self.now + DND_MS)
                };
                self.feel(Haptic::Tick, sys);
            }
            GSP_ACT_ID_CC_SOUND => {
                self.prefs.sound = !self.prefs.sound;
                sys.save_prefs(&self.prefs);
                self.feel(Haptic::Tick, sys);
            }
            GSP_ACT_ID_CC_HAPTIC => {
                self.prefs.haptic = !self.prefs.haptic;
                sys.save_prefs(&self.prefs);
                self.feel(Haptic::Tick, sys);
            }
            GSP_ACT_ID_CC_AUTO => {
                self.prefs.auto = !self.prefs.auto;
                sys.save_prefs(&self.prefs);
            }
            GSP_ACT_ID_CC_BRIGHT => {
                self.prefs.brightness = arg.clamp(5, 100) as u8;
                self.prefs.auto = false;
                sys.save_prefs(&self.prefs);
            }
            _ => {}
        }
    }

    fn on_settled(&mut self, key: u32, value: u16, sys: &mut impl Sys) {
        if key == gsp::GSP_OBJ_KEY_SCREENS {
            if value != self.screen {
                self.feel(Haptic::Tick, sys);
            }
            self.screen = value;
            self.painter.note_page(key, value);
            self.update_focus();
        } else if key == gsp::GSP_OBJ_KEY_CARDS {
            self.painter.note_page(key, value);
            let n = self.order.len();
            if n > 0 && value != self.slot {
                let forward = value == (self.slot + 1) % SLOTS as u16;
                self.deck = if forward {
                    (self.deck + 1) % n
                } else {
                    (self.deck + n - 1) % n
                };
                self.slot = value;
                self.press = None;
                self.hold = None;
                self.fill_slots();
                self.feel(Haptic::Tick, sys);
                self.update_focus();
            }
        }
    }

    fn on_net(&mut self, net: Net) {
        let was_online = self.net.state == NetState::Online;
        let online = net.state == NetState::Online;
        if online {
            self.net_down_since = None;
            if !was_online {
                self.session.retry_now(self.now);
            }
        } else if net.state != NetState::Off && self.net_down_since.is_none() {
            self.net_down_since = Some(self.now);
        }
        self.net = net;
        self.join_result();
    }

    fn on_button(&mut self, button: Button, ui: &mut impl Ui, sys: &mut impl Sys) {
        match button {
            Button::AiClick => {
                if self.screen == DECK && self.deck == 0 {
                    self.painter.page(ui, gsp::GSP_OBJ_KEY_SCREENS, HOME, true);
                } else {
                    self.jump_to_urgent(ui, sys);
                }
            }
            Button::AiLong => {
                self.dnd_until = if self.dnd() {
                    None
                } else {
                    Some(self.now + DND_MS)
                };
                self.feel(Haptic::Confirm, sys);
            }
            Button::Prev | Button::Next => {
                if self.screen != DECK || self.order.len() < 2 {
                    return;
                }
                let step = if button == Button::Next {
                    1
                } else {
                    SLOTS as u16 - 1
                };
                let page = (self.slot + step) % SLOTS as u16;
                self.painter
                    .run(ui.set_page(gsp::GSP_OBJ_KEY_CARDS, page, true));
            }
        }
    }

    // ------------------------------------------------------------ setup pages

    /// Shows the input page for `entry` with the keyboard on an empty field.
    fn open_input(&mut self, entry: Entry, ui: &mut impl Ui) {
        self.overlay = Overlay::Input {
            entry,
            error: None,
            joining: false,
        };
        self.attach_keyboard(ui);
    }

    fn attach_keyboard(&mut self, ui: &mut impl Ui) {
        // The keyboard writes the field itself; the views never do.
        self.painter.run(ui.keyboard_attach(
            gsp::GSP_ACT_ID_KB_KEY as u16,
            gsp::GSP_BIND_IN_TEXT as crate::ui::Bind,
        ));
        self.painter
            .run(ui.set_text(gsp::GSP_BIND_IN_TEXT as crate::ui::Bind, ""));
    }

    /// The keyboard's OK key: take the field and go to the next step.
    fn input_done(&mut self, ui: &mut impl Ui, sys: &mut impl Sys) {
        let Overlay::Input { entry, joining, .. } = &self.overlay else {
            return;
        };
        if *joining {
            return;
        }
        let entry = entry.clone();
        let text = match ui.keyboard_text() {
            Ok(text) => String::from(text.trim()),
            Err(code) => {
                self.painter.run(Err(code));
                return;
            }
        };
        let error = |why: &str| Overlay::Input {
            entry: entry.clone(),
            error: Some(String::from(why)),
            joining: false,
        };
        match entry.clone() {
            Entry::Address => match qr::normalize_url(&text) {
                Ok(url) => {
                    self.open_input(Entry::Password { url }, ui);
                    self.feel(Haptic::Tick, sys);
                }
                Err(why) => {
                    self.overlay = error(why);
                    self.feel(Haptic::Fail, sys);
                }
            },
            Entry::Password { url } => {
                let host = qr::host_of(&url);
                self.overlay = Overlay::Pair {
                    config: Config {
                        url,
                        password: text,
                    },
                    host,
                };
                self.feel(Haptic::Tick, sys);
            }
            Entry::WifiPassword { ssid } => {
                if text.is_empty() {
                    self.overlay = error("请输入这个网络的密码");
                    return;
                }
                sys.wifi(WifiCmd::Join {
                    ssid,
                    password: text,
                });
                self.overlay = Overlay::Input {
                    entry,
                    error: None,
                    joining: true,
                };
                self.feel(Haptic::Confirm, sys);
            }
        }
    }

    /// How a Wi-Fi password typed on the board turned out.
    fn join_result(&mut self) {
        let Overlay::Input {
            entry: Entry::WifiPassword { ssid },
            joining: true,
            ..
        } = &self.overlay
        else {
            return;
        };
        if self.net.ssid != *ssid {
            return;
        }
        let why = match self.net.state {
            NetState::Online => {
                self.overlay = Overlay::None;
                return;
            }
            NetState::WrongPassword => "密码不对，再输一次",
            NetState::NotFound => "找不到这个网络了",
            NetState::NoAddress => "连上了，但拿不到地址",
            _ => return,
        };
        let ssid = ssid.clone();
        self.overlay = Overlay::Input {
            entry: Entry::WifiPassword { ssid },
            error: Some(String::from(why)),
            joining: false,
        };
        self.keyboard_again = true;
    }

    fn scan_networks(&mut self, sys: &mut impl Sys) {
        self.networks.clear();
        self.scanning = true;
        sys.wifi(WifiCmd::Scan);
    }

    /// A row on the network list: saved and open networks join at once, the
    /// rest ask for their password.
    fn pick_network(&mut self, row: usize, ui: &mut impl Ui, sys: &mut impl Sys) {
        let Some(ap) = self.networks.get(row).cloned() else {
            return;
        };
        if ap.saved || !ap.secure {
            // An empty password joins with the saved one (or none).
            sys.wifi(WifiCmd::Join {
                ssid: ap.ssid,
                password: String::new(),
            });
            self.overlay = Overlay::None;
            self.feel(Haptic::Confirm, sys);
        } else {
            self.open_input(Entry::WifiPassword { ssid: ap.ssid }, ui);
        }
    }

    /// Two taps: the first arms, the second forgets the address and password.
    fn forget_paseo(&mut self, sys: &mut impl Sys) {
        if self.now >= self.forget_until {
            self.forget_until = self.now + FORGET_ARM_MS;
            self.feel(Haptic::Tick, sys);
            return;
        }
        self.forget_until = 0;
        sys.save_config(None);
        self.session.configure(None, self.now);
        self.reset_fleet();
        self.clock = None;
        self.host.clear();
        self.last_sync = None;
        self.overlay = Overlay::None;
        self.feel(Haptic::Confirm, sys);
    }

    /// Burn-in guard: slide the always-on parts to the next offset.
    fn drift(&mut self, ui: &mut impl Ui) {
        let (fx, fy) = DRIFT_PATH[self.drift_step];
        self.drift_step = (self.drift_step + 1) % DRIFT_PATH.len();
        let (tx, ty) = DRIFT_PATH[self.drift_step];
        // Both groups sit at (0, 0) in the scene, so the offset is the position.
        for key in [gsp::GSP_OBJ_KEY_HOME_BODY, gsp::GSP_OBJ_KEY_SB] {
            for (property, from, to) in
                [(gsp::GSP_PROP_KEY_X, fx, tx), (gsp::GSP_PROP_KEY_Y, fy, ty)]
            {
                if from != to {
                    self.painter.run(ui.animate_i32(
                        key,
                        property,
                        from,
                        to,
                        DRIFT_GLIDE_MS,
                        crate::ui::Ease::InOut,
                    ));
                }
            }
        }
    }

    fn on_scanned(&mut self, payload: &str, sys: &mut impl Sys) {
        match qr::parse(payload) {
            Some(Code::Wifi { ssid, password }) => {
                sys.camera(false);
                sys.wifi(WifiCmd::Join { ssid, password });
                self.overlay = Overlay::None;
                self.feel(Haptic::Confirm, sys);
            }
            Some(Code::Farol {
                url,
                password,
                host,
            }) => {
                sys.camera(false);
                self.overlay = Overlay::Pair {
                    config: Config { url, password },
                    host,
                };
                self.feel(Haptic::Confirm, sys);
            }
            None => {
                let lower = payload.trim().to_ascii_lowercase();
                let what = if lower.starts_with("http://") || lower.starts_with("https://") {
                    "这是一个网址，不是 Farol 连接码或 Wi-Fi 码"
                } else {
                    "这不是 Farol 连接码或 Wi-Fi 码"
                };
                self.scan_since = self.now;
                self.note_on("", what);
            }
        }
    }

    // ------------------------------------------------------------ data

    fn apply(&mut self, mut snapshot: Snapshot, sys: &mut impl Sys) {
        if snapshot.v != crate::model::VERSION {
            return;
        }
        if let Some(settings) = snapshot.settings {
            self.apply_settings(settings, sys);
        }
        // Local read marks hold until the agent finishes a newer turn.
        self.seen.retain(|id, (since, _)| {
            snapshot
                .agents
                .iter()
                .any(|a| &a.id == id && a.since == *since)
        });
        for agent in &mut snapshot.agents {
            if matches!(agent.state, State::Done | State::Fail) && self.seen.contains_key(&agent.id)
            {
                agent.state = State::Seen;
            }
        }
        // An answer sent from the board stays shown until the ask changes.
        self.sent.retain(|id, (request, _)| {
            snapshot
                .agents
                .iter()
                .any(|a| &a.id == id && a.ask.as_ref().is_some_and(|q| &q.request_id == request))
        });
        self.cue_changes(&snapshot, sys);
        self.ripple |= snapshot.agents.iter().any(|a| {
            a.state == State::Work
                && !a.now.is_empty()
                && self
                    .agent(&a.id)
                    .is_some_and(|b| b.state == State::Work && b.now != a.now)
        });
        let current = self.current_agent().map(|a| a.id.clone());
        self.clock = Some((snapshot.now, self.now));
        self.tz = snapshot.tz;
        if !snapshot.host.is_empty() {
            self.host = snapshot.host.clone();
        }
        self.snapshot = snapshot;
        self.has_snapshot = true;
        self.last_sync = Some(self.now);
        self.refresh_order(current);
    }

    /// Re-sorts the deck, keeping `current` (an agent id) on screen when it is
    /// still in it.
    fn refresh_order(&mut self, current: Option<String>) {
        self.sort();
        if let Some(id) = current
            && let Some(pos) = self.position_of(&id)
        {
            self.deck = pos;
        }
        if self.deck >= self.order.len() {
            self.deck = 0;
        }
        self.fill_slots();
        self.update_focus();
    }

    fn cue_changes(&mut self, next: &Snapshot, sys: &mut impl Sys) {
        if !self.has_snapshot {
            return;
        }
        let mut call = false;
        let mut fail = false;
        let mut done = false;
        for agent in &next.agents {
            let before = self
                .snapshot
                .agents
                .iter()
                .find(|a| a.id == agent.id)
                .map(|a| a.state);
            match (before, agent.state) {
                (Some(State::Call), State::Call) | (Some(State::Fail), State::Fail) => {}
                (_, State::Call) => call = true,
                (_, State::Fail) => fail = true,
                (Some(State::Work), State::Done) => done |= !self.stopped_here.contains(&agent.id),
                _ => {}
            }
        }
        self.stopped_here.retain(|id| {
            next.agents
                .iter()
                .any(|a| a.id == *id && a.state == State::Work)
        });
        if call {
            self.alert_until = self.now + ALERT_AWAKE_MS;
            self.feel(Haptic::Call, sys);
            self.ring(Chime::Call, sys);
        } else if fail {
            self.alert_until = self.now + ALERT_AWAKE_MS;
            self.feel(Haptic::Fail, sys);
            self.ring(Chime::Fail, sys);
        } else if done {
            self.pulse_start = true;
            self.pulse_until = self.now + PULSE_MS;
            self.feel(Haptic::Tick, sys);
            self.ring(Chime::Done, sys);
            if self.module && !self.dnd() {
                sys.glow(Glow::Flash);
                self.glow = None;
            }
        }
    }

    /// A read card leaves the deck an hour after it was read (or, when it was
    /// read elsewhere, an hour after its turn ended).
    fn expired(&self, agent: &Agent, now: Option<i64>) -> bool {
        let Some(now) = now else { return false };
        let read_at = self.seen.get(&agent.id).map(|(_, at)| *at);
        let since = agent.since.max(read_at).unwrap_or(now);
        agent.state == State::Seen && now - since >= SEEN_KEEP_MS
    }

    fn sort(&mut self) {
        let now = self.epoch();
        let agents = &self.snapshot.agents;
        let mut order: Vec<usize> = (0..agents.len())
            .filter(|&i| !self.expired(&agents[i], now))
            .collect();
        order.sort_by(|&a, &b| {
            let (a, b) = (&agents[a], &agents[b]);
            a.state
                .cmp(&b.state)
                .then_with(|| b.since.cmp(&a.since))
                .then_with(|| a.id.cmp(&b.id))
        });
        self.order = order;
    }

    fn position_of(&self, id: &str) -> Option<usize> {
        self.order
            .iter()
            .position(|&i| self.snapshot.agents[i].id == id)
    }

    pub(crate) fn agent_at(&self, deck: usize) -> Option<&Agent> {
        self.order.get(deck).map(|&i| &self.snapshot.agents[i])
    }

    pub(crate) fn current_agent(&self) -> Option<&Agent> {
        self.agent_at(self.deck)
    }

    pub(crate) fn agent(&self, id: &str) -> Option<&Agent> {
        self.snapshot.agents.iter().find(|a| a.id == id)
    }

    /// Puts the current agent on the current page and its neighbours on the
    /// pages a swipe reveals next.
    fn fill_slots(&mut self) {
        let n = self.order.len();
        if n == 0 {
            self.slots = [None, None, None];
            return;
        }
        let s = usize::from(self.slot);
        let id = |deck: usize| {
            Some(
                self.agent_at(deck % n)
                    .map(|a| a.id.clone())
                    .unwrap_or_default(),
            )
        };
        let mut slots = [None, None, None];
        slots[s] = id(self.deck);
        slots[(s + 1) % SLOTS] = id(self.deck + 1);
        slots[(s + SLOTS - 1) % SLOTS] = id(self.deck + n - 1);
        self.slots = slots;
    }

    fn update_focus(&mut self) {
        let focus = if self.screen == DECK {
            self.current_agent().map(|a| a.id.clone())
        } else {
            None
        };
        let screen = match self.screen {
            DECK => "deck",
            HOME => "home",
            _ => "usage",
        };
        if self.demo.is_none() {
            self.session.set_focus(focus.as_deref(), screen);
        }
    }

    fn jump_to_urgent(&mut self, ui: &mut impl Ui, sys: &mut impl Sys) {
        if self.order.is_empty() {
            self.painter.run(ui.set_i32(
                gsp::GSP_OBJ_KEY_LAMP,
                gsp::GSP_PROP_KEY_RIPPLE,
                RIPPLE_EMPTY,
            ));
            self.feel(Haptic::Tick, sys);
            return;
        }
        self.deck = 0;
        self.fill_slots();
        self.painter.page(ui, gsp::GSP_OBJ_KEY_SCREENS, DECK, true);
        self.screen = DECK;
        self.update_focus();
    }

    // ------------------------------------------------------------ actions

    fn read_only(&self) -> bool {
        self.snapshot.read_only
    }

    fn option_pressed(&mut self, option: usize, sys: &mut impl Sys) {
        let Some(agent) = self.current_agent() else {
            return;
        };
        let answerable = agent.state == State::Call
            && agent.ask.as_ref().is_some_and(|q| option < q.options.len())
            && !self.sent.contains_key(&agent.id);
        if !answerable {
            return;
        }
        if self.read_only() {
            let id = agent.id.clone();
            self.note_on(&id, "只读模式：在 Paseo 里回答");
            return;
        }
        self.press = Some(Press {
            agent: agent.id.clone(),
            option,
        });
        self.feel(Haptic::Tick, sys);
    }

    fn option_held(&mut self, option: usize, sys: &mut impl Sys) {
        let Some(press) = self.press.take() else {
            return;
        };
        if press.option != option {
            return;
        }
        let Some(agent) = self.agent(&press.agent) else {
            return;
        };
        let Some(ask) = agent.ask.clone() else { return };
        let id = agent.id.clone();
        if self.send(
            &id,
            ActKind::Answer,
            Some(ask.request_id.clone()),
            Some(option as u8),
            sys,
        ) {
            self.sent.insert(id, (ask.request_id, option));
            self.feel(Haptic::Confirm, sys);
        }
    }

    fn hold_pressed(&mut self, sys: &mut impl Sys) {
        let Some(agent) = self.current_agent() else {
            return;
        };
        if agent.state != State::Work {
            return;
        }
        let id = agent.id.clone();
        if !agent.stoppable {
            return;
        }
        if self.read_only() {
            self.note_on(&id, "只读模式：在 Paseo 里停止");
            return;
        }
        self.hold = Some(Hold {
            agent: id,
            at: self.now,
            shown: false,
        });
        let _ = sys;
    }

    fn mark_seen(&mut self, sys: &mut impl Sys) {
        let Some(agent) = self.current_agent() else {
            return;
        };
        if !matches!(agent.state, State::Done | State::Fail) {
            return;
        }
        let (id, since) = (agent.id.clone(), agent.since);
        let read_at = self.epoch().unwrap_or_default();
        self.seen.insert(id.clone(), (since, read_at));
        if let Some(i) = self.snapshot.agents.iter().position(|a| a.id == id) {
            self.snapshot.agents[i].state = State::Seen;
        }
        self.send(&id, ActKind::Seen, None, None, sys);
        self.feel(Haptic::Tick, sys);
        self.sort();
        if let Some(pos) = self.position_of(&id) {
            self.deck = pos;
        }
        self.fill_slots();
    }

    /// Sends an action; false (with a note) when it could not go out.
    fn send(
        &mut self,
        agent: &str,
        kind: ActKind,
        request_id: Option<String>,
        choice: Option<u8>,
        sys: &mut impl Sys,
    ) -> bool {
        let act = Act {
            agent: agent.to_string(),
            device: alloc::format!("farol-{}", self.device),
            kind,
            request_id,
            choice,
        };
        self.next_token = self.next_token.wrapping_add(1);
        let token = self.next_token;
        let sent = if let Some(demo) = &mut self.demo {
            let result = demo.act(&act, self.now);
            self.pending.insert(
                token,
                Pending {
                    agent: agent.to_string(),
                    kind,
                },
            );
            self.acted(token, &result, sys);
            self.tick_demo(sys);
            true
        } else if self.session.act(&act, token) {
            self.pending.insert(
                token,
                Pending {
                    agent: agent.to_string(),
                    kind,
                },
            );
            true
        } else {
            false
        };
        if !sent && kind != ActKind::Seen {
            self.note_on(agent, "未连接 Paseo，没有发出去");
            self.feel(Haptic::Fail, sys);
        }
        sent
    }

    fn acted(&mut self, token: u32, result: &ActResult, sys: &mut impl Sys) {
        let Some(pending) = self.pending.remove(&token) else {
            return;
        };
        if result.ok {
            if pending.kind == ActKind::Stop {
                self.note_on(&pending.agent, "已请求停止这一轮");
            }
            return;
        }
        self.sent.remove(&pending.agent);
        self.stopped_here.remove(&pending.agent);
        if pending.kind == ActKind::Seen {
            return;
        }
        let text = match result.reason.as_str() {
            "already_resolved" if pending.kind == ActKind::Stop => String::from("这一轮已经结束了"),
            "already_resolved" => String::from("已在别处回答"),
            "read_only" => String::from("只读模式：在 Paseo 里操作"),
            "disconnected" => String::from("连接断了，没有发出去"),
            "not_found" => String::from("这个 agent 已经不在了"),
            _ if result.message.is_empty() => String::from("没有发出去"),
            _ => alloc::format!("没有发出去：{}", result.message),
        };
        self.note_on(&pending.agent, &text);
        self.feel(Haptic::Fail, sys);
    }

    fn note_on(&mut self, agent: &str, text: &str) {
        self.note = Some(Note {
            agent: agent.to_string(),
            text: text.to_string(),
            until: self.now + NOTE_MS,
        });
    }

    fn tick_demo(&mut self, sys: &mut impl Sys) {
        let Some(demo) = &mut self.demo else { return };
        if let Some(snapshot) = demo.snapshot(self.now) {
            self.apply(snapshot, sys);
        }
    }

    // ------------------------------------------------------------ status

    pub(crate) fn dnd(&self) -> bool {
        self.face_down || self.dnd_on() || self.quiet_hours()
    }

    /// Do not disturb switched on by hand (control centre or AI key).
    fn dnd_on(&self) -> bool {
        self.dnd_until.is_some_and(|until| self.now < until)
    }

    /// Inside the quiet hours set in Paseo, by the plugin's clock and zone.
    fn quiet_hours(&self) -> bool {
        let (start, end) = self.quiet;
        let Some(epoch) = self.epoch() else {
            return false;
        };
        if start == end {
            return false;
        }
        let minute = ((epoch / 60_000 + i64::from(self.tz)).rem_euclid(24 * 60)) as u16;
        if start < end {
            (start..end).contains(&minute)
        } else {
            minute >= start || minute < end
        }
    }

    fn board_prefs(&self) -> BoardPrefs {
        BoardPrefs {
            sound: self.prefs.sound,
            haptic: self.prefs.haptic,
            brightness: self.prefs.brightness,
            dnd: self.dnd_on(),
        }
    }

    /// Board settings made in Paseo. A change made on the board that Paseo
    /// has not confirmed yet wins until it has.
    fn apply_settings(&mut self, settings: Settings, sys: &mut impl Sys) {
        if self.settings_rev == Some(settings.rev) || self.session.prefs_pending() {
            return;
        }
        self.settings_rev = Some(settings.rev);
        self.quiet = (settings.quiet_start, settings.quiet_end);
        self.led = settings.led_brightness.min(100);
        if self.prefs.sound != settings.sound {
            self.prefs.sound = settings.sound;
            sys.save_prefs(&self.prefs);
        }
        self.synced = Some(self.board_prefs());
    }

    /// Sends settings changed on the board to Paseo.
    fn sync_prefs(&mut self) {
        if self.demo.is_some() || !self.session.is_configured() {
            return;
        }
        let prefs = self.board_prefs();
        match self.synced {
            None => self.synced = Some(prefs),
            Some(synced) if synced != prefs => {
                self.synced = Some(prefs);
                self.session.set_prefs(prefs);
            }
            Some(_) => {}
        }
    }

    fn feel(&self, haptic: Haptic, sys: &mut impl Sys) {
        if self.prefs.haptic && (!self.dnd() || matches!(haptic, Haptic::Tick | Haptic::Confirm)) {
            sys.haptic(haptic);
        }
    }

    fn ring(&self, chime: Chime, sys: &mut impl Sys) {
        if self.prefs.sound && !self.dnd() {
            sys.chime(chime);
        }
    }

    fn touched(&mut self) {
        self.last_touch = self.now;
    }

    /// Epoch ms now, from the plugin's clock.
    /// The lamp color to show now while it glides to `target`.
    pub(crate) fn lamp_color(&self, target: u32) -> u32 {
        let now = self.now;
        let fade = match self.lamp_fade.get() {
            Some(f) if f.to == target => f,
            Some(f) => Fade {
                from: f.at(now),
                to: target,
                start: now,
            },
            None => Fade {
                from: target,
                to: target,
                start: now,
            },
        };
        self.lamp_fade.set(Some(fade));
        fade.at(now)
    }

    pub(crate) fn epoch(&self) -> Option<i64> {
        self.clock
            .map(|(epoch, at)| epoch + (self.now as i64 - at as i64))
    }

    fn update_overlay(&mut self) {
        match self.overlay {
            Overlay::Pair { .. }
            | Overlay::Scan
            | Overlay::Input { .. }
            | Overlay::Wifi
            | Overlay::NetInfo
            | Overlay::Link => return,
            _ => {}
        }
        if self.demo.is_some() {
            self.overlay = Overlay::None;
            return;
        }
        let no_wifi = self
            .net_down_since
            .is_some_and(|since| self.now >= since + NO_WIFI_PAGE_MS);
        self.overlay = if !self.session.is_configured() {
            Overlay::Welcome
        } else if no_wifi {
            Overlay::NoWifi
        } else if matches!(self.session.down(), Down::PluginVersion(_)) {
            Overlay::NoPaseo
        } else if (self.overlay == Overlay::NoPaseo && !self.session.is_ready())
            || (!self.has_snapshot && self.session.failures() > 0)
        {
            // Opened from the banner or the control centre, or never connected.
            Overlay::NoPaseo
        } else {
            Overlay::None
        };
    }

    /// The banner over stale data when Paseo has been unreachable a while.
    pub(crate) fn banner(&self) -> bool {
        self.demo.is_none()
            && self.overlay == Overlay::None
            && self.has_snapshot
            && !self.banner_dismissed
            && self
                .paseo_down_since
                .is_some_and(|since| self.now >= since + BANNER_MS)
    }

    fn apply_power(&mut self, sys: &mut impl Sys) {
        let mut level = self.prefs.brightness;
        if self.module
            && self.prefs.auto
            && let Some(ambient) = self.ambient
        {
            level = (15 + u16::from(ambient) * 85 / 100) as u8;
        }
        // Burn-in guard: the screen stays dim through the quiet hours.
        if self.quiet_hours() {
            level = level.min(DIM_PERCENT);
        }
        let alert = self.now < self.alert_until;
        let wanted = if self.module {
            let last = self.last_touch.max(self.last_presence);
            if self.presence || alert || self.now < last + OFF_AFTER_ABSENT_MS {
                Display::On(level)
            } else {
                Display::Off
            }
        } else if alert || self.now < self.last_touch + DIM_AFTER_MS {
            Display::On(level)
        } else if self.now < self.last_touch + OFF_AFTER_MS {
            Display::On(level.min(DIM_PERCENT))
        } else {
            Display::Off
        };
        if self.display != Some(wanted) {
            // Waking from dark shows the most urgent card.
            if self.display == Some(Display::Off) && !self.order.is_empty() {
                self.deck = 0;
                self.fill_slots();
                self.update_focus();
            }
            self.display = Some(wanted);
            sys.display(wanted);
        }
    }

    fn apply_glow(&mut self, sys: &mut impl Sys) {
        if !self.module {
            self.led_sent = None;
            return;
        }
        if self.led_sent != Some(self.led) {
            self.led_sent = Some(self.led);
            sys.led_brightness(self.led);
        }
        let count = |s: State| self.snapshot.agents.iter().filter(|a| a.state == s).count();
        let wanted = if self.dnd() || !self.has_snapshot {
            Glow::Off
        } else if count(State::Call) > 0 {
            Glow::Call
        } else if count(State::Fail) > 0 {
            Glow::Fail
        } else if count(State::Work) > 0 {
            Glow::Work
        } else {
            Glow::Off
        };
        if self.glow != Some(wanted) {
            self.glow = Some(wanted);
            sys.glow(wanted);
        }
    }

    /// Why Paseo is unreachable, for the status page and the banner.
    pub(crate) fn down(&self) -> &Down {
        self.session.down()
    }
}

/// Swallows effects (the demo's first snapshot, before any platform exists).
struct NullSys;

impl Sys for NullSys {
    fn link(&mut self, _: Out) {}
    fn haptic(&mut self, _: Haptic) {}
    fn chime(&mut self, _: Chime) {}
    fn glow(&mut self, _: Glow) {}
    fn led_brightness(&mut self, _: u8) {}
    fn display(&mut self, _: Display) {}
    fn save_config(&mut self, _: Option<&Config>) {}
    fn save_prefs(&mut self, _: &Prefs) {}
    fn wifi(&mut self, _: WifiCmd) {}
    fn camera(&mut self, _: bool) {}
}

#[cfg(test)]
mod tests {
    use alloc::vec;

    use super::*;
    use crate::model::VERSION;
    use crate::ui::{Bind, Ease, Key, Property, UiResult};

    /// Accepts every write; `typed` is what the keyboard holds.
    #[derive(Default)]
    struct NullUi {
        typed: String,
    }

    impl Ui for NullUi {
        fn set_text(&mut self, _: Bind, _: &str) -> UiResult {
            Ok(())
        }
        fn set_visible(&mut self, _: Bind, _: bool) -> UiResult {
            Ok(())
        }
        fn set_color(&mut self, _: Bind, _: u32) -> UiResult {
            Ok(())
        }
        fn animate_color(&mut self, _: Bind, _: u32, _: u32, _: u32, _: Ease) -> UiResult {
            Ok(())
        }
        fn set_value(&mut self, _: Key, _: i32) -> UiResult {
            Ok(())
        }
        fn animate_value(&mut self, _: Key, _: i32, _: i32, _: u32, _: Ease) -> UiResult {
            Ok(())
        }
        fn set_i32(&mut self, _: Key, _: Property, _: i32) -> UiResult {
            Ok(())
        }
        fn set_property_color(&mut self, _: Key, _: Property, _: u32) -> UiResult {
            Ok(())
        }
        fn animate_i32(
            &mut self,
            _: Key,
            _: Property,
            _: i32,
            _: i32,
            _: u32,
            _: Ease,
        ) -> UiResult {
            Ok(())
        }
        fn keyboard_attach(&mut self, _: u16, _: Bind) -> UiResult {
            Ok(())
        }
        fn keyboard_text(&mut self) -> Result<String, gsp::esp_gsp_err_t> {
            Ok(self.typed.clone())
        }
        fn set_page(&mut self, _: Key, _: u16, _: bool) -> UiResult {
            Ok(())
        }
        fn set_drawer(&mut self, _: Key, _: bool, _: bool) -> UiResult {
            Ok(())
        }
    }

    /// Records the screen and LED effects; drops the rest.
    #[derive(Default)]
    struct Effects {
        display: Option<Display>,
        glows: usize,
    }

    impl Sys for Effects {
        fn link(&mut self, _: Out) {}
        fn haptic(&mut self, _: Haptic) {}
        fn chime(&mut self, _: Chime) {}
        fn glow(&mut self, _: Glow) {
            self.glows += 1;
        }
        fn led_brightness(&mut self, _: u8) {}
        fn display(&mut self, display: Display) {
            self.display = Some(display);
        }
        fn save_config(&mut self, _: Option<&Config>) {}
        fn save_prefs(&mut self, _: &Prefs) {}
        fn wifi(&mut self, _: WifiCmd) {}
        fn camera(&mut self, _: bool) {}
    }

    const T: i64 = 1_793_583_660_000;
    const MIN: u64 = 60_000;

    fn agent(id: &str, state: State, minutes_ago: i64) -> Agent {
        Agent {
            id: id.into(),
            state,
            since: Some(T - minutes_ago * MIN as i64),
            ..Default::default()
        }
    }

    fn board(agents: Vec<Agent>) -> (App, NullUi, Effects) {
        let (mut app, mut ui, mut sys) =
            (App::new(0, "test"), NullUi::default(), Effects::default());
        let fleet = Snapshot {
            v: VERSION,
            now: T,
            agents,
            ..Default::default()
        };
        app.apply(fleet, &mut sys);
        app.poll(0, &mut ui, &mut sys);
        (app, ui, sys)
    }

    fn at(app: &mut App, ui: &mut NullUi, sys: &mut Effects, ms: u64) -> Option<Display> {
        app.poll(ms, ui, sys);
        sys.display
    }

    #[test]
    fn without_the_module_the_screen_dims_then_sleeps_and_never_glows() {
        let (mut app, mut ui, mut sys) = board(vec![agent("a", State::Work, 1)]);
        assert_eq!(sys.display, Some(Display::On(80)));
        assert_eq!(
            at(&mut app, &mut ui, &mut sys, 2 * MIN),
            Some(Display::On(DIM_PERCENT))
        );
        assert_eq!(
            at(&mut app, &mut ui, &mut sys, 10 * MIN),
            Some(Display::Off)
        );
        app.handle(10 * MIN + 5, Event::Touch, &mut ui, &mut sys);
        assert_eq!(sys.display, Some(Display::On(80)));
        assert_eq!(sys.glows, 0);
    }

    #[test]
    fn with_the_module_the_screen_follows_presence_and_the_leds_glow() {
        let (mut app, mut ui, mut sys) = board(vec![agent("a", State::Work, 1)]);
        app.handle(0, Event::Module(true), &mut ui, &mut sys);
        app.handle(0, Event::Presence(true), &mut ui, &mut sys);
        assert_eq!(
            at(&mut app, &mut ui, &mut sys, 20 * MIN),
            Some(Display::On(80))
        );
        app.handle(20 * MIN, Event::Presence(false), &mut ui, &mut sys);
        assert_eq!(
            at(&mut app, &mut ui, &mut sys, 22 * MIN),
            Some(Display::On(80))
        );
        assert_eq!(
            at(&mut app, &mut ui, &mut sys, 23 * MIN),
            Some(Display::Off)
        );
        assert!(sys.glows > 0);
    }

    #[test]
    fn waking_from_dark_shows_the_most_urgent_card() {
        let (mut app, mut ui, mut sys) = board(vec![
            agent("working", State::Work, 1),
            agent("failed", State::Fail, 2),
        ]);
        app.deck = 1;
        assert_eq!(
            at(&mut app, &mut ui, &mut sys, 10 * MIN),
            Some(Display::Off)
        );
        app.handle(10 * MIN + 5, Event::Touch, &mut ui, &mut sys);
        assert_eq!(app.current_agent().map(|a| a.id.as_str()), Some("failed"));
    }

    #[test]
    fn the_lamp_glides_to_a_new_urgency_color() {
        let app = App::new(0, "test");
        assert_eq!(app.lamp_color(0xFF0000), 0xFF0000);
        let mut app = app;
        app.now = 1_000;
        assert_eq!(app.lamp_color(0x0000FF), 0xFF0000);
        app.now = 1_300;
        let mid = app.lamp_color(0x0000FF);
        assert!(mid != 0xFF0000 && mid != 0x0000FF, "{mid:06x}");
        assert_eq!(app.next_deadline_ms(), 1_300 + FADE_STEP_MS);
        app.now = 1_600;
        assert_eq!(app.lamp_color(0x0000FF), 0x0000FF);
    }

    #[test]
    fn a_new_tool_call_ripples_the_lamp_once() {
        let working = |now: &str| Agent {
            now: now.into(),
            ..agent("a", State::Work, 1)
        };
        let fleet = |now: &str| Snapshot {
            v: VERSION,
            now: T,
            agents: vec![working(now)],
            ..Default::default()
        };
        let (mut app, _, mut sys) = board(vec![working("read  a.rs")]);
        app.apply(fleet("read  a.rs"), &mut sys);
        assert!(!app.ripple);
        app.apply(fleet("edit  a.rs"), &mut sys);
        assert!(app.ripple);
    }

    fn press(app: &mut App, ui: &mut NullUi, sys: &mut Effects, action: u32) {
        app.handle(1, Event::Call { action, arg: 0 }, ui, sys);
    }

    #[test]
    fn typing_an_address_and_password_leads_to_the_confirm_page() {
        let (mut app, mut ui, mut sys) =
            (App::new(0, "test"), NullUi::default(), Effects::default());
        press(&mut app, &mut ui, &mut sys, gsp::GSP_ACT_ID_MANUAL_OPEN);
        ui.typed = "ftp://nope".into();
        press(&mut app, &mut ui, &mut sys, gsp::GSP_ACT_ID_KB_KEY);
        assert!(matches!(
            &app.overlay,
            Overlay::Input {
                entry: Entry::Address,
                error: Some(_),
                ..
            }
        ));
        ui.typed = "paseo.example.net".into();
        press(&mut app, &mut ui, &mut sys, gsp::GSP_ACT_ID_KB_KEY);
        ui.typed = "secret".into();
        press(&mut app, &mut ui, &mut sys, gsp::GSP_ACT_ID_KB_KEY);
        let Overlay::Pair { config, host } = &app.overlay else {
            panic!("{:?}", app.overlay)
        };
        assert_eq!(
            (config.url.as_str(), config.password.as_str()),
            ("wss://paseo.example.net/ws", "secret")
        );
        assert_eq!(host, "paseo.example.net");
    }

    #[test]
    fn a_wrong_wifi_password_keeps_the_password_page() {
        let (mut app, mut ui, mut sys) =
            (App::new(0, "test"), NullUi::default(), Effects::default());
        app.handle(
            0,
            Event::Net(Net {
                state: NetState::NoSaved,
                ..Default::default()
            }),
            &mut ui,
            &mut sys,
        );
        press(&mut app, &mut ui, &mut sys, gsp::GSP_ACT_ID_WIFI_PICK);
        let ap = Ap {
            ssid: "home".into(),
            rssi: -50,
            secure: true,
            saved: false,
        };
        app.handle(1, Event::Networks(vec![ap]), &mut ui, &mut sys);
        press(&mut app, &mut ui, &mut sys, gsp::GSP_ACT_ID_WF_ROW0);
        ui.typed = "hunter2".into();
        press(&mut app, &mut ui, &mut sys, gsp::GSP_ACT_ID_KB_KEY);
        assert!(matches!(&app.overlay, Overlay::Input { joining: true, .. }));
        let wrong = Net {
            state: NetState::WrongPassword,
            ssid: "home".into(),
            ..Default::default()
        };
        app.handle(2, Event::Net(wrong), &mut ui, &mut sys);
        assert!(matches!(
            &app.overlay,
            Overlay::Input {
                joining: false,
                error: Some(_),
                ..
            }
        ));
        let online = Net {
            state: NetState::Online,
            ssid: "home".into(),
            ..Default::default()
        };
        press(&mut app, &mut ui, &mut sys, gsp::GSP_ACT_ID_KB_KEY);
        app.handle(3, Event::Net(online), &mut ui, &mut sys);
        assert!(!matches!(&app.overlay, Overlay::Input { .. }));
    }

    #[test]
    fn settings_flow_both_ways_and_a_local_change_wins_until_confirmed() {
        let (mut app, mut ui, mut sys) = board(vec![]);
        app.session.configure(
            Some(Config {
                url: "wss://paseo.example.net/ws".into(),
                password: "pw".into(),
            }),
            0,
        );
        let with = |rev, sound| Snapshot {
            v: VERSION,
            now: T,
            settings: Some(Settings {
                rev,
                sound,
                led_brightness: 40,
                quiet_start: 0,
                quiet_end: 0,
            }),
            ..Default::default()
        };
        app.poll(1, &mut ui, &mut sys);
        app.apply(with(1, false), &mut sys);
        assert!(!app.prefs.sound);
        assert!(!app.session.prefs_pending());
        // Turned back on at the board: it goes out, and Paseo's older value does not undo it.
        app.handle(
            2,
            Event::Call {
                action: gsp::GSP_ACT_ID_CC_SOUND,
                arg: 0,
            },
            &mut ui,
            &mut sys,
        );
        assert!(app.prefs.sound && app.session.prefs_pending());
        app.apply(with(2, false), &mut sys);
        assert!(app.prefs.sound);
    }

    #[test]
    fn quiet_hours_dim_the_screen() {
        let (mut app, mut ui, mut sys) = board(vec![]);
        let quiet = Snapshot {
            v: VERSION,
            now: T,
            tz: 480,
            settings: Some(Settings {
                rev: 1,
                sound: true,
                led_brightness: 60,
                quiet_start: 540,
                quiet_end: 600,
            }),
            ..Default::default()
        };
        app.apply(quiet, &mut sys);
        app.poll(1, &mut ui, &mut sys);
        assert_eq!(sys.display, Some(Display::On(DIM_PERCENT)));
    }

    #[test]
    fn quiet_hours_silence_the_board() {
        let (mut app, _, mut sys) = board(vec![]);
        // T is 09:41 in UTC+8; quiet 09:00..10:00.
        let quiet = Snapshot {
            v: VERSION,
            now: T,
            tz: 480,
            settings: Some(Settings {
                rev: 1,
                sound: true,
                led_brightness: 60,
                quiet_start: 540,
                quiet_end: 600,
            }),
            ..Default::default()
        };
        app.apply(quiet, &mut sys);
        assert!(app.dnd());
    }

    #[test]
    fn read_cards_leave_the_deck_after_an_hour() {
        let (mut app, mut ui, mut sys) = board(vec![
            agent("read", State::Seen, 50),
            agent("working", State::Work, 1),
        ]);
        assert_eq!(app.order.len(), 2);
        app.poll(9 * MIN, &mut ui, &mut sys);
        assert_eq!(app.order.len(), 2);
        app.poll(11 * MIN, &mut ui, &mut sys);
        assert_eq!(app.order.len(), 1);
        assert_eq!(app.current_agent().map(|a| a.id.as_str()), Some("working"));
    }
}
