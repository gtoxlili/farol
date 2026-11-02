//! The QR codes the board understands.
//!
//! - Wi-Fi share codes, as phones show them: `WIFI:T:WPA;S:<ssid>;P:<password>;;`
//!   with `\` escaping `;`, `,`, `:`, `"` and `\`.
//! - Farol connection codes from the plugin's settings page:
//!   `farol://connect?u=<url>&p=<password>&n=<host>`, each value
//!   percent-encoded. `p` and `n` are optional.

use alloc::string::String;
use alloc::vec::Vec;

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Code {
    Wifi {
        ssid: String,
        password: String,
    },
    Farol {
        url: String,
        password: String,
        host: String,
    },
}

pub fn parse(payload: &str) -> Option<Code> {
    let payload = payload.trim();
    if payload.len() >= 5 && payload[..5].eq_ignore_ascii_case("WIFI:") {
        return wifi(&payload[5..]);
    }
    if let Some(query) = payload.strip_prefix("farol://connect?") {
        return farol(query);
    }
    None
}

fn wifi(body: &str) -> Option<Code> {
    let mut fields: Vec<(char, String)> = Vec::new();
    let mut chars = body.chars().peekable();
    loop {
        let key = chars.next()?;
        if key == ';' {
            break;
        }
        if chars.next()? != ':' {
            return None;
        }
        let mut value = String::new();
        loop {
            match chars.next() {
                Some('\\') => value.push(chars.next()?),
                Some(';') | None => break,
                Some(c) => value.push(c),
            }
        }
        fields.push((key.to_ascii_uppercase(), value));
        if chars.peek().is_none_or(|&c| c == ';') {
            break;
        }
    }
    let get = |k: char| {
        fields
            .iter()
            .find(|(key, _)| *key == k)
            .map(|(_, v)| v.clone())
    };
    let ssid = get('S').filter(|s| !s.is_empty())?;
    let open = get('T').is_some_and(|t| t.eq_ignore_ascii_case("nopass"));
    let password = if open {
        String::new()
    } else {
        get('P').unwrap_or_default()
    };
    Some(Code::Wifi { ssid, password })
}

/// A daemon address typed by hand, made into the WebSocket URL the session
/// opens: `https://` becomes `wss://`, `http://` `ws://`, no scheme means
/// `wss://`, and an address without a path gets Paseo's `/ws`.
pub fn normalize_url(text: &str) -> Result<String, &'static str> {
    let text = text.trim();
    let (scheme, rest) = match text.split_once("://") {
        Some((s, r)) if s.eq_ignore_ascii_case("wss") || s.eq_ignore_ascii_case("https") => {
            ("wss", r)
        }
        Some((s, r)) if s.eq_ignore_ascii_case("ws") || s.eq_ignore_ascii_case("http") => ("ws", r),
        Some(_) => return Err("地址要以 wss:// 或 ws:// 开头"),
        None => ("wss", text),
    };
    let host = rest.split(['/', '?']).next().unwrap_or("");
    if host.is_empty() || host.contains(char::is_whitespace) {
        return Err("地址里缺少主机名");
    }
    let path = &rest[host.len()..];
    let path = if path.is_empty() || path == "/" {
        "/ws"
    } else {
        path
    };
    Ok(alloc::format!("{scheme}://{host}{path}"))
}

/// The host part of a WebSocket URL, for showing which Paseo this is.
pub fn host_of(url: &str) -> String {
    let rest = url.split_once("://").map_or(url, |(_, r)| r);
    let authority = rest.split(['/', '?']).next().unwrap_or("");
    let host = match authority.rsplit_once(':') {
        Some((h, port)) if port.chars().all(|c| c.is_ascii_digit()) => h,
        _ => authority,
    };
    String::from(host)
}

fn farol(query: &str) -> Option<Code> {
    let (mut url, mut password, mut host) = (None, String::new(), String::new());
    for pair in query.split('&') {
        let (key, value) = pair.split_once('=')?;
        let value = percent_decode(value)?;
        match key {
            "u" => url = Some(value),
            "p" => password = value,
            "n" => host = value,
            _ => {}
        }
    }
    let url = url?;
    let ok = url.starts_with("ws://") || url.starts_with("wss://");
    ok.then_some(Code::Farol {
        url,
        password,
        host,
    })
}

fn percent_decode(s: &str) -> Option<String> {
    let bytes = s.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut i = 0;
    while i < bytes.len() {
        match bytes[i] {
            b'%' => {
                let hex = s.get(i + 1..i + 3)?;
                out.push(u8::from_str_radix(hex, 16).ok()?);
                i += 3;
            }
            b'+' => {
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn typed_addresses_become_websocket_urls() {
        assert_eq!(
            normalize_url(" paseo.example.net ").as_deref(),
            Ok("wss://paseo.example.net/ws")
        );
        assert_eq!(
            normalize_url("https://paseo.example.net:8443/").as_deref(),
            Ok("wss://paseo.example.net:8443/ws")
        );
        assert_eq!(
            normalize_url("http://192.0.2.7:6767").as_deref(),
            Ok("ws://192.0.2.7:6767/ws")
        );
        assert_eq!(
            normalize_url("wss://p.example.net/custom/ws").as_deref(),
            Ok("wss://p.example.net/custom/ws")
        );
        assert!(normalize_url("ftp://x").is_err());
        assert!(normalize_url("wss:///ws").is_err());
        assert_eq!(
            host_of("wss://paseo.example.net:8443/ws"),
            "paseo.example.net"
        );
    }

    #[test]
    fn wifi_codes_with_escapes() {
        assert_eq!(
            parse(r"WIFI:T:WPA;S:My\;Net;P:pa\:ss\\word;H:false;;"),
            Some(Code::Wifi {
                ssid: "My;Net".into(),
                password: r"pa:ss\word".into()
            })
        );
        assert_eq!(
            parse("WIFI:S:Cafe;T:nopass;P:ignored;;"),
            Some(Code::Wifi {
                ssid: "Cafe".into(),
                password: String::new()
            })
        );
        assert_eq!(
            parse("wifi:S:家里;P:12345678;;"),
            Some(Code::Wifi {
                ssid: "家里".into(),
                password: "12345678".into()
            })
        );
        assert_eq!(parse("WIFI:T:WPA;P:x;;"), None);
    }

    #[test]
    fn farol_codes() {
        assert_eq!(
            parse("farol://connect?u=wss%3A%2F%2Fpaseo.example.net%2Fws&p=s%26cret&n=Studio%20Mac"),
            Some(Code::Farol {
                url: "wss://paseo.example.net/ws".into(),
                password: "s&cret".into(),
                host: "Studio Mac".into()
            })
        );
        assert_eq!(parse("farol://connect?u=http%3A%2F%2Fx"), None);
        assert_eq!(parse("https://example.com"), None);
    }
}
