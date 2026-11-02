//! The boundary to the platform: what it reports ([`Event`]) and what it
//! carries out ([`Sys`]). The board implements [`Sys`] over its C
//! components, the simulator logs or emulates, the tests record.

use alloc::string::String;
use alloc::vec::Vec;

use crate::paseo::{Config, Down, Out};

/// Everything the logic reacts to.
#[derive(Clone, Debug, PartialEq)]
pub enum Event {
    /// A scene `call` callback (`GSP_ACT_ID_*`) fired; `arg` is a slider value.
    Call {
        action: u32,
        arg: i32,
    },
    /// A PageFlow or Drawer (`key`) settled on `value`.
    Settled {
        key: u32,
        value: u16,
    },
    /// Any touch, for sleep and wake.
    Touch,
    Link(Link),
    Net(Net),
    Power(Power),
    Button(Button),
    /// The interaction module was attached (true) or removed (false).
    Module(bool),
    /// The interaction module's PIR: someone is near.
    Presence(bool),
    /// The interaction module's ambient light, 0..=100 (relative).
    Ambient(u8),
    /// The board was picked up or tapped (IMU), or turned face down/up.
    Motion(Motion),
    /// A QR code the camera decoded.
    Scanned(String),
    /// The camera could not start: no module in the left slot, or it failed.
    NoCamera,
    /// A Wi-Fi scan finished: networks in range, strongest first.
    Networks(Vec<Ap>),
    /// (Re)start the demo fleet this far into its script, on the home page:
    /// measurement runs start from the same moment every time.
    Demo {
        offset_ms: u64,
    },
    /// Stored settings loaded at boot.
    Restored {
        config: Option<Config>,
        prefs: Prefs,
    },
}

#[derive(Clone, Debug, PartialEq)]
pub enum Link {
    Opened,
    Closed(Down),
    Text(String),
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
pub enum NetState {
    /// Wi-Fi not started yet.
    #[default]
    Off,
    /// No network saved on the board.
    NoSaved,
    /// Saved networks exist, none in range.
    NotFound,
    Connecting,
    WrongPassword,
    /// Associated but DHCP gave no address.
    NoAddress,
    Online,
}

#[derive(Clone, Debug, PartialEq, Default)]
pub struct Net {
    pub state: NetState,
    /// The network in use or being tried.
    pub ssid: String,
    /// dBm while online.
    pub rssi: i8,
    /// Saved network names, most recent first.
    pub saved: Vec<String>,
    /// IPv4 address while online, dotted.
    pub ip: String,
}

/// One network a scan found.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Ap {
    pub ssid: String,
    /// dBm.
    pub rssi: i8,
    /// Needs a password.
    pub secure: bool,
    /// Saved on the board.
    pub saved: bool,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
pub struct Power {
    pub percent: u8,
    pub charging: bool,
    /// Minutes to empty (on battery) or to full (charging), when known.
    pub minutes: Option<u16>,
    /// The fuel gauge answered; without it the status bar hides the battery.
    pub known: bool,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Button {
    /// The AI key, clicked.
    AiClick,
    /// The AI key, held.
    AiLong,
    /// Interaction module touch keys.
    Prev,
    Next,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Motion {
    Lifted,
    FaceDown,
    FaceUp,
}

/// User preferences kept on the board.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Prefs {
    pub sound: bool,
    pub haptic: bool,
    /// Brightness, 5..=100.
    pub brightness: u8,
    /// Follow ambient light (interaction module only).
    pub auto: bool,
}

impl Default for Prefs {
    fn default() -> Self {
        Self {
            sound: true,
            haptic: true,
            brightness: 80,
            auto: true,
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Haptic {
    /// A page settled, an option was pressed.
    Tick,
    /// An action went out.
    Confirm,
    /// Someone is waiting: two soft pulses.
    Call,
    /// A turn failed: long then short.
    Fail,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Chime {
    Call,
    Done,
    Fail,
}

/// What the interaction module's LEDs show; ignored without the module.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Glow {
    Off,
    Work,
    Call,
    Fail,
    /// A one-off white flash for a finished turn.
    Flash,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Display {
    /// Panel on at this brightness (percent).
    On(u8),
    Off,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum WifiCmd {
    /// Save this network (first in the list) and connect to it. An empty
    /// password joins with the saved one if there is one, else as an open
    /// network; it never replaces a saved password.
    Join { ssid: String, password: String },
    /// Scan for networks; answered with [`Event::Networks`].
    Scan,
    /// Try the saved networks again now.
    Retry,
    /// Remove a saved network; disconnects when it is the one in use.
    Forget { ssid: String },
}

/// What the logic asks the platform to do. Every method returns at once.
pub trait Sys {
    fn link(&mut self, out: Out);
    fn haptic(&mut self, haptic: Haptic);
    fn chime(&mut self, chime: Chime);
    fn glow(&mut self, glow: Glow);
    /// Interaction-module LED brightness, 0..=100 (set in Paseo; default 60).
    fn led_brightness(&mut self, percent: u8);
    fn display(&mut self, display: Display);
    fn save_config(&mut self, config: Option<&Config>);
    fn save_prefs(&mut self, prefs: &Prefs);
    fn wifi(&mut self, cmd: WifiCmd);
    fn camera(&mut self, on: bool);
}
