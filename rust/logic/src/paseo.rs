//! The board's Paseo client session, as a pure state machine.
//!
//! The board connects to the daemon like Paseo's app, in either of its two
//! ways ([`crate::endpoint`]): straight to the daemon's `/ws`, or through a
//! Paseo relay with the end-to-end encrypted channel of [`crate::relay`]
//! wrapped around every frame. Either way it sends a `hello` carrying the
//! daemon password, then only plugin RPCs to the Farol plugin. Paseo 0.14 has no
//! plugin push, so the board long-polls `farol.next` (the plugin answers
//! within 20 s, under the daemon's 30 s RPC limit) and sends `farol.act` for
//! actions. The platform moves frames ([`Out`]) and reports transport
//! changes; everything here runs on the host in tests.
//!
//! Wire shapes (Paseo v0.14.0, `packages/protocol/src/messages.ts`): every
//! frame is one JSON text frame; session traffic is wrapped as
//! `{"type":"session","message":{..}}`; `{"type":"ping"}` is answered with
//! `{"type":"pong"}`; a refused hello yields `hello.rejected` and close 4401
//! (a relay replaces close codes with its own, so the board asks for
//! `hello.rejected` up front).

use alloc::collections::BTreeMap;
use alloc::format;
use alloc::string::{String, ToString};
use alloc::vec::Vec;

use serde_json::{Value, json};

pub use crate::endpoint::Config;
use crate::model::{self, Act, ActResult, BoardPrefs, Snapshot};
use crate::relay;

/// Plugin id the RPCs address.
pub const PLUGIN_ID: &str = "farol";
const HELLO_TIMEOUT_MS: u64 = 15_000;
/// The relay handshake's `e2ee_hello` repeats this often until the daemon answers.
const RELAY_HELLO_EVERY_MS: u64 = 1_000;
/// Relay closes: the daemon is not attached to the relay (1012), or the relay
/// gave up waiting for it or is overloaded (1013); a refused routing key (1008).
const RELAY_GONE: [u16; 2] = [1012, 1013];
const RELAY_BAD_KEY: u16 = 1008;
const PING_EVERY_MS: u64 = 10_000;
/// No frame at all for this long means the link is dead (pings get pongs).
const SILENCE_LIMIT_MS: u64 = 30_000;
const BACKOFF_MIN_MS: u64 = 1_000;
const BACKOFF_MAX_MS: u64 = 60_000;
/// After a refused password, retry slowly in case it is restored.
const AUTH_RETRY_MS: u64 = 60_000;
const POLL_RETRY_MS: u64 = 2_000;
const PLUGIN_MISSING_RETRY_MS: u64 = 15_000;

/// Why the session is not ready, for the status page.
#[derive(Clone, Debug, PartialEq, Eq, Default)]
pub enum Down {
    #[default]
    None,
    /// The board has no address yet.
    Unconfigured,
    /// The platform could not resolve the host.
    Dns,
    /// TCP connect or TLS handshake timed out.
    Timeout,
    /// TLS failed (certificate not trusted, wrong host).
    Tls,
    /// The host refused the connection or answered HTTP with an error (e.g. 403
    /// for a host name the daemon does not allow).
    Refused(u16),
    /// The daemon refused the password.
    Auth,
    /// The daemon wants a password and the board has none.
    NeedPassword,
    /// The relay's encrypted channel failed: the daemon's key changed (pair
    /// again) or the daemon is too old for it.
    Handshake,
    /// The relay could not reach the daemon (offline, or relay turned off).
    RelayAway,
    /// The daemon does not speak this protocol version.
    Incompatible,
    /// The Farol plugin answers in another farol/N (its version).
    PluginVersion(u32),
    /// Connected, but the Farol plugin is not installed or not running.
    PluginMissing,
    /// The socket closed (code) or went silent.
    Closed(u16),
}

/// What the platform must do with the transport.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Out {
    Open(String),
    Close,
    Send(String),
}

/// What the session tells the app.
#[derive(Clone, Debug, PartialEq)]
pub enum Notice {
    Ready { host: String },
    Snapshot(Snapshot),
    Acted { token: u32, result: ActResult },
    Down(Down),
}

#[derive(Clone, Debug, PartialEq, Eq)]
enum Phase {
    Idle,
    Backoff {
        until: u64,
    },
    Connecting {
        since: u64,
    },
    /// Relay only: the encrypted channel's handshake.
    Securing {
        since: u64,
        resend_at: u64,
    },
    Hello {
        since: u64,
    },
    Ready,
}

/// The relay channel of the current connection.
enum Secure {
    /// A direct connection: frames go as they are.
    Plain,
    Handshake(relay::Initiator),
    Open(relay::Channel),
}

