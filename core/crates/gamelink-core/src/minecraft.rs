//! Minecraft Java's big-endian, uncompressed NBT server list. Keep raw tag
//! slices so edits leave other entries, root tags and icons intact.
use crate::storage;
use serde::{Deserialize, Serialize};
use std::{
    borrow::Cow,
    collections::{BTreeMap, BTreeSet},
    fs,
    io::{self, Write},
    path::Path,
};

const END: u8 = 0;
const STRING: u8 = 8;
const LIST: u8 = 9;
const COMPOUND: u8 = 10;
const OWNER: &[u8] = b"gamelinkServerId";
const EMPTY: &[u8] = b"\x0a\0\0\0";
const INVALID: &str = "invalid uncompressed Minecraft servers.dat";

type Result<T> = std::result::Result<T, &'static str>;

struct Cursor<'a> {
    remaining: &'a [u8],
}

impl<'a> Cursor<'a> {
    fn take(&mut self, n: usize) -> Result<&'a [u8]> {
        let (taken, rest) = self.remaining.split_at_checked(n).ok_or(INVALID)?;
        self.remaining = rest;
        Ok(taken)
    }

    fn byte(&mut self) -> Result<u8> {
        Ok(self.take(1)?[0])
    }

    fn count(&mut self) -> Result<usize> {
        usize::try_from(i32::from_be_bytes(self.take(4)?.try_into().unwrap())).map_err(|_| INVALID)
    }

    fn string(&mut self) -> Result<&'a [u8]> {
        let size = u16::from_be_bytes(self.take(2)?.try_into().unwrap());
        let bytes = self.take(usize::from(size))?;
        validate_mutf8(bytes)?;
        Ok(bytes)
    }

    fn list_header(&mut self) -> Result<(u8, usize)> {
        let kind = self.byte()?;
        let count = self.count()?;
        if kind > 12 || (kind == END && count != 0) || count > self.remaining.len() {
            return Err(INVALID);
        }
        Ok((kind, count))
    }

    fn payload(&mut self, kind: u8, depth: u8) -> Result<()> {
        if depth >= 64 {
            return Err(INVALID);
        }
        let size = match kind {
            1 => 1,
            2 => 2,
            3 | 5 => 4,
            4 | 6 => 8,
            7 => self.count()?,
            11 => self.count()?.checked_mul(4).ok_or(INVALID)?,
            12 => self.count()?.checked_mul(8).ok_or(INVALID)?,
            STRING => {
                self.string()?;
                return Ok(());
            }
            LIST => {
                let (item, count) = self.list_header()?;
                for _ in 0..count {
                    self.payload(item, depth + 1)?;
                }
                return Ok(());
            }
            COMPOUND => {
                loop {
                    let child = self.byte()?;
                    if child == END {
                        break;
                    }
                    self.string()?;
                    self.payload(child, depth + 1)?;
                }
                return Ok(());
            }
            _ => return Err(INVALID),
        };
        self.take(size)?;
        Ok(())
    }
}

// Java uses modified UTF-8, with UTF-16 surrogate code units and a two-byte NUL.
fn validate_mutf8(mut bytes: &[u8]) -> Result<()> {
    let continuation = |b: u8| b & 0xc0 == 0x80;
    loop {
        let width = match bytes {
            [] => return Ok(()),
            [0..=0x7f, ..] => 1,
            [0xc0, 0x80, ..] => 2,
            [0xc2..=0xdf, b, ..] if continuation(*b) => 2,
            [a @ 0xe0..=0xef, b, c, ..]
                if continuation(*b) && continuation(*c) && (*a != 0xe0 || *b >= 0xa0) =>
            {
                3
            }
            _ => return Err(INVALID),
        };
        bytes = &bytes[width..];
    }
}

fn mutf8(s: &str) -> Vec<u8> {
    let mut bytes = Vec::with_capacity(s.len());
    for unit in s.encode_utf16() {
        match unit {
            1..=0x7f => bytes.push(unit as u8),
            0..=0x7ff => {
                bytes.push(0xc0 | (unit >> 6) as u8);
                bytes.push(0x80 | (unit & 0x3f) as u8);
            }
            _ => {
                bytes.push(0xe0 | (unit >> 12) as u8);
                bytes.push(0x80 | ((unit >> 6) & 0x3f) as u8);
                bytes.push(0x80 | (unit & 0x3f) as u8);
            }
        }
    }
    bytes
}

