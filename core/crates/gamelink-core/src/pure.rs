//! Format and policy logic formerly implemented by the Go host.
use base64::{
    alphabet,
    engine::{general_purpose::GeneralPurpose, GeneralPurposeConfig},
    Engine,
};
use regex::Regex;
use serde_json::{json, Value};
use std::{net::IpAddr, sync::OnceLock};
#[path = "go_unicode.rs"]
mod unicode;
fn in_table(c: char, table: &[(u32, u32)]) -> bool {
    let c = c as u32;
    table
        .binary_search_by(|&(a, b)| {
            if c < a {
                std::cmp::Ordering::Greater
            } else if c > b {
                std::cmp::Ordering::Less
            } else {
                std::cmp::Ordering::Equal
            }
        })
        .is_ok()
}
fn trim(s: &str) -> &str {
    s.trim_matches(|c| in_table(c, unicode::SPACE))
}
fn quote(s: &str) -> String {
    let mut out = String::from("\"");
    for c in s.chars() {
        match c {
            '"' => out.push_str("\\\""),
            '\\' => out.push_str("\\\\"),
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            '\t' => out.push_str("\\t"),
            '\u{7}' => out.push_str("\\a"),
            '\u{8}' => out.push_str("\\b"),
            '\u{b}' => out.push_str("\\v"),
            '\u{c}' => out.push_str("\\f"),
            c if in_table(c, unicode::PRINT) => out.push(c),
            c if (c as u32) < 32 || c == '\u{7f}' => out.push_str(&format!("\\x{:02x}", c as u32)),
            c if (c as u32) < 65536 => out.push_str(&format!("\\u{:04x}", c as u32)),
            c => out.push_str(&format!("\\U{:08x}", c as u32)),
        }
    }
    out.push('"');
    out
}
fn text<'a>(v: &'a Value, k: &str) -> &'a str {
    v[k].as_str().unwrap_or("")
}
fn number(v: &Value, k: &str) -> i64 {
    v[k].as_i64().unwrap_or(0)
}
fn boolean(v: &Value, k: &str) -> bool {
    v[k].as_bool().unwrap_or(false)
}
fn port(p: i64) -> Result<(), String> {
    if !(1..=65535).contains(&p) {
        Err(format!("port {p} is out of range (1-65535)"))
    } else {
        Ok(())
    }
}
fn display(field: &str, s: &str, max: i64, required: bool) -> Result<(), String> {
    let s = trim(s);
    if s.is_empty() {
        return if required {
            Err(format!("{field} must not be empty"))
        } else {
            Ok(())
        };
    }
    let n = s.chars().count();
    if n as i64 > max {
        return Err(format!("{field} is too long ({n} characters, max {max})"));
    }
    for c in s.chars() {
        if in_table(c, unicode::CONTROL)
            || ('\u{202a}'..='\u{202e}').contains(&c)
            || ('\u{2066}'..='\u{2069}').contains(&c)
        {
            return Err(format!("{field} contains control characters"));
        }
        if !in_table(c, unicode::PRINT) && c != ' ' {
            return Err(format!("{field} contains unprintable characters"));
        }
    }
    Ok(())
}
fn slug(s: &str) -> Result<(), String> {
    let good = !s.is_empty()
        && s.len() <= 32
        && s.bytes()
            .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == b'-')
        && s.as_bytes()[0] != b'-'
        && s.as_bytes()[s.len() - 1] != b'-';
    if good {
        Ok(())
    } else {
        Err(format!(
            "{} is not a valid identifier (use 1-32 lowercase letters, digits or dashes)",
            quote(s)
        ))
    }
}
fn hostname(s: &str) -> Result<(), String> {
    if !s.is_empty()
        && s.len() <= 253
        && s.split('.').all(|p| {
            !p.is_empty()
                && p.len() <= 64
                && p.bytes().all(|c| c.is_ascii_alphanumeric() || c == b'-')
                && p.as_bytes()[0] != b'-'
                && p.as_bytes()[p.len() - 1] != b'-'
        })
    {
        Ok(())
    } else {
        Err(format!("{} is not a valid host name", quote(s)))
    }
}
fn ip(s: &str) -> Option<(IpAddr, &str)> {
    let (s, z) = match s.split_once('%') {
        Some((_, "")) => return None,
        Some(pair) => pair,
        None => (s, ""),
    };
    let p = s.parse::<IpAddr>().ok()?;
    if !z.is_empty() && !p.is_ipv6() {
        return None;
    }
    let p = match p {
        IpAddr::V6(v) => v.to_ipv4_mapped().map(IpAddr::V4).unwrap_or(p),
        _ => p,
    };
    Some((p, z))
}
fn public_ip(s: &str) -> bool {
    let Some((p, _)) = ip(s) else { return false };
    match p {
        IpAddr::V4(p) => {
            let b = p.octets();
            !(p.is_unspecified()
                || p.is_loopback()
                || p.is_private()
                || p.is_link_local()
                || p.is_multicast()
                || (b[0] == 100 && b[1] & 0xc0 == 64))
        }
        IpAddr::V6(p) => {
            let b = p.octets();
            !(p.is_unspecified()
                || p.is_loopback()
                || p.is_multicast()
                || b[0] & 0xfe == 0xfc
                || (b[0] == 0xfe && b[1] & 0xc0 == 0x80))
        }
    }
}
fn destination(s: &str) -> Result<String, String> {
    let malformed = || {
        format!(
            "destination {} must look like host:port (for example 127.0.0.1:25565)",
            quote(s)
        )
    };
    let t = trim(s);
    let (h, p) = if let Some(t) = t.strip_prefix('[') {
        let Some((h, p)) = t.split_once(']') else {
            return Err(malformed());
        };
        let Some(p) = p.strip_prefix(':') else {
            return Err(malformed());
        };
        (h, p)
    } else {
        let Some((h, p)) = t.rsplit_once(':') else {
            return Err(malformed());
        };
        if h.contains(':') {
            return Err(malformed());
        };
        (h, p)
    };
    if p.contains([':', '[', ']']) || h.contains(['[', ']']) {
        return Err(malformed());
    }
    let h = if h.eq_ignore_ascii_case("localhost") {
        "127.0.0.1"
    } else {
        h
    };
    let Some((a, z)) = ip(h) else {
        return Err(format!(
            "destination host {} must be an IP address (or localhost)",
            quote(h)
        ));
    };
    if a.is_unspecified() || a.is_multicast() {
        return Err("destination must be a specific unicast address".into());
    }
    let n = if !p.is_empty()
        && p.strip_prefix(['+', '-'])
            .unwrap_or(p)
            .bytes()
            .all(|b| b.is_ascii_digit())
    {
        p.parse::<i64>().ok()
    } else {
        None
    };
    let Some(n) = n else {
        return Err(format!("destination port {} is not a number", quote(p)));
    };
    port(n)?;
    Ok(match a {
        IpAddr::V4(a) => format!("{a}:{n}"),
        IpAddr::V6(a) => {
            if z.is_empty() {
                format!("[{a}]:{n}")
            } else {
                format!("[{a}%{z}]:{n}")
            }
        }
    })
}
fn public_key(s: &str) -> Result<String, String> {
    let s = trim(s);
    let Some(rest) = s.strip_prefix("nodekey:") else {
        return Err("public key must start with \"nodekey:\"".into());
    };
    if rest.len() != 64 {
        return Err(format!(
            "public key must be 64 hex characters after \"nodekey:\" (got {})",
            rest.len()
        ));
    }
    if !rest
        .bytes()
        .all(|c| c.is_ascii_digit() || (b'a'..=b'f').contains(&c))
    {
        return Err("public key must be lowercase hexadecimal".into());
    }
    if rest.bytes().all(|c| c == b'0') {
        return Err("public key must not be all zeros".into());
    }
    Ok(s.into())
}
const HOST: &str = "127.0.0.1";
fn games() -> Value {
    json!([
{"type":"minecraft-java","name":"Minecraft: Java Edition","protocol":"tcp","defaultPort":25565,"verified":"protocol","hint":"In Minecraft choose Multiplayer, then Add Server, and enter the local address shown here as the server address.","launchArgs":"","launchUri":""},
{"type":"minecraft-bedrock","name":"Minecraft: Bedrock Edition","protocol":"udp","defaultPort":19132,"verified":"protocol","hint":"In Minecraft Bedrock choose Play, Servers, Add Server and enter 127.0.0.1 with the port shown here. The Windows Store edition blocks connections to the same PC by default; a one-time loopback exemption may be required (see the docs). Bedrock on consoles and phones is not supported.","launchArgs":"","launchUri":"minecraft://connect?serverUrl={host}&serverPort={port}"},
{"type":"factorio","name":"Factorio","protocol":"udp","defaultPort":34197,"verified":"protocol","hint":"In Factorio choose Multiplayer, Connect to address, and enter the local address shown here.","launchArgs":"--mp-connect {address}","launchUri":""},
{"type":"custom","name":"Other game","protocol":"","defaultPort":0,"verified":"protocol","hint":"Point your game at the local address shown here.","launchArgs":"","launchUri":""}])
}
fn check_game(g: &str, p: &str) -> Result<(), String> {
    let all = games();
    let Some(g) = all.as_array().unwrap().iter().find(|v| v["type"] == g) else {
        return Err(format!("unknown game type {}", quote(g)));
    };
    if p != "tcp" && p != "udp" {
        return Err(format!("unknown protocol {} (use tcp or udp)", quote(p)));
    }
    if g["protocol"] != "" && g["protocol"] != p {
        return Err(format!(
            "{} uses {}, not {p}",
            text(g, "name"),
            text(g, "protocol")
        ));
    }
    Ok(())
}
fn redact(v: &Value) -> String {
    let mut s = text(v, "text").to_owned();
    if let Some(secrets) = v["secrets"].as_array() {
        for secret in secrets {
            if let Some(secret) = secret.as_str().filter(|s| s.len() >= 8) {
                s = s.replace(secret, "[redacted]")
            }
        }
    }
    static TOKENS: OnceLock<Regex> = OnceLock::new();
    TOKENS
        .get_or_init(|| {
            Regex::new(r"(?-u:\btc[A-Za-z0-9_-]{60,})|privkey:[0-9a-fA-F]{16,}").unwrap()
        })
        .replace_all(&s, "[redacted]")
        .into_owned()
}
fn invite_fields(v: &Value) -> Result<(), String> {
    let b = number(v, "consoleBridge");
    if !(0..=1).contains(&b) {
        return Err("console bridge version is not supported by this GameLink".into());
    }
    if b != 0 && v["game"] != "minecraft-bedrock" {
        return Err("console bridge requires Minecraft Bedrock".into());
    }
    display("name", text(v, "name"), 64, true)?;
    display("note", text(v, "note"), 200, false)?;
    check_game(text(v, "game"), text(v, "proto"))?;
    port(number(v, "port"))
}
fn b64() -> GeneralPurpose {
    GeneralPurpose::new(
        &alphabet::URL_SAFE,
        GeneralPurposeConfig::new()
            .with_encode_padding(false)
            .with_decode_padding_mode(base64::engine::DecodePaddingMode::RequireNone)
            .with_decode_allow_trailing_bits(true),
    )
}
// Keep duplicate fields in wire order, matching Go's struct decoder. A JSON
// map would silently reorder differently-cased duplicates before validation.
struct InviteEntries(Vec<(String, Value)>);
impl<'de> serde::Deserialize<'de> for InviteEntries {
    fn deserialize<D: serde::Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
        struct Visitor;
        impl<'de> serde::de::Visitor<'de> for Visitor {
            type Value = InviteEntries;
            fn expecting(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
                f.write_str("an invitation object")
            }
            fn visit_unit<E: serde::de::Error>(self) -> Result<Self::Value, E> {
                Ok(InviteEntries(Vec::new()))
            }
            fn visit_map<M: serde::de::MapAccess<'de>>(
                self,
                mut map: M,
            ) -> Result<Self::Value, M::Error> {
                let mut fields = Vec::new();
                while let Some(entry) = map.next_entry()? {
                    fields.push(entry)
                }
                Ok(InviteEntries(fields))
            }
        }
        d.deserialize_any(Visitor)
    }
}
// encoding/json replaces invalid UTF-8 and unpaired UTF-16 surrogates with
// U+FFFD. Preserve that behavior without weakening JSON syntax validation.
pub(crate) fn go_json(bytes: &[u8]) -> String {
    let s = String::from_utf8_lossy(bytes);
    let b = s.as_bytes();
    let mut out = Vec::with_capacity(b.len());
    let mut i = 0;
    let mut in_string = false;
    while i < b.len() {
        if b[i] == b'"' {
            in_string = !in_string;
            out.push(b[i]);
            i += 1;
            continue;
        }
        if in_string && b[i] == b'\\' && i + 1 < b.len() {
            if b[i + 1] == b'u' && i + 6 <= b.len() {
                let code = std::str::from_utf8(&b[i + 2..i + 6])
                    .ok()
                    .and_then(|s| u16::from_str_radix(s, 16).ok());
                if let Some(code) = code.filter(|c| (0xd800..=0xdfff).contains(c)) {
                    let low = if i + 12 <= b.len() && &b[i + 6..i + 8] == b"\\u" {
                        std::str::from_utf8(&b[i + 8..i + 12])
                            .ok()
                            .and_then(|s| u16::from_str_radix(s, 16).ok())
                    } else {
                        None
                    };
                    if code <= 0xdbff && low.is_some_and(|l| (0xdc00..=0xdfff).contains(&l)) {
                        out.extend_from_slice(&b[i..i + 12]);
                        i += 12;
                    } else {
                        out.extend_from_slice(b"\\ufffd");
                        i += 6;
                    }
                    continue;
                }
            }
            out.extend_from_slice(&b[i..i + 2]);
            i += 2;
            continue;
        }
        out.push(b[i]);
        i += 1;
    }
    String::from_utf8(out).expect("JSON normalization retains UTF-8")
}
fn decode_invite(s: &str) -> Result<Value, String> {
    let s: String = s
        .chars()
        .filter(|c| !matches!(c, ' ' | '\n' | '\r' | '\t'))
        .collect();
    if s.is_empty() {
        return Err("paste an invitation first".into());
    }
    if s.len() > 8192 {
        return Err("invitation is too long to be valid".into());
    }
    let Some(body) = s.strip_prefix("glink1.") else {
        return Err(
            "this does not look like a GameLink invitation (it should start with \"glink1.\")"
                .into(),
        );
    };
    let bytes = b64().decode(body.trim_end_matches('=')).map_err(|_| {
        "invitation is damaged (could not decode it); ask for a fresh copy".to_owned()
    })?;
    let damaged = || "invitation is damaged or from an incompatible version".to_owned();
    let normalized = go_json(&bytes);
    let mut stream = serde_json::Deserializer::from_str(&normalized).into_iter::<InviteEntries>();
    let raw = stream.next().ok_or_else(damaged)?.map_err(|_| damaged())?;
    let tail = &normalized.as_bytes()[stream.byte_offset()..];
    if let Some(b) = tail
        .iter()
        .find(|b| !matches!(b, b' ' | b'\t' | b'\r' | b'\n'))
    {
        if !matches!(b, b']' | b'}') {
            return Err("invitation has unexpected trailing data".into());
        }
    }
    let mut v = json!({"v":0,"name":"","game":"","proto":"","port":0,"addr":""});
    {
        for (k, x) in raw.0 {
            let key = match k.to_ascii_lowercase().as_str() {
                "v" => "v",
                "name" => "name",
                "game" => "game",
                "proto" => "proto",
                "port" => "port",
                "addr" => "addr",
                "note" => "note",
                "consolebridge" => "consoleBridge",
                _ => return Err(damaged()),
            };
            if x.is_null() {
                continue;
            }
            if matches!(key, "v" | "consoleBridge" | "port") {
                let n = x.as_i64().ok_or_else(damaged)?;
                if key == "port" && !(0..=65535).contains(&n) {
                    return Err(damaged());
                }
            } else if !x.is_string() {
                return Err(damaged());
            }
            v[key] = x.clone();
        }
    }
    if number(&v, "v") != 1 {
        return Err(format!(
            "invitation version {} is not supported by this GameLink",
            number(&v, "v")
        ));
    }
    invite_fields(&v)?;
    Ok(v)
}
fn encode_invite(v: &Value) -> String {
    let mut fields = Vec::new();
    for k in [
        "v",
        "name",
        "game",
        "proto",
        "port",
        "addr",
        "note",
        "consoleBridge",
    ] {
        if k == "note" && text(v, k).is_empty() || k == "consoleBridge" && number(v, k) == 0 {
            continue;
        }
        let value = if k == "v" { json!(1) } else { v[k].clone() };
        fields.push(format!("\"{k}\":{value}"));
    }
    let raw = format!("{{{}}}", fields.join(","))
        .replace('&', "\\u0026")
        .replace('<', "\\u003c")
        .replace('>', "\\u003e")
        .replace('\u{2028}', "\\u2028")
        .replace('\u{2029}', "\\u2029");
    format!("glink1.{}", b64().encode(raw))
}
fn relay(v: &Value) -> Result<Value, String> {
    if !boolean(v, "serverKeyPresent") {
        return Err("the server address has no server key".into());
    }
    if !boolean(v, "presharedKeyPresent") {
        return Err("this address has no pre-shared key, which GameLink requires; generate the invitation with gamelink-agent".into());
    }
    let regions = v["regions"].as_array().filter(|r| r.len() == 1);
    let reg = regions.and_then(|r| r.first()).filter(|r| !r.is_null());
    let nodes = reg
        .and_then(|r| r["Nodes"].as_array())
        .filter(|r| !r.is_empty())
        .ok_or(
            "the address must include relay details; generate the invitation with gamelink-agent",
        )?;
    if nodes.len() > 8 {
        return Err("the address lists too many relay nodes".into());
    }
    let mut hosts = Vec::new();
    let mut default = true;
    for n in nodes {
        if n.is_null() {
            return Err("the address has a malformed relay entry".into());
        }
        let host = text(n, "HostName");
        hostname(host).map_err(|e| format!("relay host: {e}"))?;
        let cert = text(n, "CertName");
        if !cert.is_empty() && !cert.starts_with("sha256-raw:") {
            hostname(cert).map_err(|e| format!("relay certificate name: {e}"))?
        }
        let p = number(n, "DERPPort");
        if !(0..=65535).contains(&p) {
            return Err("relay port is out of range".into());
        }
        if !boolean(v, "allowLocalRelay") {
            if boolean(n, "InsecureForTests") {
                return Err(
                    "the address asks for an insecure relay connection and was rejected".into(),
                );
            }
            if p != 0 && p != 443 {
                return Err("the address uses a non-standard relay port and was rejected".into());
            }
            for s in [text(n, "IPv4"), text(n, "IPv6")] {
                if !s.is_empty() && s != "none" && !public_ip(s) {
                    return Err(
                        "the address points the relay at a private or invalid IP and was rejected"
                            .into(),
                    );
                }
            }
            if ip(host).is_some() && !public_ip(host) {
                return Err("the address names a private relay IP and was rejected".into());
            }
        }
        if !hosts.contains(&host) {
            hosts.push(host)
        }
        if ![".tailcat.dev", ".ipn.dev", ".tailscale.com"]
            .iter()
            .any(|s| host.ends_with(s))
        {
            default = false
        }
    }
    Ok(
        json!({"relayHosts":hosts,"relayRegion":text(reg.unwrap(),"RegionName"),"serverKey":text(v,"serverKey").chars().take(16).collect::<String>(),"defaultRelay":default}),
    )
}
/// Dispatch a pure control-plane operation. Domain errors preserve the Go API text.
pub fn dispatch(method: &str, v: Value) -> Result<Value, String> {
    match method {
        "security.port" => {
            if !boolean(&v, "optional") || number(&v, "port") != 0 {
                port(number(&v, "port"))?
            }
            Ok(Value::Null)
        }
        "security.slug" => {
            slug(text(&v, "text"))?;
            Ok(Value::Null)
        }
        "security.hostname" => {
            hostname(text(&v, "text"))?;
            Ok(Value::Null)
        }
        "security.display" => {
            display(
                text(&v, "field"),
                text(&v, "text"),
                number(&v, "maxRunes"),
                boolean(&v, "required"),
            )?;
            Ok(Value::Null)
        }
        "security.destination" => Ok(json!(destination(text(&v, "text"))?)),
        "security.public_ip" => Ok(json!(public_ip(text(&v, "text")))),
        "identity.public_key" => Ok(json!(public_key(text(&v, "text"))?)),
        "games.all" => Ok(games()),
        "games.valid_protocol" => Ok(json!(matches!(text(&v, "proto"), "tcp" | "udp"))),
        "games.check" => {
            check_game(text(&v, "game"), text(&v, "proto"))?;
            Ok(Value::Null)
        }
        "games.expand" => Ok(json!(text(&v, "text")
            .replace("{host}", HOST)
            .replace("{port}", &number(&v, "port").to_string())
            .replace("{address}", &format!("{HOST}:{}", number(&v, "port"))))),
        "games.placeholder" => Ok(json!(["{host}", "{port}", "{address}"]
            .iter()
            .any(|p| text(&v, "text").contains(p)))),
        "security.redact" => Ok(json!(redact(&v))),
        "security.redact_add" => Ok(json!(v["secrets"]
            .as_array()
            .map(|a| a
                .iter()
                .filter(|s| s.as_str().is_some_and(|s| s.len() >= 8))
                .cloned()
                .collect::<Vec<_>>())
            .unwrap_or_default())),
        "invite.decode" => decode_invite(text(&v, "text")),
        "invite.encode" => Ok(json!(encode_invite(&v))),
        "invite.fields" => {
            invite_fields(&v)?;
            Ok(Value::Null)
        }
        "invite.address_format" => {
            let s = trim(text(&v, "text"));
            if s.is_empty() {
                return Err("the server address is empty".into());
            }
            if s.len() > 2048
                || !s.starts_with("tc")
                || s.len() == 2
                || !s[2..]
                    .bytes()
                    .all(|c| c.is_ascii_alphanumeric() || c == b'_' || c == b'-')
            {
                return Err("the server address is not a valid Tailcat address".into());
            }
            Ok(json!(s))
        }
        "invite.relay" => relay(&v),
        _ => Err("unknown pure method".into()),
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn go_fixtures() {
        let cases: Value =
            serde_json::from_str(include_str!("../../../fixtures/pure.json")).unwrap();
        for c in cases.as_array().unwrap() {
            let got = dispatch(c["method"].as_str().unwrap(), c["input"].clone());
            if c["error"] != "" {
                assert_eq!(got.unwrap_err(), c["error"], "{c}")
            } else {
                assert_eq!(got.unwrap(), c["value"], "{c}")
            }
        }
    }
}
