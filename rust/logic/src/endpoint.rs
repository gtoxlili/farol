//! Where the board finds its Paseo daemon, in the two ways the Paseo app
//! offers when adding a host:
//!
//! - direct connection (local network or VPN): host, port, SSL and the daemon
//!   password, typed on the board; the session opens `ws[s]://host:port/ws`;
//! - encrypted relay: Paseo's own pairing QR (Settings › host › 配对设备), read
//!   with the camera; the session opens the relay and runs the handshake in
//!   [`crate::relay`] against the daemon key the QR carries.
//!
//! Paseo's pairing links (Paseo 0.14, `packages/protocol/src/`):
//! `https://app.paseo.sh/#offer=<base64url JSON>` (the QR),
//! `relay://host:port/<serverId>?key=…[&ssl=true][&security=protected][&password=…]`
//! (`paseo daemon pair` prints it), and `…#connect=<encoded relay://>`.

use alloc::borrow::ToOwned;
use alloc::format;
use alloc::string::{String, ToString};
use alloc::vec::Vec;

use serde_json::{Value, json};

use crate::relay;

/// The Paseo daemon's default port, as in the app's form.
pub const DEFAULT_PORT: u16 = 6767;
/// Saved record versions: 1 was firmware 1.0's (URL, password); 2 is this.
const RECORD_V1: u8 = 1;
const RECORD_V2: u8 = 2;

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Endpoint {
    /// Straight to the daemon: `ws[s]://host:port/ws`.
    Direct { host: String, port: u16, tls: bool },
    /// Through a Paseo relay, end-to-end encrypted to the daemon's key.
    Relay(Relay),
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Relay {
    /// The daemon's id at the relay (`srv_…`).
    pub server_id: String,
    /// The daemon's static X25519 public key.
    pub daemon_key: [u8; 32],
    /// The relay as `host:port`, e.g. `relay.paseo.sh:443`.
    pub endpoint: String,
    pub tls: bool,
}

/// Where to connect and with which daemon password (empty = none).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Config {
    pub endpoint: Endpoint,
    pub password: String,
}

/// `host:port`, with an IPv6 host in brackets.
pub fn host_port(host: &str, port: u16) -> String {
    if host.contains(':') && !host.starts_with('[') {
        format!("[{host}]:{port}")
    } else {
        format!("{host}:{port}")
    }
}

impl Config {
    /// The WebSocket URL the session opens.
    pub fn url(&self) -> String {
        match &self.endpoint {
            Endpoint::Direct { host, port, tls } => {
                let scheme = if *tls { "wss" } else { "ws" };
                format!("{scheme}://{}/ws", host_port(host, *port))
            }
            Endpoint::Relay(r) => {
                let scheme = if r.tls { "wss" } else { "ws" };
                format!(
                    "{scheme}://{}/ws?serverId={}&role=client&v=2",
                    r.endpoint,
                    form_encode(&r.server_id)
                )
            }
        }
    }

    pub fn relay(&self) -> Option<&Relay> {
        match &self.endpoint {
            Endpoint::Relay(r) => Some(r),
            Endpoint::Direct { .. } => None,
        }
    }

    /// One line for the screen: `ws://192.168.1.20:6767` or `relay.paseo.sh · srv_…`.
    pub fn describe(&self) -> String {
        match &self.endpoint {
            Endpoint::Direct { host, port, tls } => {
                let scheme = if *tls { "wss" } else { "ws" };
                format!("{scheme}://{}", host_port(host, *port))
            }
            Endpoint::Relay(r) => {
                let host = r
                    .endpoint
                    .rsplit_once(':')
                    .map_or(r.endpoint.as_str(), |(h, _)| h);
                format!("{host} · {}", r.server_id)
            }
        }
    }

    /// A name for this Paseo before it has told its own: the host, or the
    /// relay id.
    pub fn fallback_name(&self) -> String {
        match &self.endpoint {
            Endpoint::Direct { host, .. } => host.clone(),
            Endpoint::Relay(r) => r.server_id.clone(),
        }
    }

    /// The saved form (NVS on the board).
    pub fn encode(&self) -> Vec<u8> {
        let endpoint = match &self.endpoint {
            Endpoint::Direct { host, port, tls } => {
                json!({"direct": {"host": host, "port": port, "tls": tls}})
            }
            Endpoint::Relay(r) => json!({"relay": {
                "serverId": r.server_id,
                "key": relay::encode(&r.daemon_key),
                "endpoint": r.endpoint,
                "tls": r.tls,
            }}),
        };
        let mut record = endpoint;
        record["password"] = json!(self.password);
        let mut out = alloc::vec![RECORD_V2];
        out.extend_from_slice(record.to_string().as_bytes());
        out
    }