fn write_string(out: &mut Vec<u8>, s: &[u8]) -> Result<()> {
    let size = u16::try_from(s.len()).map_err(|_| "Minecraft server name is too long")?;
    out.extend(size.to_be_bytes());
    out.extend_from_slice(s);
    Ok(())
}

fn write_tag(out: &mut Vec<u8>, name: &[u8], value: &[u8]) -> Result<()> {
    out.push(STRING);
    write_string(out, name)?;
    write_string(out, value)
}

struct Tag<'a> {
    name: &'a [u8],
    string: Option<&'a [u8]>,
    raw: &'a [u8],
}

fn tags(data: &[u8]) -> impl Iterator<Item = Result<Tag<'_>>> {
    let mut cursor = Cursor { remaining: data };
    std::iter::from_fn(move || {
        let start = cursor.remaining;
        if start == [END] {
            return None;
        }
        let tag = (|| {
            let kind = cursor.byte()?;
            let name = cursor.string()?;
            let string = if kind == STRING {
                Some(cursor.string()?)
            } else {
                cursor.payload(kind, 2)?;
                None
            };
            Ok(Tag {
                name,
                string,
                raw: &start[..start.len() - cursor.remaining.len()],
            })
        })();
        if tag.is_err() {
            cursor.remaining = &[END];
        }
        Some(tag)
    })
}

#[derive(Default)]
struct List<'a> {
    entries: Vec<&'a [u8]>,
    // Byte range of the original list tag, or an insertion point at root END.
    start: usize,
    end: usize,
}

fn parse(data: &[u8]) -> Result<List<'_>> {
    let mut cursor = Cursor { remaining: data };
    if cursor.byte()? != COMPOUND {
        return Err(INVALID);
    }
    cursor.string()?;
    let mut servers = None;
    loop {
        let start = data.len() - cursor.remaining.len();
        let kind = cursor.byte()?;
        if kind == END {
            if !cursor.remaining.is_empty() {
                return Err(INVALID);
            }
            return Ok(servers.unwrap_or(List {
                start,
                end: start,
                ..List::default()
            }));
        }
        if cursor.string()? != b"servers" {
            cursor.payload(kind, 1)?;
            continue;
        }
        if kind != LIST || servers.is_some() {
            return Err(INVALID);
        }
        let (kind, count) = cursor.list_header()?;
        if count > 65_536 || (count > 0 && kind != COMPOUND) {
            return Err(INVALID);
        }
        let mut entries = Vec::new();
        for _ in 0..count {
            let entry = cursor.remaining;
            cursor.payload(COMPOUND, 2)?;
            entries.push(&entry[..entry.len() - cursor.remaining.len()]);
        }
        servers = Some(List {
            start,
            end: data.len() - cursor.remaining.len(),
            entries,
        });
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Deserialize, Serialize)]
pub struct Entry {
    pub name: String,
    pub port: u16,
}

impl Entry {
    fn display(&self) -> Vec<u8> {
        mutf8(&format!("{} (GameLink)", self.name))
    }

    fn address(&self) -> String {
        format!("127.0.0.1:{}", self.port)
    }
}

pub type Entries = BTreeMap<String, Entry>;

fn update_entry(raw: &[u8], id: &str, entry: &Entry) -> Result<Vec<u8>> {
    let mut out = Vec::new();
    for tag in tags(raw) {
        let tag = tag?;
        if !matches!(tag.name, b"name" | b"ip" | OWNER) {
            out.extend_from_slice(tag.raw);
        }
    }
    write_tag(&mut out, b"name", &entry.display())?;
    write_tag(&mut out, b"ip", entry.address().as_bytes())?;
    write_tag(&mut out, OWNER, id.as_bytes())?;
    out.push(END);
    Ok(out)
}