pub struct Session {
    config: Option<Config>,
    secure: Secure,
    device: String,
    phase: Phase,
    failures: u32,
    down: Down,
    next_id: u64,
    poll: Option<String>,
    poll_retry_at: Option<u64>,
    acts: BTreeMap<String, u32>,
    cursor: u64,
    focus: Option<String>,
    /// Board settings the plugin has not confirmed yet.
    prefs: Option<BoardPrefs>,
    /// The poll in flight carries `prefs`.
    prefs_in_poll: bool,
    /// The screen in front, as the plugin knows it.
    screen: &'static str,
    last_rx: u64,
    last_ping: u64,
    ping_at: Option<u64>,
    rtt_ms: Option<u32>,
    out: Vec<Out>,
    notices: Vec<Notice>,
}

impl Session {
    /// `device` identifies this board to Paseo and to the plugin (stable per board).
    pub fn new(device: &str) -> Self {
        Self {
            config: None,
            secure: Secure::Plain,
            device: device.to_string(),
            phase: Phase::Idle,
            failures: 0,
            down: Down::Unconfigured,
            next_id: 0,
            poll: None,
            poll_retry_at: None,
            acts: BTreeMap::new(),
            cursor: 0,
            focus: None,
            prefs: None,
            prefs_in_poll: false,
            screen: "home",
            last_rx: 0,
            last_ping: 0,
            ping_at: None,
            rtt_ms: None,
            out: Vec::new(),
            notices: Vec::new(),
        }
    }

    /// Sets (or clears) the address and password and starts over.
    pub fn configure(&mut self, config: Option<Config>, now: u64) {
        if self.config == config {
            return;
        }
        if !matches!(self.phase, Phase::Idle | Phase::Backoff { .. }) {
            self.out.push(Out::Close);
        }
        self.reset_requests();
        self.config = config;
        self.failures = 0;
        self.cursor = 0;
        match &self.config {
            Some(c) => {
                self.phase = Phase::Connecting { since: now };
                self.out.push(Out::Open(c.url()));
                self.set_down(Down::None);
            }
            None => {
                self.phase = Phase::Idle;
                self.set_down(Down::Unconfigured);
            }
        }
    }

    pub fn is_ready(&self) -> bool {
        self.phase == Phase::Ready
    }

    /// The daemon address and password in use, if any.
    pub fn config(&self) -> Option<&Config> {
        self.config.as_ref()
    }

    pub fn is_configured(&self) -> bool {
        self.config.is_some()
    }

    /// The reason the session is not ready, or `Down::None`.
    pub fn down(&self) -> &Down {
        &self.down
    }

    /// Consecutive failed connection attempts.
    pub fn failures(&self) -> u32 {
        self.failures
    }

    /// Round trip of the last ping, if one completed.
    pub fn rtt_ms(&self) -> Option<u32> {
        self.rtt_ms
    }

    /// When the next reconnect attempt is due, if waiting.
    pub fn retry_at(&self) -> Option<u64> {
        match self.phase {
            Phase::Backoff { until } => Some(until),
            _ => None,
        }
    }

    pub fn take_out(&mut self) -> Vec<Out> {
        core::mem::take(&mut self.out)
    }

    pub fn take_notices(&mut self) -> Vec<Notice> {
        core::mem::take(&mut self.notices)
    }

    /// Reconnect now instead of waiting out the backoff.
    pub fn retry_now(&mut self, now: u64) {
        if let (Phase::Backoff { .. }, Some(c)) = (&self.phase, &self.config) {
            self.phase = Phase::Connecting { since: now };
            self.out.push(Out::Open(c.url()));
        }
    }

    /// The transport opened. Direct: introduce ourselves. Relay: start the
    /// encrypted channel first; `entropy` (fresh random bytes) seeds its keys.
    pub fn on_open(&mut self, now: u64, entropy: &[u8; 64]) {
        let Some(config) = &self.config else {
            self.out.push(Out::Close);
            return;
        };
        self.last_rx = now;
        self.last_ping = now;
        self.ping_at = None;
        let Some(r) = config.relay() else {
            self.secure = Secure::Plain;
            self.send_hello(now);
            return;
        };
        let (ephemeral, routing) = entropy.split_at(32);
        let start = relay::Initiator::start(
            &r.daemon_key,
            ephemeral.try_into().unwrap_or_default(),
            routing.try_into().unwrap_or_default(),
        );
        match start {
            Ok(handshake) => {
                self.out.push(Out::Send(handshake.hello().into()));
                self.secure = Secure::Handshake(handshake);
                self.phase = Phase::Securing {
                    since: now,
                    resend_at: now + RELAY_HELLO_EVERY_MS,
                };
            }
            Err(_) => self.fail(now, Down::Handshake),
        }
    }

