//! Minecraft Java LAN advertisements are hints from the local machine.
use serde::Serialize;

const MAX_PACKET: usize = 1024;
const MAX_NAME: usize = 64;

#[derive(Debug, PartialEq, Eq, PartialOrd, Ord, Serialize)]
pub struct World {
    pub name: String,
    pub port: u16,
}

pub fn parse(packet: &[u8]) -> Option<World> {
    if packet.len() > MAX_PACKET {
        return None;
    }
    let text = std::str::from_utf8(packet).ok()?.strip_prefix("[MOTD]")?;
    let (name, port) = text.split_once("[/MOTD][AD]")?;
    let port = port.strip_suffix("[/AD]")?;
    if port.is_empty() || port.len() > 5 || !port.bytes().all(|b| b.is_ascii_digit()) {
        return None;
    }
    let port = port.parse().ok().filter(|&p| p != 0)?;
    if name.contains("[MOTD]") || name.contains("[/MOTD]") {
        return None;
    }
    // Remove Minecraft formatting codes before showing the untrusted MOTD.
    let mut chars = name.chars();
    let mut clean = String::new();
    while let Some(c) = chars.next() {
        if c == '§' {
            chars.next();
        } else if !c.is_control()
            && !matches!(c, '\u{061c}' | '\u{200e}' | '\u{200f}' | '\u{202a}'..='\u{202e}' | '\u{2066}'..='\u{2069}')
        {
            clean.push(c);
        }
    }
    let name: String = clean.trim().chars().take(MAX_NAME).collect();
    Some(World {
        name: if name.is_empty() {
            "Minecraft LAN world".into()
        } else {
            name.trim_end().to_owned()
        },
        port,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_unicode_and_removes_untrusted_formatting() {
        assert_eq!(
            parse("[MOTD] §a森林\u{202e} [/MOTD][AD]65535[/AD]".as_bytes()),
            Some(World {
                name: "森林".into(),
                port: 65535
            }),
        );
        assert_eq!(
            parse(b"[MOTD][/MOTD][AD]1[/AD]").unwrap().name,
            "Minecraft LAN world"
        );
    }

    #[test]
    fn rejects_invalid_packets_and_every_truncation() {
        let good = b"[MOTD]World[/MOTD][AD]25565[/AD]";
        for n in 0..good.len() {
            assert!(parse(&good[..n]).is_none());
        }
        for bad in [
            "",
            "0",
            "65536",
            "+1",
            " 1",
            "١",
            "123456",
            "1[/AD][AD]2",
            "127.0.0.1:1",
        ] {
            assert!(parse(format!("[MOTD]World[/MOTD][AD]{bad}[/AD]").as_bytes()).is_none());
        }
        for bad in [
            &b"[MOTD]\xff[/MOTD][AD]1[/AD]"[..],
            b"junk[MOTD]World[/MOTD][AD]1[/AD]",
            b"[MOTD]World[/MOTD][AD]1[/AD]\0",
            b"[MOTD][MOTD]World[/MOTD][AD]1[/AD]",
        ] {
            assert!(parse(bad).is_none());
        }
        for byte in 0..=255 {
            let mut mutated = good.to_vec();
            for position in 0..good.len() {
                mutated[position] = byte;
                if let Some(world) = parse(&mutated) {
                    assert!((1..=65535).contains(&world.port));
                    assert!(world.name.chars().count() <= MAX_NAME);
                }
                mutated[position] = good[position];
            }
        }
    }

    #[test]
    fn bounds_packet_bytes_and_display_characters() {
        let packet = |n| format!("[MOTD]{}[/MOTD][AD]9[/AD]", "x".repeat(n));
        let at_limit = MAX_PACKET - packet(0).len();
        assert_eq!(
            parse(packet(at_limit).as_bytes()).unwrap().name.len(),
            MAX_NAME
        );
        assert!(parse(packet(at_limit + 1).as_bytes()).is_none());
        assert_eq!(
            parse(format!("[MOTD]{}[/MOTD][AD]9[/AD]", "界".repeat(80)).as_bytes())
                .unwrap()
                .name
                .chars()
                .count(),
            MAX_NAME
        );
    }
}
