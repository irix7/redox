//! The store format, version 1.
//!
//! A store is a directory of static files under one base URL:
//!
//! ```text
//! root                 "webfs1 <hex sha256 of the root directory blob> <unix time>\n"
//! blob/ab/abcd...      one blob, named by the SHA-256 of its content
//! ```
//!
//! A directory blob is text, one entry per line, sorted by name:
//!
//! ```text
//! <kind> <mode, octal> <size> <hex hash> <name>\n      kind: d, f or l
//! ```
//!
//! For a directory the hash names its directory blob. For a symbolic link it
//! names a blob holding the target. For a file of at most `CHUNK` bytes it
//! names the content. A larger file is cut into `CHUNK`-byte pieces, each its
//! own blob, and the hash names a list blob: the 32-byte piece hashes in order.
//!
//! Every blob is immutable, so a cache may keep it for ever. Only `root`
//! changes when the tree is published again.

use sha2::{Digest, Sha256};

pub const MAGIC: &str = "webfs1";
/// Piece size of a large file.
pub const CHUNK: usize = 64 * 1024;
/// Largest blob a client accepts: directory and list blobs are read whole.
pub const MAX_BLOB: usize = 4 * 1024 * 1024;
pub const HASH_LEN: usize = 32;

pub type Hash = [u8; HASH_LEN];

pub fn sha256(data: &[u8]) -> Hash {
    Sha256::digest(data).into()
}

pub fn hex(hash: &Hash) -> String {
    const DIGITS: &[u8; 16] = b"0123456789abcdef";
    let mut out = String::with_capacity(HASH_LEN * 2);
    for byte in hash {
        out.push(DIGITS[usize::from(byte >> 4)] as char);
        out.push(DIGITS[usize::from(byte & 15)] as char);
    }
    out
}

pub fn parse_hex(text: &str) -> Option<Hash> {
    let bytes = text.as_bytes();
    if bytes.len() != HASH_LEN * 2 {
        return None;
    }
    let digit = |b: u8| match b {
        b'0'..=b'9' => Some(b - b'0'),
        b'a'..=b'f' => Some(b - b'a' + 10),
        _ => None,
    };
    let mut hash = [0; HASH_LEN];
    for (i, pair) in bytes.chunks_exact(2).enumerate() {
        hash[i] = digit(pair[0])? << 4 | digit(pair[1])?;
    }
    Some(hash)
}

/// Path of a blob below the base URL.
pub fn blob_path(hash: &Hash) -> String {
    let name = hex(hash);
    format!("blob/{}/{}", &name[..2], name)
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Kind {
    Dir,
    File,
    Link,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Entry {
    pub kind: Kind,
    pub mode: u16,
    pub size: u64,
    pub hash: Hash,
    pub name: String,
}

/// A name a directory blob can hold: one path component, on one line.
pub fn valid_name(name: &str) -> bool {
    !name.is_empty() && name != "." && name != ".." && !name.contains(['/', '\n', '\0'])
}

#[cfg(any(test, not(target_os = "redox")))]
pub fn encode_dir(entries: &[Entry]) -> Vec<u8> {
    let mut sorted: Vec<&Entry> = entries.iter().collect();
    sorted.sort_by(|a, b| a.name.as_bytes().cmp(b.name.as_bytes()));
    let mut out = String::new();
    for entry in sorted {
        let kind = match entry.kind {
            Kind::Dir => 'd',
            Kind::File => 'f',
            Kind::Link => 'l',
        };
        out.push_str(&format!(
            "{kind} {:o} {} {} {}\n",
            entry.mode,
            entry.size,
            hex(&entry.hash),
            entry.name
        ));
    }
    out.into_bytes()
}

fn parse_line(line: &str) -> Option<Entry> {
    let mut fields = line.splitn(5, ' ');
    let kind = match fields.next()? {
        "d" => Kind::Dir,
        "f" => Kind::File,
        "l" => Kind::Link,
        _ => return None,
    };
    let mode = u16::from_str_radix(fields.next()?, 8).ok()?;
    let size = fields.next()?.parse().ok()?;
    let hash = parse_hex(fields.next()?)?;
    let name = fields.next()?;
    if !valid_name(name) {
        return None;
    }
    Some(Entry {
        kind,
        mode,
        size,
        hash,
        name: name.to_string(),
    })
}

/// Entries of a directory blob, in stored order. `None` for a malformed blob.
pub fn parse_dir(blob: &[u8]) -> Option<Vec<Entry>> {
    let text = std::str::from_utf8(blob).ok()?;
    text.lines().map(parse_line).collect()
}

/// One entry of a directory blob, without building the others.
pub fn find_in_dir(blob: &[u8], name: &str) -> Option<Entry> {
    let text = std::str::from_utf8(blob).ok()?;
    text.lines()
        .find(|line| line.splitn(5, ' ').nth(4) == Some(name))
        .and_then(parse_line)
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Root {
    pub hash: Hash,
    /// When the tree was published, seconds since 1970. Every file reports it
    /// as its modification time.
    pub time: u64,
}

#[cfg(any(test, not(target_os = "redox")))]
pub fn encode_root(root: &Root) -> Vec<u8> {
    format!("{MAGIC} {} {}\n", hex(&root.hash), root.time).into_bytes()
}

pub fn parse_root(data: &[u8]) -> Option<Root> {
    let text = std::str::from_utf8(data).ok()?;
    let mut fields = text.split_whitespace();
    if fields.next()? != MAGIC {
        return None;
    }
    let hash = parse_hex(fields.next()?)?;
    let time = fields.next()?.parse().ok()?;
    Some(Root { hash, time })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn hex_round_trip() {
        let hash = sha256(b"abc");
        assert_eq!(
            hex(&hash),
            "ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad"
        );
        assert_eq!(parse_hex(&hex(&hash)), Some(hash));
        assert_eq!(parse_hex("zz"), None);
    }

    #[test]
    fn dir_round_trip_sorts_and_keeps_spaces() {
        let entry = |name: &str, kind| Entry {
            kind,
            mode: 0o644,
            size: 3,
            hash: sha256(name.as_bytes()),
            name: name.to_string(),
        };
        let blob = encode_dir(&[entry("b c", Kind::File), entry("a", Kind::Dir)]);
        let parsed = parse_dir(&blob).unwrap();
        assert_eq!(parsed[0].name, "a");
        assert_eq!(parsed[1].name, "b c");
        assert_eq!(find_in_dir(&blob, "b c"), Some(parsed[1].clone()));
        assert_eq!(find_in_dir(&blob, "b"), None);
    }

    #[test]
    fn malformed_dir_is_rejected() {
        assert!(parse_dir(b"f 644 3 abcd name\n").is_none());
        assert!(parse_dir(b"x 644 3\n").is_none());
    }

    #[test]
    fn root_round_trip() {
        let root = Root {
            hash: sha256(b""),
            time: 1_800_000_000,
        };
        assert_eq!(parse_root(&encode_root(&root)), Some(root));
        assert_eq!(parse_root(b"webfs2 00 1\n"), None);
    }
}