    fn send_hello(&mut self, now: u64) {
        let Some(config) = &self.config else {
            return;
        };
        let mut hello = json!({
            "type": "hello",
            "clientId": format!("farol-{}", self.device),
            "clientType": "cli",
            "protocolVersion": 1,
            "appVersion": concat!("farol/", env!("CARGO_PKG_VERSION")),
            "capabilities": {"hello_rejection": true, "owned_subscriptions": true},
        });
        if !config.password.is_empty() {
            hello["auth"] = json!({"kind": "password", "password": config.password});
        }
        self.send(hello.to_string());
        self.phase = Phase::Hello { since: now };
    }

    /// One frame to the daemon, sealed when the relay channel is open.
    fn send(&mut self, text: String) {
        let frame = match &mut self.secure {
            Secure::Open(channel) => channel.seal(&text),
            _ => text,
        };
        self.out.push(Out::Send(frame));
    }

    /// Closes the socket and waits as for a failed connection.
    fn fail(&mut self, now: u64, why: Down) {
        self.out.push(Out::Close);
        self.on_close(now, why);
    }

    /// The transport closed or failed to open; `why` is the platform's reading.
    pub fn on_close(&mut self, now: u64, why: Down) {
        if matches!(self.phase, Phase::Idle | Phase::Backoff { .. }) && self.config.is_none() {
            return;
        }
        self.reset_requests();
        let relay = self.config.as_ref().is_some_and(|c| c.relay().is_some());
        self.secure = Secure::Plain;
        let why = match (&self.down, why) {
            // A close right after hello.rejected keeps the rejection's reason.
            (Down::Auth | Down::NeedPassword | Down::Incompatible, Down::Closed(_)) => {
                self.down.clone()
            }
            (_, Down::Closed(4401)) => Down::Auth,
            (_, Down::Closed(4003)) => Down::Incompatible,
            (_, Down::Closed(code)) if relay && RELAY_GONE.contains(&code) => Down::RelayAway,
            (_, Down::Closed(RELAY_BAD_KEY)) if relay => Down::Handshake,
            (_, w) => w,
        };
        self.failures = self.failures.saturating_add(1);
        let wait = match why {
            Down::Auth | Down::NeedPassword | Down::Incompatible | Down::Handshake => AUTH_RETRY_MS,
            _ => (BACKOFF_MIN_MS << self.failures.saturating_sub(1).min(6)).min(BACKOFF_MAX_MS),
        };
        self.phase = if self.config.is_some() {
            Phase::Backoff { until: now + wait }
        } else {
            Phase::Idle
        };
        self.set_down(why);
    }

    /// One text message from the socket: through the relay channel when
    /// there is one, else straight to [`Session::on_frame`].
    pub fn on_text(&mut self, now: u64, text: &str) {
        self.last_rx = now;
        match &mut self.secure {
            Secure::Plain => self.on_frame(now, text),
            Secure::Handshake(_) => {
                let Ok(frame) = serde_json::from_str::<Value>(text) else {
                    return;
                };
                if frame["type"] != "e2ee_ready" {
                    return;
                }
                let Secure::Handshake(handshake) =
                    core::mem::replace(&mut self.secure, Secure::Plain)
                else {
                    return;
                };
                match handshake.finish(&frame) {
                    Ok(channel) => {
                        self.secure = Secure::Open(channel);
                        self.send_hello(now);
                    }
                    Err(_) => self.fail(now, Down::Handshake),
                }
            }
            Secure::Open(channel) => {
                // Base64 records never start with `{`; plaintext JSON may only
                // be a repeated ready, anything else means the peer stopped
                // encrypting.
                if text.trim_start().starts_with('{') {
                    let frame = serde_json::from_str::<Value>(text).ok();
                    if frame.is_some_and(|f| f["type"] == "e2ee_ready") {
                        return;
                    }
                    self.fail(now, Down::Handshake);
                    return;
                }
                match channel.open(text) {
                    Ok(Some(frame)) => self.on_frame(now, &frame),
                    Ok(None) => {}
                    Err(_) => self.fail(now, Down::Handshake),
                }
            }
        }
    }

    /// One application frame from the daemon.
    fn on_frame(&mut self, now: u64, text: &str) {
        let Ok(frame) = serde_json::from_str::<Value>(text) else {
            return;
        };
        match frame["type"].as_str() {
            Some("pong") => {
                if let Some(at) = self.ping_at.take() {
                    self.rtt_ms = u32::try_from(now.saturating_sub(at)).ok();
                }
            }
            Some("hello.rejected") => {
                let down = match frame["reason"].as_str() {
                    Some("incompatible_protocol") => Down::Incompatible,
                    Some("password_required") => Down::NeedPassword,
                    _ => Down::Auth,
                };
                self.set_down(down);
            }
            Some("session") => self.on_session(now, &frame["message"]),
            _ => {}
        }
    }

