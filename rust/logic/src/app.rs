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
use crate::sys::{
    Ap, Button, Chime, Display, Event, Glow, Haptic, Link, Motion, Net, NetState, Power, Prefs,
    Sys, WifiCmd,
};

mod compose;
mod setup;

use crate::lang::Lang;
use crate::ui::{Painter, Ui};
use compose::Compose;
pub(crate) use compose::Voice;

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
/// How often the overlays' visibility is sent again (see `view::OVERLAY_VIS`).
pub(crate) const OVERLAY_REASSERT_MS: u64 = 1_000;
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
/// "Mark read": the button shows its check this long, then the card leaves.
const ACK_MS: u64 = 280;
/// The card is gone by this long after it started leaving, even when no page
/// settles (the deck was not in front, or an event was missed).
const LEAVE_MS: u64 = 800;
/// How long the line saying what marking read did stays.
const READ_NOTE_MS: u64 = 3_500;
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
    /// The two ways to connect, as Paseo's 添加连接: scan its pairing QR
    /// (relay) or type a direct connection.
    Connect,
    /// A scanned pairing code or typed connection waits for "连接".
    Pair {
        config: Config,
    },
    Scan,
    /// Direct connection, step 3 of 4: SSL or not.
    Tls {
        host: String,
        port: u16,
    },
    /// A new agent session: talk or type, then send.
    Compose,
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
    /// Direct connection, step 1 of 4: the host.
    Host,
    /// Step 2 of 4: the port; an empty field takes `default` (6767, or the
    /// port typed with the host).
    Port { host: String, default: u16 },
    /// Step 4 of 4: the daemon password, optional.
    Password { host: String, port: u16, tls: bool },
    /// The daemon password, which Paseo asked for while connecting.
    DaemonPassword { config: Config },
    /// The first prompt of a new session (the keyboard has Latin letters only).
    Prompt,
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
    /// The turn on screen when the hold began: only that turn is stopped.
    pub turn: Option<String>,
    pub at: u64,
    pub shown: bool,
}

#[derive(Clone, Debug)]
struct Pending {
    agent: String,
    kind: ActKind,
}

/// A card marked read on the board, on its way out of the deck.
#[derive(Clone, Debug)]
pub(crate) struct Leaving {
    pub agent: String,
    /// When the button was tapped (monotonic ms).
    at: u64,
    /// The deck (or the screen) was asked to move on.
    moved: bool,
}

