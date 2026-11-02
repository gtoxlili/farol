//! The setup pages, part of [`App`]: connecting to Paseo in either of the two
//! ways its app offers (scan Paseo's pairing QR for the encrypted relay, or
//! type a direct connection: host, port, SSL, password), the daemon password
//! Paseo asks for while connecting, forgetting Paseo, and joining Wi-Fi. The
//! pages are drawn in `view.rs`.

use alloc::string::String;

use gsp_sys as gsp;

use super::{App, Entry, FORGET_ARM_MS, Overlay};
use crate::endpoint::{self, Config, Endpoint};
use crate::paseo::Down;
use crate::qr::{self, Code};
use crate::sys::{Haptic, NetState, Sys, WifiCmd};
use crate::ui::{Bind, Ui};

impl App {
    /// The setup pages' buttons; false when `action` is not one of them.
    pub(super) fn setup_call(&mut self, action: u32, ui: &mut impl Ui, sys: &mut impl Sys) -> bool {
        use gsp::*;
        match action {
            GSP_ACT_ID_CONNECT_OPEN => self.overlay = Overlay::Connect,
            GSP_ACT_ID_SCAN_OPEN => {
                self.overlay = Overlay::Scan;
                self.scan_since = self.now;
                self.note = None;
                sys.camera(true);
            }
            GSP_ACT_ID_SCAN_CANCEL => {
                sys.camera(false);
                self.overlay = Overlay::None;
            }
            GSP_ACT_ID_DIRECT_OPEN => {
                if self.overlay == Overlay::Scan {
                    sys.camera(false);
                }
                self.open_input(Entry::Host, ui);
            }
            GSP_ACT_ID_TLS_OFF | GSP_ACT_ID_TLS_ON => {
                if let Overlay::Tls { host, port } =
                    core::mem::replace(&mut self.overlay, Overlay::None)
                {
                    let tls = action == GSP_ACT_ID_TLS_ON;
                    self.open_input(Entry::Password { host, port, tls }, ui);
                    self.feel(Haptic::Tick, sys);
                }
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
            GSP_ACT_ID_IN_CANCEL => {
                // Declining the password Paseo asked for ends this attempt's prompts.
                if matches!(
                    self.overlay,
                    Overlay::Input {
                        entry: Entry::DaemonPassword { .. },
                        ..
                    }
                ) {
                    self.pairing = false;
                }
                self.overlay = Overlay::None;
            }
            GSP_ACT_ID_WF_CANCEL
            | GSP_ACT_ID_NI_CLOSE
            | GSP_ACT_ID_LK_CLOSE
            | GSP_ACT_ID_CN_CANCEL
            | GSP_ACT_ID_TL_CANCEL
            | GSP_ACT_ID_PAIR_CANCEL => {
                self.overlay = Overlay::None;
            }
            GSP_ACT_ID_LK_FORGET => self.forget_paseo(sys),
            GSP_ACT_ID_PAIR_CONNECT => {
                if let Overlay::Pair { config } =
                    core::mem::replace(&mut self.overlay, Overlay::None)
                {
                    self.connect(config, sys);
                    self.feel(Haptic::Confirm, sys);
                }
            }
            _ => return false,
        }
        true
    }

    /// Saves `config` and connects with it. Until Paseo answers, a refused
    /// password brings up the password page ([`App::password_needed`]).
    fn connect(&mut self, config: Config, sys: &mut impl Sys) {
        sys.save_config(Some(&config));
        self.has_snapshot = false;
        self.host.clear();
        self.pairing = true;
        self.session.configure(Some(config), self.now);
    }

    /// Paseo refused the password of a connection made on this board just
    /// now: ask for it (a relay pairing code never carries one).
    pub(super) fn password_needed(&mut self, down: &Down) {
        let asking = matches!(
            self.overlay,
            Overlay::Input {
                entry: Entry::DaemonPassword { .. },
                ..
            }
        );
        if !self.pairing || asking {
            return;
        }
        let Some(config) = self.session.config().cloned() else {
            return;
        };
        let error = (*down == Down::Auth).then(|| String::from("密码不对，再输一次"));
        self.overlay = Overlay::Input {
            entry: Entry::DaemonPassword { config },
            error,
            joining: false,
        };
        self.keyboard_again = true;
    }

    /// Shows the input page for `entry` with the keyboard on an empty field
    /// (GSP's keyboard keeps its own text, which starts empty).
    fn open_input(&mut self, entry: Entry, ui: &mut impl Ui) {
        self.overlay = Overlay::Input {
            entry,
            error: None,
            joining: false,
        };
        self.attach_keyboard(ui);
    }

    pub(super) fn attach_keyboard(&mut self, ui: &mut impl Ui) {
        // The keyboard writes the field itself; the views never do.
        self.painter
            .run(ui.keyboard_attach(gsp::GSP_ACT_ID_KB_KEY as u16, gsp::GSP_BIND_IN_TEXT as Bind));
        self.painter
            .run(ui.set_text(gsp::GSP_BIND_IN_TEXT as Bind, ""));
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
        match entry.clone() {
            Entry::Host => match endpoint::parse_host(&text) {
                Ok((host, port)) => {
                    let default = port.unwrap_or(endpoint::DEFAULT_PORT);
                    self.open_input(Entry::Port { host, default }, ui);
                    self.feel(Haptic::Tick, sys);
                }
                Err(why) => self.refuse(entry, why, sys),
            },
            Entry::Port { host, default } => match endpoint::parse_port(&text, default) {
                Ok(port) => {
                    self.overlay = Overlay::Tls { host, port };
                    self.feel(Haptic::Tick, sys);
                }
                Err(why) => self.refuse(entry, why, sys),
            },
            Entry::Password { host, port, tls } => {
                self.overlay = Overlay::Pair {
                    config: Config {
                        endpoint: Endpoint::Direct { host, port, tls },
                        password: text,
                    },
                };
                self.feel(Haptic::Tick, sys);
            }
            Entry::DaemonPassword { mut config } => {
                if text.is_empty() {
                    self.refuse(entry, "请输入 daemon 密码", sys);
                } else {
                    config.password = text;
                    self.overlay = Overlay::None;
                    self.connect(config, sys);
                    self.feel(Haptic::Confirm, sys);
                }
            }
            Entry::WifiPassword { ssid } => {
                if text.is_empty() {
                    self.refuse(entry, "请输入这个网络的密码", sys);
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

    /// Keeps the input page for `entry`, saying why its text was refused.
    fn refuse(&mut self, entry: Entry, why: &str, sys: &mut impl Sys) {
        self.overlay = Overlay::Input {
            entry,
            error: Some(String::from(why)),
            joining: false,
        };
        self.feel(Haptic::Fail, sys);
    }

    /// How a Wi-Fi password typed on the board turned out.
    pub(super) fn join_result(&mut self) {
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

    /// Two taps: the first arms, the second forgets this Paseo and its password.
    fn forget_paseo(&mut self, sys: &mut impl Sys) {
        if self.now >= self.forget_until {
            self.forget_until = self.now + FORGET_ARM_MS;
            self.feel(Haptic::Tick, sys);
            return;
        }
        self.forget_until = 0;
        self.pairing = false;
        sys.save_config(None);
        self.session.configure(None, self.now);
        self.reset_fleet();
        self.clock = None;
        self.host.clear();
        self.last_sync = None;
        self.overlay = Overlay::None;
        self.feel(Haptic::Confirm, sys);
    }

    pub(super) fn on_scanned(&mut self, payload: &str, sys: &mut impl Sys) {
        match qr::parse(payload) {
            Some(Code::Wifi { ssid, password }) => {
                sys.camera(false);
                sys.wifi(WifiCmd::Join { ssid, password });
                self.overlay = Overlay::None;
                self.feel(Haptic::Confirm, sys);
            }
            Some(Code::Paseo(config)) => {
                sys.camera(false);
                self.overlay = Overlay::Pair { config };
                self.feel(Haptic::Confirm, sys);
            }
            None => {
                let lower = payload.trim().to_ascii_lowercase();
                let what = if lower.starts_with("http://") || lower.starts_with("https://") {
                    "这是一个网址，不是 Paseo 的配对码"
                } else {
                    "这不是 Paseo 配对码或 Wi-Fi 码"
                };
                self.scan_since = self.now;
                self.note_on("", what);
            }
        }
    }
}
