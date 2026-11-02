//! The QR codes the board understands.
//!
//! - Wi-Fi share codes, as phones show them: `WIFI:T:WPA;S:<ssid>;P:<password>;;`
//!   with `\` escaping `;`, `,`, `:`, `"` and `\`.
//! - Paseo's pairing codes ([`crate::endpoint::parse_link`]): the QR under
//!   Settings › host › 配对设备, and the `relay://` links `paseo daemon pair`
//!   prints.

use alloc::string::String;
use alloc::vec::Vec;

use crate::endpoint::{self, Config};

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Code {
    Wifi { ssid: String, password: String },
    Paseo(Config),
}

pub fn parse(payload: &str) -> Option<Code> {
    let payload = payload.trim();
    if payload.len() >= 5 && payload[..5].eq_ignore_ascii_case("WIFI:") {
        return wifi(&payload[5..]);
    }
    endpoint::parse_link(payload).map(Code::Paseo)
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

#[cfg(test)]
mod tests {
    use super::*;

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
    fn paseo_pairing_codes_and_nothing_else() {
        let v: serde_json::Value =
            serde_json::from_str(include_str!("../../../protocol/fixtures/relay.json")).unwrap();
        let offer = v["links"]["offerUrl"].as_str().unwrap();
        assert!(matches!(parse(offer), Some(Code::Paseo(c)) if c.relay().is_some()));
        assert_eq!(parse("farol://connect?u=wss%3A%2F%2Fx"), None);
        assert_eq!(parse("https://example.com"), None);
    }
}
