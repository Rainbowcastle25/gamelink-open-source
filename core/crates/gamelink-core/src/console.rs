//! Bounded NetherNet control policy. Socket ownership, stream framing and all
//! gameplay datagram forwarding/fragmentation remain in the Go host.
use base64::{engine::general_purpose::STANDARD, Engine};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use std::net::{IpAddr, Ipv4Addr, SocketAddrV4};

const MAX_DATA: usize = 65536;
pub const MAX_CONTROL: usize = 128 * 1024;
const CANDIDATE_ERROR: &str = "unsafe or unsupported ICE candidate";
type Result<T> = std::result::Result<T, String>;
fn err(s: &str) -> String {
    s.to_owned()
}
pub(crate) fn bytes(v: &Value, field: &str, max: usize) -> Result<Vec<u8>> {
    let s = v
        .get(field)
        .and_then(Value::as_str)
        .ok_or_else(|| err("invalid wire bytes"))?;
    if s.len() > max.div_ceil(3) * 4 {
        return Err(err("invalid length"));
    }
    let b = STANDARD.decode(s).map_err(|_| err("invalid wire bytes"))?;
    if b.len() > max {
        return Err(err("invalid length"));
    }
    Ok(b)
}
pub(crate) fn encoded(b: &[u8]) -> Value {
    json!({"data": STANDARD.encode(b)})
}
fn ipv4(s: &str) -> Option<Ipv4Addr> {
    s.parse().ok()
}
// netip.ParseAddr accepts a nonempty IPv6 zone; Unmap removes it for a
// mapped IPv4 address. Ordinary IPv6 candidates are filtered, including zones.
fn parse_ip(s: &str) -> Option<IpAddr> {
    if let Some((address, zone)) = s.split_once('%') {
        if zone.is_empty() {
            return None;
        }
        return match address.parse().ok()? {
            IpAddr::V6(a) => Some(IpAddr::V6(a)),
            IpAddr::V4(_) => None,
        };
    }
    s.parse().ok()
}
fn unmapped(s: &str) -> Option<Ipv4Addr> {
    match parse_ip(s)? {
        IpAddr::V4(a) => Some(a),
        IpAddr::V6(a) => a.to_ipv4_mapped(),
    }
}
fn port(s: &str) -> Option<u16> {
    if s.is_empty() || !s.bytes().all(|b| b.is_ascii_digit()) {
        None
    } else {
        s.parse().ok()
    }
}
fn normalized(b: &[u8]) -> String {
    // Go strings.ToLower uses simple Unicode case mapping. Rust expands
    // U+0130 to i + combining dot; Go maps it to ASCII i. Preserve Go's
    // fail-closed recognition of candidate-shaped/remote-candidate lines.
    String::from_utf8_lossy(b)
        .trim()
        .replace('İ', "i")
        .to_lowercase()
}
fn spans(b: &[u8]) -> Vec<(usize, usize)> {
    let mut out = Vec::new();
    let mut i = 0;
    while i < b.len() {
        if b[i] == b' ' || b[i] == b'\t' {
            i += 1;
            continue;
        }
        let start = i;
        while i < b.len() && b[i] != b' ' && b[i] != b'\t' {
            i += 1;
        }
        out.push((start, i));
    }
    out
}
fn rewrite_line(
    raw: &[u8],
    allowed: Ipv4Addr,
    first: u16,
    last: u16,
    relay: SocketAddrV4,
) -> Result<(Vec<u8>, String)> {
    let body = raw.strip_prefix(b"a=").unwrap_or(raw);
    if body.iter().any(|&b| b > 126 || (b < 32 && b != b'\t')) {
        return Err(err(CANDIDATE_ERROR));
    }
    let s = spans(body);
    let f: Vec<&[u8]> = s.iter().map(|&(a, b)| &body[a..b]).collect();
    if f.len() < 8
        || !f[0].starts_with(b"candidate:")
        || f[0].len() <= 10
        || f[6] != b"typ"
        || !f[2].eq_ignore_ascii_case(b"udp")
    {
        return Err(err(CANDIDATE_ERROR));
    }
    let ip = unmapped(std::str::from_utf8(f[4]).unwrap()).ok_or_else(|| err(CANDIDATE_ERROR))?;
    let p = port(std::str::from_utf8(f[5]).unwrap()).ok_or_else(|| err(CANDIDATE_ERROR))?;
    if ip != allowed || p < first || p > last {
        return Err(err(CANDIDATE_ERROR));
    }
    let mut out = Vec::new();
    if raw.starts_with(b"a=") {
        out.extend_from_slice(b"a=");
    }
    out.extend_from_slice(&body[..s[4].0]);
    out.extend_from_slice(relay.ip().to_string().as_bytes());
    out.extend_from_slice(&body[s[4].1..s[5].0]);
    out.extend_from_slice(relay.port().to_string().as_bytes());
    out.extend_from_slice(&body[s[5].1..]);
    Ok((out, format!("{ip}:{p}")))
}
fn candidates(v: Value, map: bool) -> Result<Value> {
    let input = bytes(&v, "data", MAX_DATA)?;
    // Desktop offers may contain candidates for several local interfaces.
    // The socket owner supplies the interface addresses; SDP never expands it.
    let allowed: Vec<Ipv4Addr> = if let Some(values) = v["allowed"].as_array() {
        if values.is_empty() || values.len() > 64 {
            return Err(err(CANDIDATE_ERROR));
        }
        values
            .iter()
            .map(|value| {
                value
                    .as_str()
                    .and_then(ipv4)
                    .ok_or_else(|| err(CANDIDATE_ERROR))
            })
            .collect::<Result<_>>()?
    } else {
        vec![v["allowed"]
            .as_str()
            .and_then(ipv4)
            .ok_or_else(|| err(CANDIDATE_ERROR))?]
    };
    let relay = v["relay"]
        .as_str()
        .and_then(|s| s.parse::<SocketAddrV4>().ok())
        .filter(|a| a.port() != 0)
        .ok_or_else(|| err(CANDIDATE_ERROR))?;
    let first = v["first"]
        .as_u64()
        .filter(|&p| p > 0 && p <= 65535)
        .ok_or_else(|| err(CANDIDATE_ERROR))? as u16;
    let last = v["last"]
        .as_u64()
        .filter(|&p| p >= u64::from(first) && p <= 65535)
        .ok_or_else(|| err(CANDIDATE_ERROR))? as u16;
    let mut output = Vec::new();
    let mut targets = Vec::new();
    let mut count = 0;
    for line in input.split_inclusive(|&b| b == b'\n') {
        let raw = line.strip_suffix(b"\n").unwrap_or(line);
        let raw = raw.strip_suffix(b"\r").unwrap_or(raw);
        let lower = normalized(raw);
        if lower.starts_with(if map {
            "a=remote-candidates"
        } else {
            "a=remote-candidates:"
        }) {
            return Err(err(CANDIDATE_ERROR));
        }
        if raw.starts_with(b"a=candidate:") || raw.starts_with(b"candidate:") {
            count += 1;
            if map && (count > 32 || raw.len() > 2048) {
                return Err(err(CANDIDATE_ERROR));
            }
            if map {
                let body = String::from_utf8_lossy(raw);
                let body = body.strip_prefix("a=").unwrap_or(&body);
                let f: Vec<_> = body.split_whitespace().collect();
                if f.len() < 8 || f[0].len() <= 10 || f[6] != "typ" {
                    return Err(err(CANDIDATE_ERROR));
                }
                let ip = parse_ip(f[4]).ok_or_else(|| err(CANDIDATE_ERROR))?;
                let p = port(f[5])
                    .filter(|&p| p > 0)
                    .ok_or_else(|| err(CANDIDATE_ERROR))?;
                let ip = match ip {
                    IpAddr::V4(a) => Some(a),
                    IpAddr::V6(a) => a.to_ipv4_mapped(),
                };
                if ip.is_none() || !f[2].eq_ignore_ascii_case("udp") {
                    continue;
                }
                let ip = ip.unwrap();
                let (mapped, target) = rewrite_line(raw, ip, 1, 65535, relay)?;
                if !allowed.contains(&ip) || p < first || p > last {
                    continue;
                }
                output.extend(mapped);
                targets.push(target);
            } else {
                if allowed.len() != 1 {
                    return Err(err(CANDIDATE_ERROR));
                }
                let (mapped, target) = rewrite_line(raw, allowed[0], first, last, relay)?;
                output.extend(mapped);
                targets.push(target);
            }
            // Preserve exactly the original line terminator, including a final CR.
            output.extend_from_slice(&line[raw.len()..]);
        } else {
            if lower.starts_with("a=candidate") || lower.starts_with("candidate") {
                return Err(err(CANDIDATE_ERROR));
            }
            output.extend_from_slice(line);
        }
    }
    if !map && targets.is_empty() {
        return Err(err(CANDIDATE_ERROR));
    }
    Ok(json!({"data": STANDARD.encode(output), "targets": targets}))
}
fn signal_parse(v: Value) -> Result<Value> {
    let data = bytes(&v, "data", MAX_DATA)?;
    let p: Vec<_> = data.splitn(3, |&b| b == b' ').collect();
    if p.len() != 3 {
        return Err(err("invalid signal"));
    }
    let n = p[1].iter().take_while(|b| b.is_ascii_digit()).count();
    let id = std::str::from_utf8(&p[1][..n])
        .ok()
        .and_then(|s| s.parse::<u64>().ok())
        .filter(|&id| id != 0)
        .ok_or_else(|| err("invalid signal connection"))?;
    let typ = match p[0] {
        b"CONNECTREQUEST" => "offer",
        b"CONNECTRESPONSE" => "answer",
        b"CANDIDATEADD" => "candidate",
        b"CONNECTERROR" => "error",
        _ => return Err(err("unknown signal")),
    };
    if typ == "error" {
        let start = usize::from(p[2].starts_with(b"-"));
        let n = start
            + p[2][start..]
                .iter()
                .take_while(|b| b.is_ascii_digit())
                .count();
        if std::str::from_utf8(&p[2][..n])
            .ok()
            .and_then(|s| s.parse::<i32>().ok())
            .is_none()
        {
            return Err(err("invalid signal error"));
        }
    }
    Ok(json!({"Type":typ,"ConnectionID":id,"Data":STANDARD.encode(p[2])}))
}
fn signal_format(v: Value) -> Result<Value> {
    let data = bytes(&v, "Data", MAX_DATA)?;
    let id = v["ConnectionID"]
        .as_u64()
        .ok_or_else(|| err("invalid signal connection"))?;
    let typ = match v["Type"].as_str().unwrap_or("") {
        "offer" => "CONNECTREQUEST",
        "answer" => "CONNECTRESPONSE",
        "candidate" => "CANDIDATEADD",
        "error" => "CONNECTERROR",
        _ => "",
    };
    let mut out = format!("{typ} {id} ").into_bytes();
    out.extend(data);
    Ok(encoded(&out))
}
fn zero(v: &u64) -> bool {
    *v == 0
}
#[derive(Default, Serialize)]
struct Control {
    version: i64,
    #[serde(rename = "type")]
    typ: String,
    #[serde(skip_serializing_if = "zero")]
    session: u64,
    #[serde(rename = "peerIp", skip_serializing_if = "String::is_empty")]
    peer_ip: String,
    #[serde(skip_serializing_if = "String::is_empty")]
    data: String,
}
impl<'de> Deserialize<'de> for Control {
    fn deserialize<D: serde::Deserializer<'de>>(
        deserializer: D,
    ) -> std::result::Result<Self, D::Error> {
        struct Visitor;
        impl<'de> serde::de::Visitor<'de> for Visitor {
            type Value = Control;
            fn expecting(&self, f: &mut std::fmt::Formatter) -> std::fmt::Result {
                f.write_str("a control message")
            }
            fn visit_map<M: serde::de::MapAccess<'de>>(
                self,
                mut map: M,
            ) -> std::result::Result<Control, M::Error> {
                let mut c = Control::default();
                while let Some(key) = map.next_key::<String>()? {
                    match key.to_ascii_lowercase().as_str() {
                        "version" => {
                            if let Some(v) = map.next_value::<Option<i64>>()? {
                                c.version = v;
                            }
                        }
                        "type" => {
                            if let Some(v) = map.next_value::<Option<String>>()? {
                                c.typ = v;
                            }
                        }
                        "session" => {
                            if let Some(v) = map.next_value::<Option<u64>>()? {
                                c.session = v;
                            }
                        }
                        "peerip" => {
                            if let Some(v) = map.next_value::<Option<String>>()? {
                                c.peer_ip = v;
                            }
                        }
                        "data" => {
                            if let Some(v) = map.next_value::<Option<String>>()? {
                                c.data = v;
                            }
                        }
                        _ => return Err(serde::de::Error::custom("unknown control field")),
                    }
                }
                Ok(c)
            }
        }
        deserializer.deserialize_map(Visitor)
    }
}
impl Control {
    fn validate(&self) -> Result<()> {
        if self.version != 1 || self.data.len() > MAX_DATA {
            return Err(err("invalid console bridge control message"));
        }
        match self.typ.as_str() {
            "hello" | "advertisement" | "status" if self.session == 0 => Ok(()),
            "offer" | "answer" | "candidate" | "error" if self.session != 0 => Ok(()),
            "hello" | "advertisement" | "status" => {
                Err(err("invalid console bridge control session"))
            }
            "offer" | "answer" | "candidate" | "error" => {
                Err(err("missing console bridge control session"))
            }
            _ => Err(err("unsupported console bridge control message")),
        }
    }
}
// encoding/json's wire contract escapes HTML and JavaScript separators. Match
// that encoding after serde has emitted the typed fields in declaration order.
fn go_json(c: &Control) -> Result<Vec<u8>> {
    let s = serde_json::to_string(c).map_err(|_| err("invalid console bridge control message"))?;
    Ok(s.replace('&', "\\u0026")
        .replace('<', "\\u003c")
        .replace('>', "\\u003e")
        .replace('\u{2028}', "\\u2028")
        .replace('\u{2029}', "\\u2029")
        .into_bytes())
}
// Go accepts unpaired JSON surrogate escapes as U+FFFD. Repair only these
// escapes within strings; serde still validates the JSON grammar and types.
fn go_utf8(mut raw: &[u8]) -> String {
    let mut out = String::new();
    while !raw.is_empty() {
        match std::str::from_utf8(raw) {
            Ok(s) => {
                out.push_str(s);
                break;
            }
            Err(e) => {
                let valid = e.valid_up_to();
                out.push_str(std::str::from_utf8(&raw[..valid]).expect("valid prefix"));
                out.push('\u{fffd}');
                // encoding/json consumes one byte per invalid UTF-8 rune,
                // including each byte in an incomplete final sequence.
                raw = &raw[valid + 1..];
            }
        }
    }
    out
}
fn go_json_input(raw: &[u8]) -> String {
    let raw = go_utf8(raw);
    let b = raw.as_bytes();
    let mut out = Vec::new();
    let mut i = 0;
    let mut string = false;
    while i < b.len() {
        if b[i] == b'"' {
            string = !string;
        }
        if string && b[i] == b'\\' && i + 1 < b.len() {
            if b[i + 1] == b'u' && i + 6 <= b.len() {
                if let Ok(h) =
                    u16::from_str_radix(std::str::from_utf8(&b[i + 2..i + 6]).unwrap_or(""), 16)
                {
                    if (0xd800..=0xdbff).contains(&h)
                        && i + 12 <= b.len()
                        && &b[i + 6..i + 8] == b"\\u"
                        && u16::from_str_radix(
                            std::str::from_utf8(&b[i + 8..i + 12]).unwrap_or(""),
                            16,
                        )
                        .is_ok_and(|l| (0xdc00..=0xdfff).contains(&l))
                    {
                        out.extend_from_slice(&b[i..i + 12]);
                        i += 12;
                        continue;
                    }
                    if (0xd800..=0xdfff).contains(&h) {
                        out.extend_from_slice(b"\\ufffd");
                        i += 6;
                        continue;
                    }
                }
            }
            out.extend_from_slice(&b[i..i + 2]);
            i += 2;
            continue;
        }
        out.push(b[i]);
        i += 1;
    }
    String::from_utf8(out).expect("only ASCII escapes changed")
}
fn control(v: Value, encode: bool) -> Result<Value> {
    let c: Control = if encode {
        serde_json::from_value(v).map_err(|_| err("invalid console bridge control message"))?
    } else {
        let raw = bytes(&v, "data", MAX_CONTROL)?;
        if raw.is_empty() {
            return Err(err("invalid console bridge frame length"));
        }
        serde_json::from_str(&go_json_input(&raw))
            .map_err(|_| err("invalid console bridge control message"))?
    };
    c.validate()?;
    if encode {
        let data = go_json(&c)?;
        if data.len() > MAX_CONTROL {
            return Err(err("console bridge frame too large"));
        }
        Ok(encoded(&data))
    } else {
        serde_json::to_value(c).map_err(|_| err("invalid console bridge control message"))
    }
}
pub fn valid_host_ip(s: &str) -> bool {
    ipv4(s).is_some_and(|a| !a.is_unspecified() && !a.is_multicast() && a != Ipv4Addr::BROADCAST)
}
pub fn dispatch(method: &str, v: Value) -> Result<Value> {
    match method {
        "console.candidates_rewrite" => candidates(v, false),
        "console.candidates_map" => candidates(v, true),
        "console.signal_parse" => signal_parse(v),
        "console.signal_format" => signal_format(v),
        "console.control_encode" => control(v, true),
        "console.control_decode" => control(v, false),
        "console.host_ip" => Ok(json!(valid_host_ip(v["ip"].as_str().unwrap_or("")))),
        "console.host_config" => {
            let first = v["first"].as_u64().unwrap_or(0);
            let last = v["last"].as_u64().unwrap_or(0);
            Ok(json!(
                valid_host_ip(v["server"].as_str().unwrap_or(""))
                    && valid_host_ip(v["bind"].as_str().unwrap_or(""))
                    && first > 0
                    && last <= 65535
                    && last >= first
                    && last - first < 256
            ))
        }
        _ => Err(err("unknown console operation")),
    }
}
