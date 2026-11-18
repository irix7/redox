//! Turn a directory into a store: `blob/` and `root`.
//!
//! Packing is incremental. A blob that already exists in the output is left
//! alone, so publishing a changed tree writes only the changed pieces, their
//! directories and `root`. Blobs no tree refers to any more stay in place.

use std::fs::{self, File};
use std::io::{self, Read, Write};
use std::os::unix::fs::PermissionsExt;
use std::path::Path;

use crate::format::{self, CHUNK, Entry, Hash, Kind, MAX_BLOB, Root};

#[derive(Default, Debug)]
pub struct Stats {
    pub files: u64,
    pub dirs: u64,
    pub links: u64,
    pub skipped: u64,
    pub content_bytes: u64,
    pub blobs_written: u64,
    pub blobs_present: u64,
    pub bytes_written: u64,
}

struct Packer<'a> {
    out: &'a Path,
    stats: Stats,
}

fn other(message: String) -> io::Error {
    io::Error::other(message)
}

fn write_atomic(path: &Path, data: &[u8]) -> io::Result<()> {
    let tmp = path.with_extension("tmp");
    let mut file = File::create(&tmp)?;
    file.write_all(data)?;
    drop(file);
    fs::rename(&tmp, path)
}

/// Fill `buf` from `file`; short only at the end of the file.
fn read_full(file: &mut File, buf: &mut [u8]) -> io::Result<usize> {
    let mut filled = 0;
    while filled < buf.len() {
        match file.read(&mut buf[filled..])? {
            0 => break,
            n => filled += n,
        }
    }
    Ok(filled)
}

impl Packer<'_> {
    fn put(&mut self, data: &[u8]) -> io::Result<Hash> {
        let hash = format::hash(data);
        let path = self.out.join(format::blob_path(&hash));
        if path.exists() {
            self.stats.blobs_present += 1;
        } else {
            fs::create_dir_all(path.parent().expect("a blob path has a parent"))?;
            write_atomic(&path, data)?;
            self.stats.blobs_written += 1;
            self.stats.bytes_written += data.len() as u64;
        }
        Ok(hash)
    }

    fn file(&mut self, path: &Path, size: u64) -> io::Result<Hash> {
        let mut file = File::open(path)?;
        let mut piece = vec![0; CHUNK];
        if size <= CHUNK as u64 {
            let n = read_full(&mut file, &mut piece)?;
            if n as u64 != size {
                return Err(other(format!("{}: changed while packing", path.display())));
            }
            return self.put(&piece[..n]);
        }
        let mut list = Vec::new();
        let mut total = 0;
        loop {
            let n = read_full(&mut file, &mut piece)?;
            if n == 0 {
                break;
            }
            total += n as u64;
            list.extend_from_slice(&self.put(&piece[..n])?);
        }
        if total != size {
            return Err(other(format!("{}: changed while packing", path.display())));
        }
        if list.len() > MAX_BLOB {
            return Err(other(format!("{}: too large for the format", path.display())));
        }
        self.put(&list)
    }

    fn dir(&mut self, dir: &Path) -> io::Result<Hash> {
        let mut entries = Vec::new();
        for item in fs::read_dir(dir)? {
            let item = item?;
            let path = item.path();
            let Some(name) = item.file_name().to_str().map(str::to_string) else {
                return Err(other(format!("{}: name is not UTF-8", path.display())));
            };
            if !format::valid_name(&name) {
                return Err(other(format!("{}: name cannot be stored", path.display())));
            }
            let meta = fs::symlink_metadata(&path)?;
            let kind = meta.file_type();
            let entry = if kind.is_dir() {
                self.stats.dirs += 1;
                Entry {
                    kind: Kind::Dir,
                    mode: 0o755,
                    size: 0,
                    hash: self.dir(&path)?,
                    name,
                }
            } else if kind.is_file() {
                self.stats.files += 1;
                self.stats.content_bytes += meta.len();
                let executable = meta.permissions().mode() & 0o111 != 0;
                Entry {
                    kind: Kind::File,
                    mode: if executable { 0o755 } else { 0o644 },
                    size: meta.len(),
                    hash: self.file(&path, meta.len())?,
                    name,
                }
            } else if kind.is_symlink() {
                self.stats.links += 1;
                let target = fs::read_link(&path)?;
                let Some(target) = target.to_str() else {
                    return Err(other(format!("{}: target is not UTF-8", path.display())));
                };
                Entry {
                    kind: Kind::Link,
                    mode: 0o777,
                    size: target.len() as u64,
                    hash: self.put(target.as_bytes())?,
                    name,
                }
            } else {
                eprintln!("webfs: skipping {} (not a file, directory or link)", path.display());
                self.stats.skipped += 1;
                continue;
            };
            entries.push(entry);
        }
        let blob = format::encode_dir(&entries);
        if blob.len() > MAX_BLOB {
            return Err(other(format!("{}: too many entries for the format", dir.display())));
        }
        self.put(&blob)
    }
}

/// Pack `src` into the store at `out`. `time` becomes the modification time
/// every file reports.
pub fn pack(src: &Path, out: &Path, time: u64) -> io::Result<(Root, Stats)> {
    fs::create_dir_all(out)?;
    let mut packer = Packer {
        out,
        stats: Stats::default(),
    };
    let hash = packer.dir(src)?;
    let root = Root { hash, time };
    // Last: a reader that sees the new root finds every blob it names.
    write_atomic(&out.join("root"), &format::encode_root(&root))?;
    Ok((root, packer.stats))
}
