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
use crate::model::{Act, ActKind, ActResult, Agent, Snapshot, State};
use crate::paseo::{Config, Down, Notice, Out, Session};
use crate::qr::{self, Code};
use crate::sys::{
    Button, Chime, Display, Event, Glow, Haptic, Link, Motion, Net, NetState, Power, Prefs, Sys,
    WifiCmd,
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
/// A change of urgency glides the lamp to its new color this long. GSP cannot
/// tween the orb's color, so the logic steps it every [`FADE_STEP_MS`].
const LAMP_FADE_MS: u64 = 600;
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

    pub fn take_first_rejection(&mut self) -> Option<gsp::esp_gsp_err_t> {
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
        if self.has_snapshot
            && now_ms / ORDER_REFRESH_MS != self.last_second * 1000 / ORDER_REFRESH_MS
        {
            let current = self.current_agent().map(|a| a.id.clone());
            self.refresh_order(current);
        }
        self.last_second = now_ms / 1000;
        self.after(ui, sys);
    }

    /// When [`App::poll`] is next due, on the same clock.
    pub fn next_deadline_ms(&self) -> u64 {
        let mut next = (self.now / 1000 + 1) * 1000;
        next = next.min(self.session.next_deadline());
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
        self.pump_session(sys);
        self.update_overlay();
        self.apply_power(sys);
        self.apply_glow(sys);
        self.render(ui);
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
                sys.camera(true);
            }
            GSP_ACT_ID_SCAN_CANCEL => {
                sys.camera(false);
                self.overlay = Overlay::None;
            }
            GSP_ACT_ID_MANUAL_OPEN | GSP_ACT_ID_WIFI_PICK => {
                // Typing addresses and picking networks arrive with the keyboard pages.
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
                // Until the connection details page lands: the reason page
                // when Paseo is unreachable.
                if self.session.is_configured() && !self.session.is_ready() {
                    self.overlay = Overlay::NoPaseo;
                }
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
                self.note = Some(Note {
                    agent: String::new(),
                    text: String::from("这不是 Paseo 配对码或 Wi-Fi 码"),
                    until: self.now + NOTE_MS,
                });
            }
        }
    }

    // ------------------------------------------------------------ data

    fn apply(&mut self, mut snapshot: Snapshot, sys: &mut impl Sys) {
        if snapshot.v != crate::model::VERSION {
            return;
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
        let current = self.current_agent().map(|a| a.id.clone());
        self.clock = Some((snapshot.now, self.now));
        self.tz = snapshot.tz;
        if !snapshot.host.is_empty() {
            self.host = snapshot.host.clone();
        }
        self.snapshot = snapshot;
        self.has_snapshot = true;
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
        if self.demo.is_none() {
            self.session.set_focus(focus.as_deref());
        }
    }

    fn jump_to_urgent(&mut self, ui: &mut impl Ui, sys: &mut impl Sys) {
        if self.order.is_empty() {
            self.painter
                .run(ui.set_i32(gsp::GSP_OBJ_KEY_LAMP, gsp::GSP_PROP_KEY_RIPPLE, 95));
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
        self.face_down || self.dnd_until.is_some_and(|until| self.now < until)
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
            Overlay::Pair { .. } | Overlay::Scan => return,
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
            return;
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

    struct NullUi;

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
        fn animate_color_to(&mut self, _: Key, _: u32, _: u32, _: Ease) -> UiResult {
            Ok(())
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
        let (mut app, mut ui, mut sys) = (App::new(0, "test"), NullUi, Effects::default());
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
