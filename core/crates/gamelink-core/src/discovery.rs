//! NetherNet discovery wire codec, adapted from df-mc/go-nethernet v1.0.26.
//! Copyright (c) 2019 Dragonfly Tech. MIT license: discovery-LICENSE; see
//! discovery-NOTICE. The shared LAN key provides wire compatibility, not peer
//! authentication: Go must still pin the UDP source/interface before decoding.
use crate::console::{bytes, encoded};
use aes::cipher::{BlockDecrypt, BlockEncrypt, KeyInit};
use aes::Aes256;
use base64::{engine::general_purpose::STANDARD, Engine};
use hmac::{Hmac, Mac};
use serde_json::{json, Value};
use sha2::{Digest, Sha256};

type Result<T> = std::result::Result<T, String>;
const MAX: usize = 65535;
fn error(s: &str) -> String {
    s.to_owned()
}
fn key() -> [u8; 32] {
    Sha256::digest(0xdeadbeef_u64.to_le_bytes()).into()
}
#[allow(deprecated)] // cipher 0.4's generic-array API; no replacement in aes 0.8.
fn encrypt(src: &[u8]) -> Vec<u8> {
    let padding = 16 - src.len() % 16;
    let mut out = src.to_vec();
    out.resize(src.len() + padding, padding as u8);
    let cipher = Aes256::new_from_slice(&key()).expect("AES-256 key");
    for block in out.as_chunks_mut::<16>().0 {
        cipher.encrypt_block(block.into());
    }
    out
}
#[allow(deprecated)]
fn decrypt(src: &[u8]) -> Result<Vec<u8>> {
    if src.is_empty() || !src.len().is_multiple_of(16) {
        return Err(error("invalid ciphertext length"));
    }
    let cipher = Aes256::new_from_slice(&key()).expect("AES-256 key");
    let mut out = src.to_vec();
    for block in out.as_chunks_mut::<16>().0 {
        cipher.decrypt_block(block.into());
    }
    let n = *out.last().unwrap() as usize;
    if n == 0 || n > 16 || out[out.len() - n..].iter().any(|&b| b as usize != n) {
        return Err(error("invalid padding"));
    }
    out.truncate(out.len() - n);
    Ok(out)
}
fn seal(payload: &[u8]) -> Vec<u8> {
    let mut mac = <Hmac<Sha256> as Mac>::new_from_slice(&key()).expect("HMAC key");
    mac.update(payload);
    let mut out = mac.finalize().into_bytes().to_vec();
    out.extend(encrypt(payload));
    out
}
fn open(src: &[u8]) -> Result<Vec<u8>> {
    if src.len() < 32 {
        return Err(error("short discovery packet"));
    }
    let payload = decrypt(&src[32..])?;
    let mut mac = <Hmac<Sha256> as Mac>::new_from_slice(&key()).expect("HMAC key");
    mac.update(&payload);
    mac.verify_slice(&src[..32])
        .map_err(|_| error("checksum mismatch"))?;
    Ok(payload)
}
struct Reader<'a> {
    b: &'a [u8],
    pos: usize,
}
impl<'a> Reader<'a> {
    fn new(b: &'a [u8]) -> Self {
        Self { b, pos: 0 }
    }
    fn take(&mut self, n: usize) -> Result<&'a [u8]> {
        if n > self.b.len() - self.pos {
            return Err(error("invalid length"));
        }
        let b = &self.b[self.pos..self.pos + n];
        self.pos += n;
        Ok(b)
    }
    fn u16(&mut self) -> Result<u16> {
        Ok(u16::from_le_bytes(self.take(2)?.try_into().unwrap()))
    }
    fn u32(&mut self) -> Result<u32> {
        Ok(u32::from_le_bytes(self.take(4)?.try_into().unwrap()))
    }
    fn u64(&mut self) -> Result<u64> {
        Ok(u64::from_le_bytes(self.take(8)?.try_into().unwrap()))
    }
    fn byte(&mut self) -> Result<u8> {
        Ok(self.take(1)?[0])
    }
    fn rest(&mut self) -> &'a [u8] {
        let b = &self.b[self.pos..];
        self.pos = self.b.len();
        b
    }
    fn finish(&self) -> Result<()> {
        if self.pos != self.b.len() {
            Err(error("unread discovery bytes"))
        } else {
            Ok(())
        }
    }
    fn length_bytes(&mut self) -> Result<&'a [u8]> {
        let n = self.u32()? as usize;
        if n > MAX {
            return Err(error("invalid length"));
        }
        self.take(n)
    }
    fn varuint(&mut self) -> Result<u32> {
        let mut v = 0u32;
        for i in (0..35).step_by(7) {
            let b = self.byte()?;
            v |= u32::from(b & 0x7f) << i;
            if b & 0x80 == 0 {
                return Ok(v);
            }
        }
        Err(error("varuint32 did not terminate after 5 bytes"))
    }
    fn varint(&mut self) -> Result<i32> {
        let u = self.varuint()?;
        let v = (u >> 1) as i32;
        Ok(if u & 1 != 0 { !v } else { v })
    }
    fn string(&mut self) -> Result<String> {
        let n = self.varuint()? as usize;
        Ok(STANDARD.encode(self.take(n)?))
    }
}
fn num(v: &Value, name: &str) -> Result<u64> {
    v[name]
        .as_u64()
        .ok_or_else(|| error("invalid discovery value"))
}
fn id(v: &Value) -> Result<u16> {
    u16::try_from(num(v, "id")?).map_err(|_| error("unknown packet ID"))
}
fn header_write(v: &Value) -> Result<Vec<u8>> {
    let mut out = id(v)?.to_le_bytes().to_vec();
    out.extend(num(v, "sender")?.to_le_bytes());
    out.extend([0; 8]);
    Ok(out)
}
fn header_read(r: &mut Reader<'_>) -> Result<Value> {
    let id = r.u16()?;
    let sender = r.u64()?;
    r.take(8)?;
    Ok(json!({"id":id,"sender":sender}))
}
fn body_write(v: &Value) -> Result<Vec<u8>> {
    let mut out = Vec::new();
    match id(v)? {
        0 => {}
        1 => {
            let data = bytes(v, "data", MAX / 2)?;
            let hex: Vec<u8> = data
                .iter()
                .flat_map(|&b| {
                    [
                        b"0123456789abcdef"[(b >> 4) as usize],
                        b"0123456789abcdef"[(b & 15) as usize],
                    ]
                })
                .collect();
            out.extend((hex.len() as u32).to_le_bytes());
            out.extend(hex);
        }
        2 => {
            out.extend(num(v, "recipient")?.to_le_bytes());
            let data = bytes(v, "data", MAX)?;
            out.extend((data.len() as u32).to_le_bytes());
            out.extend(data);
        }
        _ => return Err(error("unknown packet ID")),
    }
    if out.len() > MAX {
        return Err(error("invalid length"));
    }
    Ok(out)
}
fn body_read(packet_id: u16, r: &mut Reader<'_>) -> Result<Value> {
    let mut out = json!({"id":packet_id});
    match packet_id {
        0 => {}
        1 => {
            let hex = r.length_bytes()?;
            if !hex.len().is_multiple_of(2) {
                return Err(error("invalid application hex"));
            }
            let mut data = Vec::new();
            for p in hex.as_chunks::<2>().0 {
                let a = (p[0] as char)
                    .to_digit(16)
                    .ok_or_else(|| error("invalid application hex"))?;
                let b = (p[1] as char)
                    .to_digit(16)
                    .ok_or_else(|| error("invalid application hex"))?;
                data.push(((a << 4) | b) as u8)
            }
            out["data"] = json!(STANDARD.encode(data));
        }
        2 => {
            out["recipient"] = json!(r.u64()?);
            let mut data = r.length_bytes()?.to_vec();
            data.extend(r.rest());
            out["data"] = json!(STANDARD.encode(data));
        }
        _ => return Err(error("unknown packet ID")),
    }
    r.finish()?;
    Ok(out)
}
fn packet_encode(v: Value) -> Result<Value> {
    let mut payload = vec![0, 0];
    payload.extend(header_write(&v)?);
    payload.extend(body_write(&v)?);
    if payload.len() > MAX {
        return Err(error("invalid length"));
    }
    let n = (payload.len() as u16).to_le_bytes();
    payload[..2].copy_from_slice(&n);
    Ok(encoded(&seal(&payload)))
}
fn packet_decode(v: Value) -> Result<Value> {
    let wire = bytes(&v, "data", MAX + 49)?;
    let payload = open(&wire)?;
    if payload.len() > MAX {
        return Err(error("invalid length"));
    }
    let mut r = Reader::new(&payload);
    r.take(2)?; // Both vanilla-inclusive and legacy-exclusive prefixes are advisory.
    let h = header_read(&mut r)?;
    let mut out = body_read(id(&h)?, &mut r)?;
    out["sender"] = h["sender"].clone();
    Ok(out)
}
fn write_varuint(out: &mut Vec<u8>, mut v: u32) {
    while v >= 0x80 {
        out.push(v as u8 | 0x80);
        v >>= 7;
    }
    out.push(v as u8);
}
fn write_varint(out: &mut Vec<u8>, v: i32) {
    write_varuint(out, ((v as u32) << 1) ^ ((v >> 31) as u32));
}
fn write_string(out: &mut Vec<u8>, v: &Value, name: &str) -> Result<()> {
    let b = bytes(v, name, MAX)?;
    write_varuint(out, b.len() as u32);
    out.extend(b);
    Ok(())
}
fn write_int(out: &mut Vec<u8>, v: &Value, name: &str) -> Result<()> {
    let n = v[name]
        .as_i64()
        .and_then(|n| i32::try_from(n).ok())
        .ok_or_else(|| error("invalid advertisement integer"))?;
    write_varint(out, n);
    Ok(())
}
fn ad_encode(v: Value) -> Result<Value> {
    let mut out = vec![7];
    write_string(&mut out, &v, "ServerName")?;
    write_int(&mut out, &v, "Protocol")?;
    write_string(&mut out, &v, "Version")?;
    write_string(&mut out, &v, "LevelName")?;
    for name in ["PlayerCount", "MaxPlayerCount", "GameType"] {
        write_int(&mut out, &v, name)?;
    }
    for name in [
        "EditorWorld",
        "Hardcore",
        "AcceptsOnlineAuth",
        "AcceptsSelfSignedAuth",
    ] {
        out.push(u8::from(
            v[name]
                .as_bool()
                .ok_or_else(|| error("invalid advertisement bool"))?,
        ));
    }
    write_string(&mut out, &v, "Nonce")?;
    write_int(&mut out, &v, "ConnectionType")?;
    if out.len() > MAX {
        return Err(error("invalid length"));
    }
    Ok(encoded(&out))
}
fn ad_decode(v: Value) -> Result<Value> {
    let b = bytes(&v, "data", MAX)?;
    let mut r = Reader::new(&b);
    if r.byte()? != 7 {
        return Err(error("advertisement version mismatch"));
    }
    let mut out = json!({});
    out["ServerName"] = json!(r.string()?);
    out["Protocol"] = json!(r.varint()?);
    out["Version"] = json!(r.string()?);
    out["LevelName"] = json!(r.string()?);
    for name in ["PlayerCount", "MaxPlayerCount", "GameType"] {
        out[name] = json!(r.varint()?);
    }
    for name in [
        "EditorWorld",
        "Hardcore",
        "AcceptsOnlineAuth",
        "AcceptsSelfSignedAuth",
    ] {
        out[name] = json!(r.byte()? != 0);
    }
    out["Nonce"] = json!(r.string()?);
    out["ConnectionType"] = json!(r.varint()?);
    r.finish()?;
    Ok(out)
}
pub fn dispatch(method: &str, v: Value) -> Result<Value> {
    match method {
        "discovery.packet_encode" => packet_encode(v),
        "discovery.packet_decode" => packet_decode(v),
        "discovery.body_encode" => Ok(encoded(&body_write(&v)?)),
        "discovery.body_decode" => {
            let b = bytes(&v, "data", MAX)?;
            body_read(id(&v)?, &mut Reader::new(&b))
        }
        "discovery.header_encode" => Ok(encoded(&header_write(&v)?)),
        "discovery.header_decode" => {
            let b = bytes(&v, "data", 18)?;
            let mut r = Reader::new(&b);
            let h = header_read(&mut r)?;
            r.finish()?;
            Ok(h)
        }
        "discovery.ad_encode" => ad_encode(v),
        "discovery.ad_decode" => ad_decode(v),
        _ => Err(error("unknown discovery operation")),
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn shared_key_is_not_authentication_and_bad_ciphertext_is_rejected() {
        let wire = seal(b"control");
        assert_eq!(open(&wire).unwrap(), b"control");
        for i in [0, 31, 32, wire.len() - 1] {
            let mut b = wire.clone();
            b[i] ^= 1;
            assert!(open(&b).is_err());
        }
        assert!(decrypt(&[]).is_err());
        assert!(decrypt(&[0; 15]).is_err());
    }
}