    /// Reads [`Config::encode`]'s form, and firmware 1.0's (a WebSocket URL
    /// and the password) as a direct connection.
    pub fn decode(bytes: &[u8]) -> Option<Self> {
        match bytes.split_first()? {
            (&RECORD_V2, json) => {
                let v: Value = serde_json::from_slice(json).ok()?;
                let password = v["password"].as_str()?.to_owned();
                let endpoint = if let Some(d) = v.get("direct") {
                    Endpoint::Direct {
                        host: d["host"].as_str().filter(|h| !h.is_empty())?.to_owned(),
                        port: u16::try_from(d["port"].as_u64()?).ok().filter(|p| *p > 0)?,
                        tls: d["tls"].as_bool()?,
                    }
                } else {
                    let r = &v["relay"];
                    Endpoint::Relay(Relay {
                        server_id: r["serverId"].as_str().filter(|s| !s.is_empty())?.to_owned(),
                        daemon_key: key(r["key"].as_str()?)?,
                        endpoint: r["endpoint"].as_str().filter(|e| !e.is_empty())?.to_owned(),
                        tls: r["tls"].as_bool()?,
                    })
                };
                Some(Self { endpoint, password })
            }
            (&RECORD_V1, mut rest) => {
                let url = take_field(&mut rest)?;
                let password = take_field(&mut rest)?.to_owned();
                if !rest.is_empty() {
                    return None;
                }
                let (tls, rest) = if let Some(r) = url.strip_prefix("wss://") {
                    (true, r)
                } else {
                    (false, url.strip_prefix("ws://")?)
                };
                let authority = rest.split(['/', '?']).next().unwrap_or("");
                let (host, port) = split_host_port(authority)?;
                let port = port.unwrap_or(if tls { 443 } else { 80 });
                Some(Self {
                    endpoint: Endpoint::Direct { host, port, tls },
                    password,
                })
            }
            _ => None,
        }
    }
}

/// Firmware 1.0's field: u16 little-endian length, then UTF-8.
fn take_field<'a>(bytes: &mut &'a [u8]) -> Option<&'a str> {
    let (length, rest) = bytes.split_first_chunk::<2>()?;
    let (field, rest) = rest.split_at_checked(usize::from(u16::from_le_bytes(*length)))?;
    *bytes = rest;
    core::str::from_utf8(field).ok()
}

fn key(b64: &str) -> Option<[u8; 32]> {
    relay::decode(b64)?.try_into().ok()
}

/// `host`, `host:port`, `[v6]:port` or a bare IPv6 address.
fn split_host_port(text: &str) -> Option<(String, Option<u16>)> {
    if let Some(rest) = text.strip_prefix('[') {
        let (host, after) = rest.split_once(']')?;
        let port = match after.strip_prefix(':') {
            Some(p) => Some(port(p)?),
            None if after.is_empty() => None,
            None => return None,
        };
        return (!host.is_empty()).then(|| (host.to_owned(), port));
    }
    match text.matches(':').count() {
        0 => (!text.is_empty()).then(|| (text.to_owned(), None)),
        1 => {
            let (host, port) = text.split_once(':')?;
            (!host.is_empty()).then_some(())?;
            Some((host.to_owned(), Some(self::port(port)?)))
        }
        // More than one colon and no brackets: an IPv6 address, as in the app.
        _ => Some((text.to_owned(), None)),
    }
}

// ------------------------------------------------------------------ typed

/// The Host step: trims, and takes what people paste (`https://host:8443/…`).
/// Returns the host and the port if one came with it.
pub fn parse_host(text: &str) -> Result<(String, Option<u16>), &'static str> {
    let text = text.trim();
    let text = text.split_once("://").map_or(text, |(_, rest)| rest);
    let authority = text.split(['/', '?', '#']).next().unwrap_or("");
    if authority.is_empty() {
        return Err("Host 必填");
    }
    if authority.contains(char::is_whitespace) || authority.contains('@') {
        return Err("Host 里不能有空格或 @");
    }
    split_host_port(authority).ok_or("这不是一个有效的 Host")
}

/// The Port step; an empty field takes `default`.
pub fn parse_port(text: &str, default: u16) -> Result<u16, &'static str> {
    let text = text.trim();
    if text.is_empty() {
        return Ok(default);
    }
    port(text).ok_or("端口必须在 1 到 65535 之间")
}