    fn on_session(&mut self, now: u64, message: &Value) {
        match message["type"].as_str() {
            Some("status") if message["payload"]["status"] == "server_info" => {
                let host = message["payload"]["hostname"]
                    .as_str()
                    .unwrap_or("")
                    .to_string();
                self.phase = Phase::Ready;
                self.failures = 0;
                self.set_down(Down::None);
                self.notices.push(Notice::Ready { host });
                self.send_poll(now);
            }
            Some("plugin.rpc.invoke.response") => {
                let payload = &message["payload"];
                let id = payload["requestId"].as_str().unwrap_or("");
                self.on_output(now, id, &payload["output"]);
            }
            Some("rpc_error") => {
                let payload = &message["payload"];
                let id = payload["requestId"].as_str().unwrap_or("");
                let error = payload["error"].as_str().unwrap_or("");
                self.on_error(now, id, error);
            }
            _ => {}
        }
    }

    fn on_output(&mut self, now: u64, id: &str, output: &Value) {
        if self.poll.as_deref() == Some(id) {
            self.poll = None;
            match serde_json::from_value::<Snapshot>(output.clone()) {
                Ok(snapshot) if snapshot.v == model::VERSION => {
                    if matches!(self.down, Down::PluginMissing | Down::PluginVersion(_)) {
                        self.set_down(Down::None);
                    }
                    // The plugin has seen what this poll carried.
                    if self.prefs_in_poll {
                        self.prefs = None;
                    }
                    self.cursor = snapshot.rev;
                    if !snapshot.idle {
                        self.notices.push(Notice::Snapshot(snapshot));
                    }
                    self.send_poll(now);
                }
                _ => {
                    let v = output["v"]
                        .as_u64()
                        .map_or(0, |v| v.min(u64::from(u32::MAX)) as u32);
                    self.set_down(Down::PluginVersion(v));
                    self.poll_retry_at = Some(now + PLUGIN_MISSING_RETRY_MS);
                }
            }
        } else if let Some(token) = self.acts.remove(id) {
            let result = serde_json::from_value::<ActResult>(output.clone()).unwrap_or(ActResult {
                ok: false,
                reason: "failed".into(),
                message: "unreadable reply".into(),
            });
            self.notices.push(Notice::Acted { token, result });
        }
    }

    fn on_error(&mut self, now: u64, id: &str, error: &str) {
        if self.poll.as_deref() == Some(id) {
            self.poll = None;
            let missing = error.contains("not available") || error.contains("does not contribute");
            if missing {
                self.set_down(Down::PluginMissing);
            }
            let wait = if missing {
                PLUGIN_MISSING_RETRY_MS
            } else {
                POLL_RETRY_MS
            };
            self.poll_retry_at = Some(now + wait);
        } else if let Some(token) = self.acts.remove(id) {
            let result = ActResult {
                ok: false,
                reason: "failed".into(),
                message: error.to_string(),
            };
            self.notices.push(Notice::Acted { token, result });
        }
    }