/// A transient line on one agent's card (an empty `agent`: the scan page or,
/// after the last card was read, the home page).
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
    /// epoch ms of the mark). They stay out of the deck until `since` changes.
    seen: BTreeMap<String, (Option<i64>, i64)>,
    pub(crate) leaving: Option<Leaving>,
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
    /// The input page needs the keyboard (again) at the next render: a wrong
    /// Wi-Fi password, or a daemon password Paseo asked for.
    keyboard_again: bool,
    /// A connection made on this board has not reached Paseo yet: a refused
    /// password asks for it.
    pairing: bool,
    /// The new-session page.
    pub(crate) compose: Compose,
    /// An agent started here: its card shows once a snapshot carries it.
    show_next: Option<String>,
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
    /// When the overlays' visibility is next sent again.
    overlay_reassert_at: u64,
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
            leaving: None,
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
            pairing: false,
            compose: Compose::default(),
            show_next: None,
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
            overlay_reassert_at: 0,
            wifi_phase: 0,
            last_second: now_ms / 1000,
        };
        // The scene starts with its Chinese texts.
        for (bind, zh, _) in crate::scene::TEXTS {
            app.painter.assume_text(bind, zh);
        }
        app.update_overlay();
        app
    }

    /// Runs on built-in sample agents instead of Paseo (the simulator, and
    /// boards without a daemon). Actions on the cards change the samples.
    /// `offset_ms` starts the script that far in.
    pub fn start_demo(&mut self, offset_ms: u64) {
        let epoch = self.epoch().unwrap_or(crate::demo::EPOCH_MS);
        self.session.configure(None, self.now);
        self.demo = Some(Demo::new(self.now, epoch, offset_ms, self.lang()));
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
        self.leaving = None;
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
                Link::Opened => {
                    // Fresh keys for the relay's encrypted channel.
                    let mut entropy = [0u8; 64];
                    sys.random(&mut entropy);
                    self.session.on_open(self.now, &entropy);
                }
                Link::Closed(why) => self.session.on_close(self.now, why),
                Link::Text(text) => self.session.on_text(self.now, &text),
            },
            Event::Audio(pcm) => self.audio(&pcm),
            Event::AudioEnd => self.audio_end(),
            Event::Loudness(level) => self.loudness(level),
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
                    let text = self.lang().pick(
                        "没有检测到摄像头模块：请改用直接连接",
                        "No camera module: use a direct connection",
                    );
                    self.note_on("", text);
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
                let (agent, turn) = self
                    .hold
                    .take()
                    .map(|h| (h.agent, h.turn))
                    .unwrap_or_default();
                // Marked before sending: the demo answers within send().
                self.stopped_here.insert(agent.clone());
                if self.send(&agent, ActKind::Stop, None, None, turn, sys) {
                    self.feel(Haptic::Confirm, sys);
                } else {
                    self.stopped_here.remove(&agent);
                }
            }
        }
        if self.note.as_ref().is_some_and(|n| now_ms >= n.until) {
            self.note = None;
        }
        self.advance_leaving(ui);
        if self.net.state == NetState::Connecting {
            self.wifi_phase = ((now_ms / 400) % 4) as u8;
        }
        if now_ms / ORDER_REFRESH_MS != self.last_second * 1000 / ORDER_REFRESH_MS {
            if self.has_snapshot {
                let current = self.current_agent().map(|a| a.id.clone());
                self.refresh_order(current);
            }
            if self.display != Some(Display::Off) {
                self.drift(ui);
            }
        }
        self.poll_compose(sys);
        if self.overlay == Overlay::Scan && now_ms >= self.scan_since + SCAN_HINT_MS {
            self.scan_since = now_ms;
            let text = self
                .lang()
                .pick("认不出来：换个角度试试", "Can't read it: try another angle");
            self.note_on("", text);
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
        next = next.min(self.compose_deadline());
        if self.render_retry {
            next = next.min(self.now + RENDER_RETRY_MS);
        }
        if self.lamp_fade.get().is_some_and(|f| f.active(self.now)) {
            next = next.min(self.now + FADE_STEP_MS);
        }
        if let Some(lv) = &self.leaving {
            next = next.min(lv.at + ACK_MS + if lv.moved { LEAVE_MS } else { 0 });
        }
        if let Some(note) = &self.note {
            next = next.min(note.until);
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
        let pulse = core::mem::take(&mut self.pulse_start);
        let ripple = core::mem::take(&mut self.ripple);
        // A dark screen has its frames stopped (app_display pauses GSP): no scene
        // writes until it wakes; the painter's cache still describes the panel.
        if self.display == Some(Display::Off) {
            self.render_retry = false;
            return;
        }
        if pulse {
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
        self.show_started(ui);
        if self.now >= self.overlay_reassert_at {
            self.painter.forget_visible(&crate::view::OVERLAY_VIS);
            self.overlay_reassert_at = self.now + OVERLAY_REASSERT_MS;
        }
        if ripple {
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
                    self.pairing = false;
                    self.paseo_down_since = None;
                    self.banner_dismissed = false;
                    if self.overlay == Overlay::NoPaseo {
                        self.overlay = Overlay::None;
                    }
                }
                Notice::Snapshot(snapshot) => self.apply(snapshot, sys),
                Notice::Acted { token, result } => {
                    if !self.started(token, &result, sys) {
                        self.acted(token, &result, sys);
                    }
                }
                Notice::Heard(heard) => self.heard(heard, sys),
                Notice::Down(down) => {
                    if matches!(down, Down::Auth | Down::NeedPassword) {
                        self.password_needed(&down);
                    }
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
        if self.setup_call(action, ui, sys) || self.compose_call(action, ui, sys) {
            return;
        }
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
            GSP_ACT_ID_LAMP_LONG => {
                self.open_compose();
                self.feel(Haptic::Tick, sys);
            }
            GSP_ACT_ID_HOLD_PRESS => self.hold_pressed(sys),
            GSP_ACT_ID_HOLD_RELEASE | GSP_ACT_ID_HOLD_CANCEL => {
                if self.hold.take().is_some_and(|h| h.shown) {
                    self.painter.run(ui.set_value(GSP_OBJ_KEY_HOLD_RING, 0));
                }
            }
            GSP_ACT_ID_SEEN => self.mark_seen(sys),
            GSP_ACT_ID_CC_WIFI => {
                self.painter
                    .run(ui.set_drawer(gsp::GSP_OBJ_KEY_CC, false, true));
                self.overlay = Overlay::NetInfo;
                self.feel(Haptic::Tick, sys);
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
            if self.leaving.as_ref().is_some_and(|lv| lv.moved) {
                self.finish_leaving();
            }
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
                if self.leaving.as_ref().is_some_and(|lv| lv.moved) {
                    self.finish_leaving();
                } else {
                    self.feel(Haptic::Tick, sys);
                }
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
            // Hold to talk: a new session by voice (the page explains when it can't).
            Button::AiLong => self.talk(sys),
            Button::AiRelease => self.talked(sys),
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

    // ------------------------------------------------------------ data

    fn apply(&mut self, mut snapshot: Snapshot, sys: &mut impl Sys) {
        if snapshot.v != crate::model::VERSION {
            return;
        }
        // The board speaks the Paseo app's language (kept for the pages
        // shown before Paseo answers).
        if let Some(lang) = snapshot.lang.as_deref().and_then(Lang::parse)
            && lang != self.prefs.lang
        {
            self.prefs.lang = lang;
            sys.save_prefs(&self.prefs);
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
        // Read on the board: out of the deck (the card on its way out stays
        // until it has left).
        let leaving = self.leaving.as_ref().map(|lv| lv.agent.as_str());
        snapshot
            .agents
            .retain(|a| !self.seen.contains_key(&a.id) || leaving == Some(a.id.as_str()));
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
            let text = self
                .lang()
                .pick("只读模式：在 Paseo 里回答", "Read-only: answer in Paseo");
            self.note_on(&id, text);
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
            None,
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
        let (id, turn) = (agent.id.clone(), agent.turn.clone());
        if !agent.stoppable {
            return;
        }
        if self.read_only() {
            let text = self
                .lang()
                .pick("只读模式：在 Paseo 里停止", "Read-only: stop it in Paseo");
            self.note_on(&id, text);
            return;
        }
        self.hold = Some(Hold {
            agent: id,
            turn,
            at: self.now,
            shown: false,
        });
        let _ = sys;
    }

    /// "Mark read": the button acknowledges at once, the card leaves the
    /// deck a beat later (`advance_leaving`), and a note says what is left.
    fn mark_seen(&mut self, sys: &mut impl Sys) {
        if self.leaving.is_some() {
            return;
        }
        let Some(agent) = self.current_agent() else {
            return;
        };
        if !matches!(agent.state, State::Done | State::Fail) {
            return;
        }
        let (id, since) = (agent.id.clone(), agent.since);
        let read_at = self.epoch().unwrap_or_default();
        self.seen.insert(id.clone(), (since, read_at));
        self.send(&id, ActKind::Seen, None, None, None, sys);
        self.feel(Haptic::Confirm, sys);
        self.press = None;
        self.hold = None;
        self.leaving = Some(Leaving {
            agent: id,
            at: self.now,
            moved: false,
        });
    }

    /// After the acknowledgement: the deck slides to the next card, or, when
    /// this was the last one, the board goes back to the lamp.
    fn advance_leaving(&mut self, ui: &mut impl Ui) {
        let Some((at, moved)) = self.leaving.as_ref().map(|lv| (lv.at, lv.moved)) else {
            return;
        };
        if !moved && self.now >= at + ACK_MS {
            let on_deck = self.screen == DECK
                && self
                    .current_agent()
                    .zip(self.leaving.as_ref())
                    .is_some_and(|(a, lv)| a.id == lv.agent);
            if let Some(lv) = &mut self.leaving {
                lv.moved = true;
            }
            if !on_deck {
                self.finish_leaving();
            } else if self.order.len() > 1 {
                let page = (self.slot + 1) % SLOTS as u16;
                self.painter
                    .run(ui.set_page(gsp::GSP_OBJ_KEY_CARDS, page, true));
            } else {
                self.painter.page(ui, gsp::GSP_OBJ_KEY_SCREENS, HOME, true);
            }
            return;
        }
        if moved && self.now >= at + ACK_MS + LEAVE_MS {
            self.finish_leaving();
        }
    }

    /// Takes the read card out of the deck and says what is left.
    fn finish_leaving(&mut self) {
        let Some(lv) = self.leaving.take() else {
            return;
        };
        let current = self
            .current_agent()
            .map(|a| a.id.clone())
            .filter(|id| *id != lv.agent);
        self.snapshot.agents.retain(|a| a.id != lv.agent);
        self.refresh_order(current);
        let l = self.lang();
        let left = self.order.len();
        let (agent, text) = match self.current_agent() {
            Some(a) => (
                a.id.clone(),
                crate::tr!(l, "已读 · 还剩 {} 张", "Marked read · {} left", left),
            ),
            None => (
                String::new(),
                String::from(l.pick("都看完了", "All caught up")),
            ),
        };
        self.note = Some(Note {
            agent,
            text,
            until: self.now + READ_NOTE_MS,
        });
    }

    /// Sends an action; false (with a note) when it could not go out.
    fn send(
        &mut self,
        agent: &str,
        kind: ActKind,
        request_id: Option<String>,
        choice: Option<u8>,
        turn: Option<String>,
        sys: &mut impl Sys,
    ) -> bool {
        let act = Act {
            agent: agent.to_string(),
            device: alloc::format!("farol-{}", self.device),
            kind,
            request_id,
            choice,
            turn,
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
            let text = self.lang().pick(
                "未连接 Paseo，没有发出去",
                "Not connected to Paseo; nothing sent",
            );
            self.note_on(agent, text);
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
                let text = self
                    .lang()
                    .pick("已请求停止这一轮", "Stop requested for this turn");
                self.note_on(&pending.agent, text);
            }
            return;
        }
        self.sent.remove(&pending.agent);
        self.stopped_here.remove(&pending.agent);
        if pending.kind == ActKind::Seen {
            return;
        }
        let l = self.lang();
        let text = match result.reason.as_str() {
            "already_resolved" if pending.kind == ActKind::Stop => {
                String::from(l.pick("这一轮已经结束了", "This turn already ended"))
            }
            "already_resolved" => String::from(l.pick("已在别处回答", "Answered elsewhere")),
            "read_only" => {
                String::from(l.pick("只读模式：在 Paseo 里操作", "Read-only: act in Paseo"))
            }
            "disconnected" => {
                String::from(l.pick("连接断了，没有发出去", "Disconnected; nothing sent"))
            }
            "not_found" => String::from(l.pick("这个 agent 已经不在了", "This agent is gone")),
            _ if result.message.is_empty() => String::from(l.pick("没有发出去", "Not sent")),
            _ => crate::tr!(l, "没有发出去：{}", "Not sent: {}", result.message),
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
            Overlay::Connect
            | Overlay::Compose
            | Overlay::Pair { .. }
            | Overlay::Tls { .. }
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

    /// The board's language.
    pub(crate) fn lang(&self) -> Lang {
        self.prefs.lang
    }

    /// Sets the language before anything ran (the simulator's FAROL_SIM_LANG).
    pub fn set_lang(&mut self, lang: Lang) {
        self.prefs.lang = lang;
    }

    /// The card of an agent started here, once a snapshot carries it.
    fn show_started(&mut self, ui: &mut impl Ui) {
        let Some(id) = &self.show_next else {
            return;
        };
        let Some(at) = self
            .order
            .iter()
            .position(|&i| self.snapshot.agents.get(i).is_some_and(|a| &a.id == id))
        else {
            return;
        };
        self.show_next = None;
        self.deck = at;
        self.fill_slots();
        self.painter.page(ui, gsp::GSP_OBJ_KEY_SCREENS, DECK, true);
        self.screen = DECK;
        self.update_focus();
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
    fn random(&mut self, out: &mut [u8]) {
        out.fill(0);
    }
    fn listen(&mut self, _: bool) {}
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
        /// Scene writes made so far.
        writes: usize,
        /// The last visibility written to each bind.
        shown: alloc::collections::BTreeMap<Bind, bool>,
    }

    impl Ui for NullUi {
        fn set_text(&mut self, _: Bind, _: &str) -> UiResult {
            self.writes += 1;
            Ok(())
        }
        fn set_visible(&mut self, bind: Bind, visible: bool) -> UiResult {
            self.writes += 1;
            self.shown.insert(bind, visible);
            Ok(())
        }
        fn set_color(&mut self, _: Bind, _: u32) -> UiResult {
            self.writes += 1;
            Ok(())
        }
        fn animate_color(&mut self, _: Bind, _: u32, _: u32, _: u32, _: Ease) -> UiResult {
            self.writes += 1;
            Ok(())
        }
        fn set_value(&mut self, _: Key, _: i32) -> UiResult {
            self.writes += 1;
            Ok(())
        }
        fn animate_value(&mut self, _: Key, _: i32, _: i32, _: u32, _: Ease) -> UiResult {
            self.writes += 1;
            Ok(())
        }
        fn set_i32(&mut self, _: Key, _: Property, _: i32) -> UiResult {
            self.writes += 1;
            Ok(())
        }
        fn set_property_color(&mut self, _: Key, _: Property, _: u32) -> UiResult {
            self.writes += 1;
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
            self.writes += 1;
            Ok(())
        }
        fn keyboard_attach(&mut self, _: u16, _: Bind) -> UiResult {
            self.writes += 1;
            Ok(())
        }
        fn keyboard_text(&mut self) -> Result<String, gsp::esp_gsp_err_t> {
            Ok(self.typed.clone())
        }
        fn set_page(&mut self, _: Key, _: u16, _: bool) -> UiResult {
            self.writes += 1;
            Ok(())
        }
        fn set_drawer(&mut self, _: Key, _: bool, _: bool) -> UiResult {
            self.writes += 1;
            Ok(())
        }
    }

    /// Records the screen and LED effects; drops the rest.
    #[derive(Default)]
    struct Effects {
        display: Option<Display>,
        glows: usize,
        saved: Option<Config>,
        listening: bool,
        sent: Vec<String>,
    }

    impl Sys for Effects {
        fn link(&mut self, out: Out) {
            if let Out::Send(text) = out {
                self.sent.push(text);
            }
        }
        fn haptic(&mut self, _: Haptic) {}
        fn chime(&mut self, _: Chime) {}
        fn glow(&mut self, _: Glow) {
            self.glows += 1;
        }
        fn led_brightness(&mut self, _: u8) {}
        fn display(&mut self, display: Display) {
            self.display = Some(display);
        }
        fn save_config(&mut self, config: Option<&Config>) {
            self.saved = config.cloned();
        }
        fn save_prefs(&mut self, _: &Prefs) {}
        fn wifi(&mut self, _: WifiCmd) {}
        fn camera(&mut self, _: bool) {}
        fn random(&mut self, out: &mut [u8]) {
            out.fill(7);
        }
        fn listen(&mut self, on: bool) {
            self.listening = on;
        }
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
    fn a_dark_screen_gets_no_scene_writes_until_it_wakes() {
        let (mut app, mut ui, mut sys) = board(vec![agent("a", State::Work, 1)]);
        assert_eq!(
            at(&mut app, &mut ui, &mut sys, 10 * MIN),
            Some(Display::Off)
        );
        let before = ui.writes;
        app.poll(12 * MIN, &mut ui, &mut sys);
        assert_eq!(ui.writes, before);
        app.handle(12 * MIN + 5, Event::Touch, &mut ui, &mut sys);
        assert!(ui.writes > before);
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

    fn direct(password: &str) -> Config {
        Config {
            endpoint: crate::endpoint::Endpoint::Direct {
                host: "192.168.1.20".into(),
                port: 6767,
                tls: false,
            },
            password: password.into(),
        }
    }

    fn type_in(app: &mut App, ui: &mut NullUi, sys: &mut Effects, text: &str) {
        ui.typed = text.into();
        press(app, ui, sys, gsp::GSP_ACT_ID_KB_KEY);
    }

    #[test]
    fn a_direct_connection_takes_host_port_ssl_and_password() {
        let (mut app, mut ui, mut sys) =
            (App::new(0, "test"), NullUi::default(), Effects::default());
        press(&mut app, &mut ui, &mut sys, gsp::GSP_ACT_ID_DIRECT_OPEN);
        type_in(&mut app, &mut ui, &mut sys, "  ");
        assert!(matches!(
            &app.overlay,
            Overlay::Input {
                entry: Entry::Host,
                error: Some(_),
                ..
            }
        ));
        type_in(&mut app, &mut ui, &mut sys, "192.168.1.20");
        assert!(matches!(
            &app.overlay,
            Overlay::Input {
                entry: Entry::Port { .. },
                ..
            }
        ));
        type_in(&mut app, &mut ui, &mut sys, "99999");
        assert!(matches!(
            &app.overlay,
            Overlay::Input { error: Some(_), .. }
        ));
        type_in(&mut app, &mut ui, &mut sys, "");
        assert_eq!(
            app.overlay,
            Overlay::Tls {
                host: "192.168.1.20".into(),
                port: 6767
            }
        );
        press(&mut app, &mut ui, &mut sys, gsp::GSP_ACT_ID_TLS_OFF);
        type_in(&mut app, &mut ui, &mut sys, "secret");
        assert_eq!(
            app.overlay,
            Overlay::Pair {
                config: direct("secret")
            }
        );
        press(&mut app, &mut ui, &mut sys, gsp::GSP_ACT_ID_PAIR_CONNECT);
        assert_eq!(sys.saved, Some(direct("secret")));
        assert_eq!(app.session.config(), Some(&direct("secret")));
    }

    #[test]
    fn a_lost_overlay_write_heals_within_a_second() {
        let (mut app, mut ui, mut sys) =
            (App::new(0, "test"), NullUi::default(), Effects::default());
        // Not configured yet: the welcome page is up.
        app.poll(100, &mut ui, &mut sys);
        let status = crate::view::OVERLAY_VIS[0];
        assert_eq!(ui.shown.get(&status), Some(&true));
        // The page goes away, but GSP drops the write that hides it while the
        // painter believes it went out.
        app.start_demo(0);
        app.poll(200, &mut ui, &mut sys);
        assert_eq!(app.overlay, Overlay::None);
        assert_eq!(ui.shown.get(&status), Some(&false));
        ui.shown.insert(status, true);
        app.poll(300, &mut ui, &mut sys);
        assert_eq!(ui.shown.get(&status), Some(&true), "cached, not re-sent yet");
        app.poll(100 + OVERLAY_REASSERT_MS + 50, &mut ui, &mut sys);
        assert_eq!(ui.shown.get(&status), Some(&false));
    }

    /// The order a real board saw on 2026-11-02: it restores its config before
    /// Wi-Fi is up, fails DNS three times, joins Wi-Fi, connects and is answered.
    #[test]
    fn a_board_that_boots_before_its_wifi_reaches_the_lamp_once_paseo_answers() {
        use crate::paseo::Down;
        use crate::sys::{Net, NetState};
        let (mut app, mut ui, mut sys) =
            (App::new(0, "test"), NullUi::default(), Effects::default());
        let net = |state| {
            Event::Net(Net {
                state,
                ssid: "home".into(),
                ..Default::default()
            })
        };
        app.handle(3870, net(NetState::Connecting), &mut ui, &mut sys);
        let restored = Event::Restored {
            config: Some(direct("pw")),
            prefs: Prefs::default(),
        };
        app.handle(5864, restored, &mut ui, &mut sys);
        for (closed, next) in [(5895, 6955), (7014, 9075), (9141, 10_000)] {
            app.handle(closed, Event::Link(Link::Closed(Down::Dns)), &mut ui, &mut sys);
            app.poll(next, &mut ui, &mut sys);
        }
        assert_eq!(app.overlay, Overlay::NoPaseo);
        app.handle(10_292, net(NetState::Online), &mut ui, &mut sys);
        app.handle(10_340, Event::Link(Link::Opened), &mut ui, &mut sys);
        let info = r#"{"type":"session","message":{"type":"status","payload":{"status":"server_info","hostname":"studio"}}}"#;
        app.handle(10_380, Event::Link(Link::Text(info.into())), &mut ui, &mut sys);
        app.poll(10_400, &mut ui, &mut sys);
        let poll = sys
            .sent
            .iter()
            .rev()
            .find(|t| t.contains("plugin.rpc.invoke.request"))
            .expect("a poll went out");
        let poll: serde_json::Value = serde_json::from_str(poll).unwrap();
        let id = poll["message"]["requestId"]
            .as_str()
            .or_else(|| poll["message"]["payload"]["requestId"].as_str())
            .expect("the poll has a request id")
            .to_string();
        let snapshot: serde_json::Value =
            serde_json::from_str(include_str!("../../../protocol/fixtures/snapshot.json")).unwrap();
        let answer = serde_json::json!({"type": "session", "message": {
            "type": "plugin.rpc.invoke.response",
            "payload": {"requestId": id, "output": snapshot}}});
        app.handle(10_500, Event::Link(Link::Text(answer.to_string())), &mut ui, &mut sys);
        app.poll(10_600, &mut ui, &mut sys);
        assert!(app.session.is_ready(), "down = {:?}", app.down());
        assert_eq!(app.down(), &Down::None);
        assert!(app.has_snapshot);
        assert_eq!(app.overlay, Overlay::None);
    }

    #[test]
    fn a_link_that_comes_up_after_dns_failures_leaves_the_no_paseo_page() {
        let (mut app, mut ui, mut sys) =
            (App::new(0, "test"), NullUi::default(), Effects::default());
        app.overlay = Overlay::Pair { config: direct("pw") };
        press(&mut app, &mut ui, &mut sys, gsp::GSP_ACT_ID_PAIR_CONNECT);
        let mut t = 10;
        for _ in 0..3 {
            app.handle(t, Event::Link(Link::Closed(crate::paseo::Down::Dns)), &mut ui, &mut sys);
            t += 5_000;
            app.poll(t, &mut ui, &mut sys);
            t += 10;
        }
        assert_eq!(app.overlay, Overlay::NoPaseo);
        app.handle(t, Event::Link(Link::Opened), &mut ui, &mut sys);
        let info = r#"{"type":"session","message":{"type":"status","payload":{"status":"server_info","hostname":"studio"}}}"#;
        app.handle(t + 1, Event::Link(Link::Text(info.into())), &mut ui, &mut sys);
        app.poll(t + 2, &mut ui, &mut sys);
        assert!(app.session.is_ready(), "down = {:?}", app.down());
        assert_eq!(app.down(), &crate::paseo::Down::None);
        assert_eq!(app.overlay, Overlay::None);
    }

    #[test]
    fn a_password_paseo_asks_for_while_connecting_is_asked_on_the_board() {
        let (mut app, mut ui, mut sys) =
            (App::new(0, "test"), NullUi::default(), Effects::default());
        app.overlay = Overlay::Pair { config: direct("") };
        press(&mut app, &mut ui, &mut sys, gsp::GSP_ACT_ID_PAIR_CONNECT);
        let rejected = |reason: &str| {
            Event::Link(Link::Text(format!(
                r#"{{"type":"hello.rejected","reason":"{reason}","accepts":["password"]}}"#
            )))
        };
        app.handle(2, Event::Link(Link::Opened), &mut ui, &mut sys);
        app.handle(3, rejected("password_required"), &mut ui, &mut sys);
        assert!(matches!(
            &app.overlay,
            Overlay::Input {
                entry: Entry::DaemonPassword { .. },
                error: None,
                ..
            }
        ));
        type_in(&mut app, &mut ui, &mut sys, "wrong");
        assert_eq!(sys.saved, Some(direct("wrong")));
        app.handle(4, Event::Link(Link::Opened), &mut ui, &mut sys);
        app.handle(5, rejected("incorrect_password"), &mut ui, &mut sys);
        assert!(matches!(
            &app.overlay,
            Overlay::Input {
                entry: Entry::DaemonPassword { .. },
                error: Some(_),
                ..
            }
        ));
        type_in(&mut app, &mut ui, &mut sys, "pw");
        app.handle(6, Event::Link(Link::Opened), &mut ui, &mut sys);
        let info = r#"{"type":"session","message":{"type":"status","payload":{"status":"server_info","hostname":"studio"}}}"#;
        app.handle(7, Event::Link(Link::Text(info.into())), &mut ui, &mut sys);
        assert!(app.session.is_ready());
        assert!(!app.pairing);
        // Later refusals (a changed password) show on the status page instead.
        app.overlay = Overlay::None;
        app.session.on_close(8, crate::paseo::Down::Closed(1006));
        app.handle(9, Event::Link(Link::Opened), &mut ui, &mut sys);
        app.handle(10, rejected("incorrect_password"), &mut ui, &mut sys);
        assert!(!matches!(&app.overlay, Overlay::Input { .. }));
    }

    #[test]
    fn a_session_started_by_voice_opens_its_card() {
        let (mut app, mut ui, mut sys) =
            (App::new(0, "test"), NullUi::default(), Effects::default());
        app.start_demo(0);
        app.handle(1, Event::Button(Button::AiLong), &mut ui, &mut sys);
        assert_eq!(app.overlay, Overlay::Compose);
        assert_eq!(app.compose.voice, Voice::Listening);
        app.handle(2, Event::Button(Button::AiRelease), &mut ui, &mut sys);
        assert_eq!(app.compose.voice, Voice::Transcribing);
        app.poll(1_000, &mut ui, &mut sys);
        assert_eq!(app.compose.voice, Voice::Idle);
        assert!(!app.compose.text.is_empty());
        // A tap on the bar sends nothing; holding it through GSP's long press does.
        press(&mut app, &mut ui, &mut sys, gsp::GSP_ACT_ID_SEND_PRESS);
        press(&mut app, &mut ui, &mut sys, gsp::GSP_ACT_ID_SEND_RELEASE);
        assert_eq!(app.overlay, Overlay::Compose);
        press(&mut app, &mut ui, &mut sys, gsp::GSP_ACT_ID_SEND_PRESS);
        press(&mut app, &mut ui, &mut sys, gsp::GSP_ACT_ID_SEND_LONG);
        assert_eq!(app.overlay, Overlay::None);
        assert_eq!(app.screen, DECK);
        assert_eq!(app.current_agent().map(|a| a.id.as_str()), Some("new-1"));
    }

    #[test]
    fn without_paseo_the_ai_key_explains_instead_of_recording() {
        let (mut app, mut ui, mut sys) =
            (App::new(0, "test"), NullUi::default(), Effects::default());
        app.handle(1, Event::Button(Button::AiLong), &mut ui, &mut sys);
        assert_eq!(app.overlay, Overlay::Compose);
        assert_eq!(app.compose.voice, Voice::Idle);
        assert!(app.compose_blocked().is_some());
        assert!(!sys.listening);
    }

    #[test]
    fn the_board_speaks_paseos_language_and_keeps_it() {
        let (mut app, _, mut sys) = board(vec![]);
        let with = |lang: Option<&str>| Snapshot {
            v: VERSION,
            now: T,
            lang: lang.map(Into::into),
            ..Default::default()
        };
        app.apply(with(Some("en")), &mut sys);
        assert_eq!(app.lang(), Lang::En);
        app.apply(with(None), &mut sys);
        assert_eq!(app.lang(), Lang::En);
        app.apply(with(Some("zh")), &mut sys);
        assert_eq!(app.lang(), Lang::Zh);
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

    fn snapshot_of(agents: Vec<Agent>) -> Snapshot {
        Snapshot {
            v: VERSION,
            now: T,
            agents,
            ..Default::default()
        }
    }

    fn ids(app: &App) -> Vec<&str> {
        (0..app.order.len())
            .map(|i| app.agent_at(i).unwrap().id.as_str())
            .collect()
    }

    #[test]
    fn a_card_marked_read_acknowledges_slides_to_the_next_and_stays_out() {
        let fleet = || {
            vec![
                agent("a", State::Done, 1),
                agent("b", State::Done, 2),
                agent("c", State::Work, 3),
            ]
        };
        let (mut app, mut ui, mut sys) = board(fleet());
        app.screen = DECK;
        press(&mut app, &mut ui, &mut sys, gsp::GSP_ACT_ID_SEEN);
        // The button acknowledges at once; the card is still there.
        assert_eq!(app.leaving.as_ref().map(|l| l.agent.as_str()), Some("a"));
        assert_eq!(ids(&app), ["a", "b", "c"]);
        app.poll(ACK_MS + 1, &mut ui, &mut sys);
        assert!(app.leaving.as_ref().is_some_and(|l| l.moved));
        let next = (app.slot + 1) % SLOTS as u16;
        app.handle(
            ACK_MS + 300,
            Event::Settled {
                key: gsp::GSP_OBJ_KEY_CARDS,
                value: next,
            },
            &mut ui,
            &mut sys,
        );
        assert!(app.leaving.is_none());
        assert_eq!(ids(&app), ["b", "c"]);
        assert_eq!(app.current_agent().unwrap().id, "b");
        let note = app.note.as_ref().unwrap();
        assert_eq!(
            (note.agent.as_str(), note.text.as_str()),
            ("b", "已读 · 还剩 2 张")
        );
        // An older plugin keeps reporting it: it stays out of the deck...
        app.apply(snapshot_of(fleet()), &mut sys);
        assert_eq!(ids(&app), ["b", "c"]);
        // ...until the agent moves on.
        let mut again = fleet();
        again[0].since = Some(T);
        app.apply(snapshot_of(again), &mut sys);
        assert_eq!(ids(&app), ["a", "b", "c"]);
    }

    #[test]
    fn marking_the_last_card_read_returns_to_the_lamp() {
        let (mut app, mut ui, mut sys) = board(vec![agent("a", State::Fail, 1)]);
        app.screen = DECK;
        press(&mut app, &mut ui, &mut sys, gsp::GSP_ACT_ID_SEEN);
        app.poll(ACK_MS + 1, &mut ui, &mut sys);
        app.handle(
            ACK_MS + 400,
            Event::Settled {
                key: gsp::GSP_OBJ_KEY_SCREENS,
                value: HOME,
            },
            &mut ui,
            &mut sys,
        );
        assert_eq!(app.screen, HOME);
        assert!(app.order.is_empty());
        let note = app.note.as_ref().unwrap();
        assert_eq!((note.agent.as_str(), note.text.as_str()), ("", "都看完了"));
        // The note fades on its own.
        app.poll(ACK_MS + 400 + READ_NOTE_MS, &mut ui, &mut sys);
        assert!(app.note.is_none());
    }

    #[test]
    fn a_missed_settle_still_takes_the_card_out() {
        let (mut app, mut ui, mut sys) =
            board(vec![agent("a", State::Done, 1), agent("b", State::Done, 2)]);
        app.screen = DECK;
        press(&mut app, &mut ui, &mut sys, gsp::GSP_ACT_ID_SEEN);
        app.poll(ACK_MS + 1, &mut ui, &mut sys);
        app.poll(ACK_MS + LEAVE_MS + 1, &mut ui, &mut sys);
        assert!(app.leaving.is_none());
        assert_eq!(ids(&app), ["b"]);
    }

    #[test]
    fn settings_flow_both_ways_and_a_local_change_wins_until_confirmed() {
        let (mut app, mut ui, mut sys) = board(vec![]);
        app.session.configure(Some(direct("pw")), 0);
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