// Minecraft preserves the name and address but drops extension tags when it
// saves the list. Recognize previously written entries by that pair as well.
// A matching entry the player already had is left intact and stays unowned.
fn reconcile(
    data: &[u8],
    previous: &Entries,
    desired: &Entries,
) -> Result<(Option<Vec<u8>>, Entries)> {
    let list = parse(data)?;
    let mut output = Vec::new();
    let mut owned = BTreeMap::new();
    let mut present = BTreeSet::new();
    let keys = |entries: &Entries| {
        entries
            .iter()
            .map(|(id, e)| ((e.display(), e.address().into_bytes()), id.clone()))
            .collect::<BTreeMap<_, _>>()
    };
    let previous_keys = keys(previous);
    let desired_keys = keys(desired);
    let mut entries = Vec::new();
    let mut marked = BTreeSet::new();
    let mut unmarked_counts = BTreeMap::new();
    for raw in list.entries {
        let (mut name, mut addr, mut owner) = (None, None, None);
        for tag in tags(raw) {
            let tag = tag?;
            match tag.name {
                b"name" => name = tag.string,
                b"ip" => addr = tag.string,
                OWNER => owner = tag.string.and_then(|s| std::str::from_utf8(s).ok()),
                _ => (),
            }
        }
        let pair = (
            name.unwrap_or_default().to_vec(),
            addr.unwrap_or_default().to_vec(),
        );
        let owner = owner.filter(|id| valid_id(id));
        if let Some(id) = owner {
            marked.insert(id);
        } else if let Some(id) = previous_keys.get(&pair) {
            *unmarked_counts.entry(id.as_str()).or_insert(0) += 1;
        }
        entries.push((raw, pair, owner));
    }
    let mut changed = false;
    for (raw, pair, owner) in entries {
        let id = owner.or_else(|| {
            previous_keys
                .get(&pair)
                .map(String::as_str)
                .filter(|id| !marked.contains(id) && unmarked_counts.get(id) == Some(&1))
        });
        if let Some(id) = id {
            if let Some(entry) = desired.get(id).filter(|_| !owned.contains_key(id)) {
                if desired_keys.get(&pair).is_some_and(|key| key == id) {
                    output.push(Cow::Borrowed(raw));
                } else {
                    output.push(Cow::Owned(update_entry(raw, id, entry)?));
                    changed = true;
                }
                owned.insert(id.to_owned(), entry.clone());
            } else {
                changed = true;
            }
        } else {
            if let Some(id) = desired_keys.get(&pair) {
                present.insert(id.clone());
            }
            output.push(Cow::Borrowed(raw));
        }
    }
    for (id, entry) in desired {
        if owned.contains_key(id) || present.contains(id) {
            continue;
        }
        output.push(Cow::Owned(update_entry(&[END], id, entry)?));
        owned.insert(id.clone(), entry.clone());
        changed = true;
    }
    if !changed {
        return Ok((None, owned));
    }
    let mut result = data[..list.start].to_vec();
    result.push(LIST);
    write_string(&mut result, b"servers")?;
    result.push(COMPOUND);
    let count = i32::try_from(output.len()).map_err(|_| INVALID)?;
    result.extend(count.to_be_bytes());
    for entry in output {
        result.extend_from_slice(&entry);
    }
    result.extend_from_slice(&data[list.end..]);
    if result.len() > 8 * 1024 * 1024 {
        return Err("Minecraft server list exceeds 8 MiB");
    }
    Ok((Some(result), owned))
}

fn valid_id(id: &str) -> bool {
    // Desktop server IDs are generated from six random bytes.
    id.len() == 12 && id.bytes().all(|b| b.is_ascii_hexdigit())
}

fn read_list(path: &Path) -> io::Result<Option<Vec<u8>>> {
    match fs::symlink_metadata(path) {
        Ok(meta) if !meta.is_file() => Err(io::Error::other("servers.dat is not a regular file")),
        Ok(_) => storage::read_bounded(path).map(Some),
        Err(e) if e.kind() == io::ErrorKind::NotFound => Ok(None),
        Err(e) => Err(e),
    }
}