/// A port number, 1 to 65535.
fn port(text: &str) -> Option<u16> {
    text.parse::<u16>().ok().filter(|p| *p > 0)
}

// ------------------------------------------------------------------ scanned

/// A Paseo pairing link from the camera, or `None` when this is not one.
/// A `relay://` link may carry the password; the others never do.
pub fn parse_link(text: &str) -> Option<Config> {
    let text = text.trim();
    if let Some(at) = text.find("#offer=") {
        return offer(&text[at + "#offer=".len()..]);
    }
    if let Some(at) = text.find("#connect=") {
        return relay_uri(&percent_decode(&text[at + "#connect=".len()..], false)?);
    }
    if text.len() > 8 && text[..8].eq_ignore_ascii_case("relay://") {
        return relay_uri(text);
    }
    None
}

fn offer(encoded: &str) -> Option<Config> {
    let json = relay::decode(encoded.trim())?;
    let v: Value = serde_json::from_slice(&json).ok()?;
    if v["v"].as_u64() != Some(2) {
        return None;
    }
    let endpoint = v["relay"]["endpoint"].as_str().filter(|e| !e.is_empty())?;
    let (_, port) = split_host_port(endpoint)?;
    // Offers from before explicit TLS: TLS on port 443 (Paseo's migration rule).
    let tls = v["relay"]["useTls"].as_bool().unwrap_or(port == Some(443));
    Some(Config {
        endpoint: Endpoint::Relay(Relay {
            server_id: v["serverId"].as_str().filter(|s| !s.is_empty())?.to_owned(),
            daemon_key: key(v["daemonPublicKeyB64"].as_str()?)?,
            endpoint: endpoint.to_owned(),
            tls,
        }),
        password: String::new(),
    })
}

fn relay_uri(uri: &str) -> Option<Config> {
    if !uri.get(..8)?.eq_ignore_ascii_case("relay://") {
        return None;
    }
    let rest = &uri[8..];
    let (authority, rest) = rest.split_once('/')?;
    if authority.contains('@') {
        return None;
    }
    let (host, port) = split_host_port(authority)?;
    let endpoint = host_port(&host, port?);
    let (path, query) = rest.split_once('?').unwrap_or((rest, ""));
    let server_id = percent_decode(path, false)?;
    if server_id.is_empty() || server_id.contains('/') {
        return None;
    }
    let (mut daemon_key, mut tls, mut password) = (None, false, String::new());
    for pair in query.split('&').filter(|p| !p.is_empty()) {
        let (name, value) = pair.split_once('=').unwrap_or((pair, ""));
        let value = percent_decode(value, true)?;
        match name {
            "key" => daemon_key = Some(key(&value)?),
            "ssl" => tls = value == "true",
            "security" if value != "protected" => return None,
            "password" => password = value,
            _ => {}
        }
    }
    Some(Config {
        endpoint: Endpoint::Relay(Relay {
            server_id,
            daemon_key: daemon_key?,
            endpoint,
            tls,
        }),
        password,
    })
}

/// `%XX` escapes; `+` is a space in query values (URLSearchParams).
fn percent_decode(text: &str, plus_is_space: bool) -> Option<String> {
    let bytes = text.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut i = 0;
    while i < bytes.len() {
        match bytes[i] {
            b'%' => {
                out.push(u8::from_str_radix(text.get(i + 1..i + 3)?, 16).ok()?);
                i += 3;
            }
            b'+' if plus_is_space => {
                out.push(b' ');
                i += 1;
            }
            b => {
                out.push(b);
                i += 1;
            }
        }
    }
    String::from_utf8(out).ok()
}