    /// Time-driven work: reconnects, pings, timeouts, poll retries.
    pub fn poll(&mut self, now: u64) {
        match self.phase {
            Phase::Backoff { until } if now >= until => self.retry_now(now),
            Phase::Connecting { since } if now.saturating_sub(since) > HELLO_TIMEOUT_MS * 2 => {
                self.fail(now, Down::Timeout);
            }
            Phase::Securing { since, .. } if now.saturating_sub(since) > HELLO_TIMEOUT_MS => {
                self.fail(now, Down::Timeout);
            }
            Phase::Securing { since, resend_at } if now >= resend_at => {
                // The same bytes again: the daemon answers each with a ready.
                if let Secure::Handshake(handshake) = &self.secure {
                    self.out.push(Out::Send(handshake.hello().into()));
                }
                self.phase = Phase::Securing {
                    since,
                    resend_at: now + RELAY_HELLO_EVERY_MS,
                };
            }
            Phase::Hello { since } if now.saturating_sub(since) > HELLO_TIMEOUT_MS => {
                self.fail(now, Down::Timeout);
            }
            Phase::Hello { .. } | Phase::Ready => {
                if now.saturating_sub(self.last_rx) > SILENCE_LIMIT_MS {
                    self.fail(now, Down::Closed(0));
                    return;
                }
                if now.saturating_sub(self.last_ping) >= PING_EVERY_MS {
                    self.last_ping = now;
                    self.ping_at = Some(now);
                    self.send(String::from(r#"{"type":"ping"}"#));
                }
                if self.phase == Phase::Ready
                    && self.poll.is_none()
                    && let Some(at) = self.poll_retry_at
                    && now >= at
                {
                    self.poll_retry_at = None;
                    self.send_poll(now);
                }
            }
            _ => {}
        }
    }

    /// When [`Session::poll`] next has work, at the latest.
    pub fn next_deadline(&self) -> u64 {
        match self.phase {
            Phase::Backoff { until } => until,
            Phase::Connecting { since } => since + HELLO_TIMEOUT_MS * 2,
            Phase::Securing { since, resend_at } => resend_at.min(since + HELLO_TIMEOUT_MS),
            Phase::Hello { since } => {
                (since + HELLO_TIMEOUT_MS).min(self.last_ping + PING_EVERY_MS)
            }
            Phase::Ready => {
                let ping = self.last_ping + PING_EVERY_MS;
                self.poll_retry_at.map_or(ping, |at| at.min(ping))
            }
            Phase::Idle => u64::MAX,
        }
    }

    /// Sends an action; `token` comes back in [`Notice::Acted`]. Returns false
    /// when the session cannot send now (the caller shows "not connected").
    pub fn act(&mut self, act: &Act, token: u32) -> bool {
        if self.phase != Phase::Ready {
            return false;
        }
        let input = serde_json::to_value(act).unwrap_or(Value::Null);
        let id = self.rpc("farol.act", input);
        self.acts.insert(id, token);
        true
    }

    /// The card on screen changed; the plugin answers the waiting poll at once.
    /// The agent card on screen (its live detail is pushed) and the screen
    /// in front ("usage" asks for fresh quota numbers).
    pub fn set_focus(&mut self, focus: Option<&str>, screen: &'static str) {
        if self.focus.as_deref() == focus && self.screen == screen {
            return;
        }
        self.focus = focus.map(ToString::to_string);
        self.screen = screen;
        self.send_focus();
    }

    /// Settings changed on the board that the plugin has not confirmed yet.
    pub fn prefs_pending(&self) -> bool {
        self.prefs.is_some()
    }

    /// Board settings changed on the board: sent now and with every poll
    /// until one is answered.
    pub fn set_prefs(&mut self, prefs: BoardPrefs) {
        self.prefs = Some(prefs);
        self.prefs_in_poll = false;
        self.send_focus();
    }

    fn send_focus(&mut self) {
        if self.phase == Phase::Ready {
            let input = serde_json::to_value(model::Focus {
                focus: self.focus.as_deref(),
                device: &self.device,
                screen: self.screen,
                prefs: self.prefs,
            })
            .unwrap_or(Value::Null);
            self.rpc("farol.focus", input);
        }
    }

    fn send_poll(&mut self, _now: u64) {
        self.prefs_in_poll = self.prefs.is_some();
        let input = serde_json::to_value(model::Next {
            v: model::VERSION,
            cursor: self.cursor,
            focus: self.focus.as_deref(),
            device: &self.device,
            screen: self.screen,
            fw: crate::FIRMWARE,
            prefs: self.prefs,
        })
        .unwrap_or(Value::Null);
        let id = self.rpc("farol.next", input);
        self.poll = Some(id);
    }

    fn rpc(&mut self, method: &str, input: Value) -> String {
        self.next_id += 1;
        let id = format!("f{}", self.next_id);
        let frame = json!({
            "type": "session",
            "message": {
                "type": "plugin.rpc.invoke.request",
                "requestId": id,
                "pluginId": PLUGIN_ID,
                "method": method,
                "input": input,
            }
        });
        self.send(frame.to_string());
        id
    }

    fn reset_requests(&mut self) {
        self.poll = None;
        self.poll_retry_at = None;
        for (_, token) in core::mem::take(&mut self.acts) {
            self.notices.push(Notice::Acted {
                token,
                result: ActResult {
                    ok: false,
                    reason: "disconnected".into(),
                    message: String::new(),
                },
            });
        }
    }

    fn set_down(&mut self, down: Down) {
        if self.down != down {
            self.down = down.clone();
            self.notices.push(Notice::Down(down));
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::ActKind;

    fn config() -> Option<Config> {
        Some(Config {
            endpoint: crate::endpoint::Endpoint::Direct {
                host: "paseo.example.net".into(),
                port: 443,
                tls: true,
            },
            password: "pw".into(),
        })
    }

    fn sent(s: &mut Session) -> Vec<Value> {
        s.take_out()
            .into_iter()
            .filter_map(|o| match o {
                Out::Send(t) => Some(serde_json::from_str(&t).unwrap()),
                _ => None,
            })
            .collect()
    }

    const SERVER_INFO: &str = r#"{"type":"session","message":{"type":"status","payload":{"status":"server_info","hostname":"studio","version":"0.14.0"}}}"#;

    /// A ready session whose only queued frame is the first `farol.next`.
    fn ready() -> Session {
        let mut s = Session::new("3a1f");
        s.configure(config(), 0);
        s.on_open(10, &[0; 64]);
        s.take_out();
        s.on_text(20, SERVER_INFO);
        s
    }

    fn relay_vectors() -> Value {
        serde_json::from_str(include_str!("../../../protocol/fixtures/relay.json")).unwrap()
    }

    /// The vectors' board secrets, as the platform's random bytes.
    fn relay_entropy(v: &Value) -> [u8; 64] {
        let mut out = [0u8; 64];
        let hex = format!(
            "{}{}",
            v["boardEphemeralSecret"].as_str().unwrap(),
            v["boardRoutingSecret"].as_str().unwrap()
        );
        for (i, byte) in out.iter_mut().enumerate() {
            *byte = u8::from_str_radix(&hex[i * 2..i * 2 + 2], 16).unwrap();
        }
        out
    }

    /// A relay session past the handshake, and the daemon's end of its channel.
    fn relay_ready(password: &str) -> (Session, relay::Channel) {
        let v = relay_vectors();
        let mut config =
            crate::endpoint::parse_link(v["links"]["offerUrl"].as_str().unwrap()).unwrap();
        config.password = password.into();
        let mut s = Session::new("3a1f");
        s.configure(Some(config), 0);
        s.take_out();
        s.on_open(10, &relay_entropy(&v));
        s.take_out();
        s.on_text(20, v["handshake"]["readyText"].as_str().unwrap());
        let Secure::Open(channel) = &s.secure else {
            panic!("the channel did not open");
        };
        let daemon = channel.mirror();
        (s, daemon)
    }

    /// What the daemon reads from the board's queued frames.
    fn relay_sent(s: &mut Session, daemon: &mut relay::Channel) -> Vec<Value> {
        s.take_out()
            .into_iter()
            .filter_map(|o| match o {
                Out::Send(t) => daemon.open(&t).unwrap(),
                _ => None,
            })
            .map(|t| serde_json::from_str(&t).unwrap())
            .collect()
    }

    #[test]
    fn a_relay_connection_opens_the_relay_and_repeats_its_hello() {
        let v = relay_vectors();
        let config = crate::endpoint::parse_link(v["links"]["offerUrl"].as_str().unwrap()).unwrap();
        let mut s = Session::new("3a1f");
        s.configure(Some(config), 0);
        let Some(Out::Open(url)) = s.take_out().pop() else {
            panic!("no open");
        };
        assert_eq!(
            url.replace(":443", ""),
            v["links"]["relayWebSocketUrl"].as_str().unwrap()
        );
        s.on_open(10, &relay_entropy(&v));
        let hello = Out::Send(v["handshake"]["helloText"].as_str().unwrap().into());
        assert_eq!(s.take_out(), core::slice::from_ref(&hello));
        s.poll(1_010);
        assert_eq!(s.take_out(), [hello]);
        s.poll(15_011);
        assert!(s.take_out().contains(&Out::Close));
        assert_eq!(s.down(), &Down::Timeout);
    }

    #[test]
    fn over_the_relay_the_hello_and_rpcs_travel_sealed() {
        let (mut s, mut daemon) = relay_ready("pw");
        let hello = &relay_sent(&mut s, &mut daemon)[0];
        assert_eq!(hello["type"], "hello");
        assert_eq!(hello["clientId"], "farol-3a1f");
        assert_eq!(hello["auth"]["password"], "pw");
        s.on_text(30, &daemon.seal(SERVER_INFO));
        assert!(s.is_ready());
        let poll = &relay_sent(&mut s, &mut daemon)[0]["message"];
        assert_eq!(poll["method"], "farol.next");
        // Pings are sealed too; the daemon's pong comes back sealed.
        s.poll(10_030);
        assert_eq!(relay_sent(&mut s, &mut daemon)[0]["type"], "ping");
        s.on_text(10_050, &daemon.seal(r#"{"type":"pong"}"#));
        assert_eq!(s.rtt_ms(), Some(20));
    }

    #[test]
    fn a_relay_without_its_daemon_or_with_another_key_says_so() {
        let (mut s, _) = relay_ready("");
        s.on_close(40, Down::Closed(1013));
        assert_eq!(s.down(), &Down::RelayAway);

        let v = relay_vectors();
        let mut config =
            crate::endpoint::parse_link(v["links"]["offerUrl"].as_str().unwrap()).unwrap();
        if let crate::endpoint::Endpoint::Relay(r) = &mut config.endpoint {
            r.daemon_key[0] ^= 0x40;
        }
        let mut s = Session::new("3a1f");
        s.configure(Some(config), 0);
        s.on_open(10, &relay_entropy(&v));
        s.take_out();
        s.on_text(20, v["handshake"]["readyText"].as_str().unwrap());
        assert!(s.take_out().contains(&Out::Close));
        assert_eq!(s.down(), &Down::Handshake);
    }

    #[test]
    fn plaintext_after_the_channel_opened_ends_it() {
        let (mut s, _) = relay_ready("");
        s.take_out();
        s.on_text(30, SERVER_INFO);
        assert!(!s.is_ready());
        assert!(s.take_out().contains(&Out::Close));
        assert_eq!(s.down(), &Down::Handshake);
    }

    #[test]
    fn a_missing_password_is_told_apart_from_a_wrong_one() {
        let (mut s, mut daemon) = relay_ready("");
        s.on_text(
            30,
            &daemon.seal(
                r#"{"type":"hello.rejected","reason":"password_required","accepts":["password"]}"#,
            ),
        );
        s.on_close(31, Down::Closed(1012));
        assert_eq!(s.down(), &Down::NeedPassword);
    }

    #[test]
    fn hello_carries_the_password_and_identity() {
        let mut s = Session::new("3a1f");
        s.configure(config(), 0);
        assert_eq!(
            s.take_out(),
            [Out::Open("wss://paseo.example.net:443/ws".into())]
        );
        s.on_open(10, &[0; 64]);
        let hello = &sent(&mut s)[0];
        assert_eq!(hello["type"], "hello");
        assert_eq!(hello["clientId"], "farol-3a1f");
        assert_eq!(hello["clientType"], "cli");
        assert_eq!(hello["protocolVersion"], 1);
        assert_eq!(hello["auth"]["password"], "pw");
        assert_eq!(hello["capabilities"]["owned_subscriptions"], true);
    }

    #[test]
    fn server_info_makes_it_ready_and_starts_the_long_poll() {
        let mut s = Session::new("3a1f");
        s.configure(config(), 0);
        s.on_open(10, &[0; 64]);
        s.take_out();
        s.on_text(20, SERVER_INFO);
        assert!(s.is_ready());
        assert!(s.take_notices().contains(&Notice::Ready {
            host: "studio".into()
        }));
        let poll = &sent(&mut s)[0]["message"];
        assert_eq!(poll["type"], "plugin.rpc.invoke.request");
        assert_eq!(poll["pluginId"], "farol");
        assert_eq!(poll["method"], "farol.next");
        assert_eq!(poll["input"]["cursor"], 0);
    }

    #[test]
    fn a_snapshot_advances_the_cursor_and_polls_again() {
        let mut s = ready();
        let id = sent(&mut s)[0]["message"]["requestId"]
            .as_str()
            .unwrap()
            .to_string();
        s.take_notices();
        let reply = format!(
            r#"{{"type":"session","message":{{"type":"plugin.rpc.invoke.response","payload":{{"requestId":"{id}","output":{{"v":1,"rev":7,"now":1000,"agents":[]}}}}}}}}"#
        );
        s.on_text(30, &reply);
        assert!(matches!(s.take_notices()[0], Notice::Snapshot(ref snap) if snap.rev == 7));
        let next = &sent(&mut s)[0]["message"];
        assert_eq!(next["method"], "farol.next");
        assert_eq!(next["input"]["cursor"], 7);
    }

    #[test]
    fn board_settings_ride_the_polls_until_one_is_answered() {
        let mut s = ready();
        let first = sent(&mut s)[0]["message"]["requestId"]
            .as_str()
            .unwrap()
            .to_string();
        let prefs = BoardPrefs {
            sound: false,
            haptic: true,
            brightness: 70,
            dnd: false,
        };
        s.set_prefs(prefs);
        let focus = &sent(&mut s)[0]["message"];
        assert_eq!(focus["method"], "farol.focus");
        assert_eq!(focus["input"]["prefs"]["sound"], false);
        // The poll in flight was sent before the change: its answer confirms nothing.
        let answer = |id: &str, rev| {
            format!(
                r#"{{"type":"session","message":{{"type":"plugin.rpc.invoke.response","payload":{{"requestId":"{id}","output":{{"v":1,"rev":{rev},"now":1000}}}}}}}}"#
            )
        };
        s.on_text(30, &answer(&first, 1));
        assert!(s.prefs_pending());
        let next = sent(&mut s)
            .into_iter()
            .find(|f| f["message"]["method"] == "farol.next")
            .unwrap();
        assert_eq!(next["message"]["input"]["prefs"]["brightness"], 70);
        let id = next["message"]["requestId"].as_str().unwrap().to_string();
        s.on_text(40, &answer(&id, 2));
        assert!(!s.prefs_pending());
    }

    #[test]
    fn idle_answers_poll_again_without_a_snapshot() {
        let mut s = ready();
        let id = sent(&mut s)[0]["message"]["requestId"]
            .as_str()
            .unwrap()
            .to_string();
        s.take_notices();
        s.on_text(30, &format!(
            r#"{{"type":"session","message":{{"type":"plugin.rpc.invoke.response","payload":{{"requestId":"{id}","output":{{"v":1,"rev":0,"now":1,"idle":true}}}}}}}}"#
        ));
        assert!(s.take_notices().is_empty());
        assert_eq!(sent(&mut s).len(), 1);
    }

    #[test]
    fn a_missing_plugin_is_reported_and_retried_later() {
        let mut s = ready();
        let id = sent(&mut s)[0]["message"]["requestId"]
            .as_str()
            .unwrap()
            .to_string();
        s.on_text(30, &format!(
            r#"{{"type":"session","message":{{"type":"rpc_error","payload":{{"requestId":"{id}","error":"Request failed: Plugin is not available: farol","code":"handler_error"}}}}}}"#
        ));
        assert_eq!(s.down(), &Down::PluginMissing);
        s.poll(30 + PLUGIN_MISSING_RETRY_MS - 1);
        assert!(sent(&mut s).iter().all(|f| f["type"] == "ping"));
        s.on_text(30 + PLUGIN_MISSING_RETRY_MS, r#"{"type":"pong"}"#);
        s.poll(30 + PLUGIN_MISSING_RETRY_MS);
        assert!(
            sent(&mut s)
                .iter()
                .any(|f| f["message"]["method"] == "farol.next")
        );
    }

    #[test]
    fn a_refused_password_waits_long_and_says_why() {
        let mut s = Session::new("3a1f");
        s.configure(config(), 0);
        s.on_open(10, &[0; 64]);
        s.on_text(
            20,
            r#"{"type":"hello.rejected","reason":"incorrect_password","accepts":["password"]}"#,
        );
        s.on_close(21, Down::Closed(4401));
        assert_eq!(s.down(), &Down::Auth);
        assert_eq!(s.retry_at(), Some(21 + AUTH_RETRY_MS));
    }

    #[test]
    fn backoff_doubles_up_to_a_minute_and_resets_when_ready() {
        let mut s = Session::new("3a1f");
        s.configure(config(), 0);
        let mut now = 0;
        let mut waits = Vec::new();
        for _ in 0..8 {
            s.on_close(now, Down::Timeout);
            let until = s.retry_at().unwrap();
            waits.push(until - now);
            now = until;
            s.poll(now);
        }
        assert_eq!(
            waits,
            [1_000, 2_000, 4_000, 8_000, 16_000, 32_000, 60_000, 60_000]
        );
        s.on_open(now, &[0; 64]);
        s.on_text(now, SERVER_INFO);
        assert_eq!(s.failures(), 0);
    }

    #[test]
    fn pings_keep_alive_and_silence_closes() {
        let mut s = ready();
        s.take_out();
        s.poll(20 + PING_EVERY_MS);
        assert!(sent(&mut s).iter().any(|f| f["type"] == "ping"));
        s.on_text(25 + PING_EVERY_MS, r#"{"type":"pong"}"#);
        assert_eq!(s.rtt_ms(), Some(5));
        s.poll(25 + PING_EVERY_MS + SILENCE_LIMIT_MS + 1);
        assert!(s.take_out().contains(&Out::Close));
        assert!(!s.is_ready());
    }

    #[test]
    fn actions_round_trip_and_fail_on_disconnect() {
        let mut s = ready();
        s.take_out();
        let act = Act {
            agent: "a".into(),
            device: "farol-3a1f".into(),
            kind: ActKind::Answer,
            request_id: Some("r".into()),
            choice: Some(0),
            turn: None,
        };
        assert!(s.act(&act, 9));
        let frame = &sent(&mut s)[0]["message"];
        assert_eq!(frame["method"], "farol.act");
        assert_eq!(frame["input"]["choice"], 0);
        let id = frame["requestId"].as_str().unwrap().to_string();
        s.take_notices();
        s.on_text(40, &format!(
            r#"{{"type":"session","message":{{"type":"plugin.rpc.invoke.response","payload":{{"requestId":"{id}","output":{{"ok":true}}}}}}}}"#
        ));
        assert_eq!(
            s.take_notices(),
            [Notice::Acted {
                token: 9,
                result: ActResult {
                    ok: true,
                    ..Default::default()
                }
            }]
        );

        assert!(s.act(&act, 10));
        s.on_close(50, Down::Closed(1006));
        assert!(
            s.take_notices()
                .iter()
                .any(|n| matches!(n, Notice::Acted { token: 10, result } if !result.ok))
        );
        assert!(!s.act(&act, 11));
    }

    #[test]
    fn focus_changes_wake_the_plugin_once() {
        let mut s = ready();
        s.take_out();
        s.set_focus(Some("a-work"), "deck");
        s.set_focus(Some("a-work"), "deck");
        let frames = sent(&mut s);
        assert_eq!(frames.len(), 1);
        assert_eq!(frames[0]["message"]["method"], "farol.focus");
        assert_eq!(frames[0]["message"]["input"]["focus"], "a-work");
    }
}