/// Reconcile one existing Minecraft directory. Returns the entries we own for
/// the next sync. The file is untouched if it fails to parse.
pub fn sync(
    dir: &Path,
    previous: &Entries,
    desired: &Entries,
) -> std::result::Result<Entries, String> {
    let path = dir.join("servers.dat");
    let original = read_list(&path).map_err(|e| e.to_string())?;
    let (updated, owned) = reconcile(original.as_deref().unwrap_or(EMPTY), previous, desired)?;
    let Some(updated) = updated else {
        return Ok(owned);
    };
    if let Some(original) = &original {
        let backup = dir.join("servers.dat.gamelink-backup");
        match fs::symlink_metadata(&backup) {
            Ok(meta) if meta.is_file() => (),
            Ok(_) => return Err("Minecraft backup is not a regular file".into()),
            Err(e) if e.kind() == io::ErrorKind::NotFound => {
                let tmp = stage(dir, original).map_err(|e| e.to_string())?;
                tmp.persist_noclobber(backup)
                    .map_err(|e| e.error.to_string())?;
            }
            Err(e) => return Err(e.to_string()),
        }
    }
    let tmp = stage(dir, &updated).map_err(|e| e.to_string())?;
    // Detect a concurrent Minecraft save before replacing the file.
    if read_list(&path).map_err(|e| e.to_string())? != original {
        return Err(
            "Minecraft changed servers.dat during sync. Close Minecraft and try again.".into(),
        );
    }
    tmp.persist(path).map_err(|e| e.error.to_string())?;
    Ok(owned)
}

fn stage(dir: &Path, bytes: &[u8]) -> io::Result<tempfile::NamedTempFile> {
    let mut tmp = tempfile::Builder::new()
        .prefix(".gamelink-")
        .tempfile_in(dir)?;
    tmp.write_all(bytes)?;
    tmp.as_file().sync_all()?;
    Ok(tmp)
}

#[cfg(test)]
mod tests {
    use super::*;

    const ID: &str = "012345abcdef";
    const OTHER: &str = "abcdef012345";

    fn desired(id: &str, name: &str, port: u16) -> Entries {
        BTreeMap::from([(
            id.into(),
            Entry {
                name: name.into(),
                port,
            },
        )])
    }

    fn tag(out: &mut Vec<u8>, kind: u8, name: &[u8], payload: &[u8]) {
        out.push(kind);
        write_string(out, name).unwrap();
        out.extend_from_slice(payload);
    }

    fn existing() -> (Vec<u8>, Vec<u8>, Vec<u8>) {
        let mut data = vec![COMPOUND, 0, 0];
        let root_prefix = b"\x00\x00\x00\x03\xff\xfe\xfd";
        tag(&mut data, 7, b"unknown bytes", root_prefix);

        let mut list = vec![COMPOUND, 0, 0, 0, 1];
        let mut entry = Vec::new();
        write_tag(&mut entry, b"ip", b"example.invalid").unwrap();
        write_tag(&mut entry, b"name", &mutf8("Player's ☕ 🌎")).unwrap();
        tag(&mut entry, 1, b"acceptTextures", &[0]);
        write_tag(&mut entry, b"icon", b"example").unwrap();
        let mut nested = Vec::new();
        tag(&mut nested, 2, b"short", &[0, 1]);
        tag(&mut nested, 3, b"int", &[0, 0, 0, 2]);
        tag(&mut nested, 4, b"long", &[0; 8]);
        tag(&mut nested, 5, b"float", &[0x7f, 0x80, 0, 1]);
        tag(&mut nested, 6, b"double", &[0; 8]);
        tag(&mut nested, LIST, b"nested list", &[1, 0, 0, 0, 2, 3, 4]);
        tag(&mut nested, 11, b"ints", &[0, 0, 0, 1, 0, 0, 0, 5]);
        tag(
            &mut nested,
            12,
            b"longs",
            &[0, 0, 0, 1, 0, 0, 0, 0, 0, 0, 0, 6],
        );
        nested.push(END);
        tag(&mut entry, COMPOUND, b"extension", &nested);
        entry.push(END);
        list.extend_from_slice(&entry);
        tag(&mut data, LIST, b"servers", &list);

        let mut suffix = Vec::new();
        write_tag(&mut suffix, b"unknown text", &mutf8("NUL\0 and emoji 🌎")).unwrap();
        suffix.push(END);
        data.extend_from_slice(&suffix);
        (data, entry, suffix)
    }