/// A query value as URLSearchParams writes it.
fn form_encode(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    for b in text.bytes() {
        if b.is_ascii_alphanumeric() || matches!(b, b'-' | b'_' | b'.' | b'*') {
            out.push(char::from(b));
        } else if b == b' ' {
            out.push('+');
        } else {
            out.push_str(&format!("%{b:02X}"));
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn links() -> Value {
        let v: Value =
            serde_json::from_str(include_str!("../../../protocol/fixtures/relay.json")).unwrap();
        v["links"].clone()
    }

    fn vector_key() -> [u8; 32] {
        let v: Value =
            serde_json::from_str(include_str!("../../../protocol/fixtures/relay.json")).unwrap();
        key(v["daemonKeyB64"].as_str().unwrap()).unwrap()
    }

    fn official(password: &str) -> Config {
        Config {
            endpoint: Endpoint::Relay(Relay {
                server_id: "srv_AAAAAAAAAAAA".into(),
                daemon_key: vector_key(),
                endpoint: "relay.paseo.sh:443".into(),
                tls: true,
            }),
            password: password.into(),
        }
    }

    #[test]
    fn paseos_pairing_qr_becomes_a_relay_endpoint() {
        let l = links();
        for url in ["offerUrl", "pinnedUrl"] {
            assert_eq!(
                parse_link(l[url].as_str().unwrap()),
                Some(official("")),
                "{url}"
            );
        }
        assert_eq!(
            official("").url().replace(":443", ""),
            l["relayWebSocketUrl"].as_str().unwrap()
        );
        assert_eq!(official("").describe(), "relay.paseo.sh · srv_AAAAAAAAAAAA");
    }

    #[test]
    fn relay_links_carry_the_password_and_tls_as_paseo_writes_them() {
        let l = links();
        let password = l["password"].as_str().unwrap();
        assert_eq!(
            parse_link(l["relayUri"].as_str().unwrap()),
            Some(official(password))
        );
        assert_eq!(
            parse_link(l["connectUrl"].as_str().unwrap()),
            Some(official(password))
        );
        let plain = parse_link(l["plainUri"].as_str().unwrap()).unwrap();
        assert_eq!(
            plain.url(),
            "ws://192.168.1.20:8080/ws?serverId=srv_AAAAAAAAAAAA&role=client&v=2"
        );
    }

    #[test]
    fn other_codes_are_not_pairing_links() {
        assert_eq!(parse_link("https://example.com"), None);
        assert_eq!(parse_link("https://app.paseo.sh/#offer=bm90IGpzb24"), None);
        assert_eq!(parse_link("relay://relay.paseo.sh:443/srv_x"), None);
        assert_eq!(
            parse_link("relay://relay.paseo.sh:443/srv_x?key=AAAA&security=other"),
            None
        );
        assert_eq!(parse_link("WIFI:S:x;;"), None);
    }

    #[test]
    fn direct_endpoints_make_the_apps_urls() {
        let direct = |host: &str, port, tls| Config {
            endpoint: Endpoint::Direct {
                host: host.into(),
                port,
                tls,
            },
            password: String::new(),
        };
        assert_eq!(
            direct("192.168.1.20", 6767, false).url(),
            "ws://192.168.1.20:6767/ws"
        );
        assert_eq!(
            direct("paseo.example.net", 443, true).url(),
            "wss://paseo.example.net:443/ws"
        );
        assert_eq!(
            direct("fd00::1", 6767, false).url(),
            "ws://[fd00::1]:6767/ws"
        );
    }

    #[test]
    fn typed_hosts_and_ports() {
        assert_eq!(
            parse_host(" 192.168.1.20 "),
            Ok(("192.168.1.20".into(), None))
        );
        assert_eq!(
            parse_host("https://paseo.example.net:8443/ws"),
            Ok(("paseo.example.net".into(), Some(8443)))
        );
        assert_eq!(parse_host("fd00::1"), Ok(("fd00::1".into(), None)));
        assert_eq!(
            parse_host("[fd00::1]:7000"),
            Ok(("fd00::1".into(), Some(7000)))
        );
        assert!(parse_host("").is_err());
        assert!(parse_host("a b").is_err());
        assert_eq!(parse_port("6767", 1), Ok(6767));
        assert_eq!(parse_port(" ", DEFAULT_PORT), Ok(DEFAULT_PORT));
        assert!(parse_port("0", 1).is_err());
        assert!(parse_port("70000", 1).is_err());
        assert_eq!(parse_host("example.net:"), Err("这不是一个有效的 Host"));
    }

    #[test]
    fn saved_configs_round_trip_and_firmware_1_records_still_load() {
        let direct = Config {
            endpoint: Endpoint::Direct {
                host: "paseo.example.net".into(),
                port: 8443,
                tls: true,
            },
            password: "pw".into(),
        };
        for c in [direct, official("s3cret")] {
            assert_eq!(Config::decode(&c.encode()), Some(c));
        }
        let mut v1 = alloc::vec![RECORD_V1];
        for field in ["wss://paseo.example.net/ws", "pw"] {
            v1.extend_from_slice(&u16::try_from(field.len()).unwrap().to_le_bytes());
            v1.extend_from_slice(field.as_bytes());
        }
        assert_eq!(
            Config::decode(&v1),
            Some(Config {
                endpoint: Endpoint::Direct {
                    host: "paseo.example.net".into(),
                    port: 443,
                    tls: true
                },
                password: "pw".into(),
            })
        );
        assert_eq!(Config::decode(&[9, 1, 2]), None);
    }
}