    #[test]
    fn upsert_and_remove_round_trip_preserves_original_nbt_bytes() {
        let (data, entry, suffix) = existing();
        let empty = Entries::new();
        let first = desired(ID, "World 🌎", 25565);

        let (updated, owned) = reconcile(&data, &empty, &first).unwrap();
        let updated = updated.unwrap();
        assert_eq!(owned, first);
        assert!(updated.ends_with(&suffix));
        let list = parse(&updated).unwrap();
        assert_eq!(list.entries[0], entry);
        assert_eq!(updated[..list.start], data[..list.start]);
        let fields: BTreeMap<_, _> = tags(list.entries[1])
            .map(|r| {
                let t = r.unwrap();
                (t.name, t.string.unwrap())
            })
            .collect();
        assert_eq!(
            fields[b"name".as_slice()],
            b"World \xed\xa0\xbc\xed\xbc\x8e (GameLink)"
        );
        assert_eq!(fields[b"ip".as_slice()], b"127.0.0.1:25565");
        assert_eq!(reconcile(&updated, &owned, &first).unwrap(), (None, first));

        let renamed = desired(ID, "Renamed", 20000);
        let (updated, owned) = reconcile(&updated, &owned, &renamed).unwrap();
        let updated = updated.unwrap();
        assert_eq!(owned, renamed);
        let list = parse(&updated).unwrap();
        assert_eq!(list.entries.len(), 2);
        assert_eq!(list.entries[0], entry);
        assert!(tags(list.entries[1]).any(|r| r.unwrap().string == Some(b"127.0.0.1:20000")));
        let (removed, owned) = reconcile(&updated, &owned, &empty).unwrap();
        assert_eq!(removed, Some(data));
        assert_eq!(owned, empty);
    }

    #[test]
    fn minecraft_can_strip_marker_without_losing_ownership() {
        let first = desired(ID, "World", 25565);
        let second = desired(OTHER, "Another world", 20000);
        let (_, owned) = reconcile(EMPTY, &Entries::new(), &first).unwrap();
        let mut rewritten = vec![COMPOUND, 0, 0];
        let mut list = vec![COMPOUND, 0, 0, 0, 1];
        write_tag(&mut list, b"name", &first[ID].display()).unwrap();
        write_tag(&mut list, b"ip", b"127.0.0.1:25565").unwrap();
        // Minecraft can add player preferences to GameLink's entry.
        tag(&mut list, 1, b"acceptTextures", &[1]);
        list.push(END);
        tag(&mut rewritten, LIST, b"servers", &list);
        rewritten.push(END);
        assert_eq!(
            reconcile(&rewritten, &Entries::new(), &first).unwrap(),
            (None, Entries::new())
        );
        assert_eq!(
            reconcile(&rewritten, &owned, &first).unwrap(),
            (None, owned.clone())
        );

        let mut both = first.clone();
        both.extend(second.clone());
        let (updated, owned) = reconcile(&rewritten, &owned, &both).unwrap();
        let updated = updated.unwrap();
        assert_eq!(
            parse(&updated).unwrap().entries[0],
            parse(&rewritten).unwrap().entries[0]
        );
        let (removed, owned) = reconcile(&updated, &owned, &second).unwrap();
        assert_eq!(parse(&removed.unwrap()).unwrap().entries.len(), 1);
        assert_eq!(owned, second);
    }

    #[test]
    fn sync_backs_up_once_and_never_writes_malformed_files() {
        let temp = tempfile::tempdir().unwrap();
        let dir = temp.path();
        let path = dir.join("servers.dat");
        let backup = dir.join("servers.dat.gamelink-backup");
        let empty = Entries::new();
        let (data, _, _) = existing();
        let first = desired(ID, "World", 25565);

        for end in 0..data.len() {
            assert!(
                parse(&data[..end]).is_err(),
                "accepted truncated NBT at {end}"
            );
        }
        for invalid in [
            b"\x1f\x8b".as_slice(),
            b"\x0a\0\0\x08\0\x01X\0\x01\xff\0",
            b"\x0a\0\0\x07\0\x01x\xff\xff\xff\xff\0",
            b"\x0a\0\0\x00trailing",
        ] {
            fs::write(&path, invalid).unwrap();
            assert!(sync(dir, &empty, &first).is_err());
            assert_eq!(fs::read(&path).unwrap(), invalid);
            assert!(!backup.exists());
        }
        fs::write(&path, &data).unwrap();
        let owned = sync(dir, &empty, &first).unwrap();
        assert_eq!(fs::read(&backup).unwrap(), data);
        assert_eq!(sync(dir, &owned, &empty).unwrap(), empty);
        assert_eq!(fs::read(&path).unwrap(), data);
        assert_eq!(fs::read(&backup).unwrap(), data);
    }
}
