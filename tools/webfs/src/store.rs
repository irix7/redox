//! The client side of a store: fetch blobs, check them against their names,
//! keep a bounded cache, resolve paths and read file ranges.

use std::collections::{HashMap, VecDeque};
use std::io;
use std::sync::{Arc, Mutex, MutexGuard};
use std::thread;
use std::time::{Duration, Instant};

use crate::format::{self, CHUNK, Entry, HASH_LEN, Hash, Kind, MAX_BLOB, Root};
use crate::http::{Client, Url};
use crate::trust::TrustStore;
use std::net::IpAddr;

/// Symbolic links followed while resolving one path.
const MAX_LINKS: u32 = 8;
/// How long after start-up a failed mount is retried before the caller gets
/// an error: the first open usually arrives before DHCP has finished.
const MOUNT_PATIENCE: Duration = Duration::from_secs(60);

#[derive(Debug)]
pub enum Error {
    NotFound,
    NotDir,
    IsDir,
    Loop,
    /// The store or the network failed; the text says how.
    Io(String),
}

impl From<io::Error> for Error {
    fn from(err: io::Error) -> Self {
        Error::Io(err.to_string())
    }
}

impl std::fmt::Display for Error {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Error::NotFound => f.write_str("no such file or directory"),
            Error::NotDir => f.write_str("not a directory"),
            Error::IsDir => f.write_str("is a directory"),
            Error::Loop => f.write_str("too many symbolic links"),
            Error::Io(text) => f.write_str(text),
        }
    }
}

pub type Result<T> = std::result::Result<T, Error>;

/// Verified blobs, least recently used out first, capped by total bytes.
struct Cache {
    blobs: HashMap<Hash, (Arc<[u8]>, u64)>,
    bytes: usize,
    cap: usize,
    tick: u64,
}

impl Cache {
    fn get(&mut self, hash: &Hash) -> Option<Arc<[u8]>> {
        self.tick += 1;
        let (blob, used) = self.blobs.get_mut(hash)?;
        *used = self.tick;
        Some(blob.clone())
    }

    fn insert(&mut self, hash: Hash, blob: Arc<[u8]>) {
        self.tick += 1;
        self.bytes += blob.len();
        if let Some((old, _)) = self.blobs.insert(hash, (blob, self.tick)) {
            self.bytes -= old.len();
        }
        // The newest blob stays even when it alone is over the cap.
        while self.bytes > self.cap && self.blobs.len() > 1 {
            let oldest = *self
                .blobs
                .iter()
                .min_by_key(|(_, (_, used))| *used)
                .map(|(hash, _)| hash)
                .expect("the cache is not empty");
            if let Some((blob, _)) = self.blobs.remove(&oldest) {
                self.bytes -= blob.len();
            }
        }
    }
}

/// A poisoned lock means a worker panicked; the data it guards is a cache
/// and a list of connections, both still usable.
fn lock<T>(mutex: &Mutex<T>) -> MutexGuard<'_, T> {
    mutex.lock().unwrap_or_else(|poisoned| poisoned.into_inner())
}

/// Shared by every worker thread of the daemon.
pub struct Store {
    pub url: Url,
    trust: Arc<TrustStore>,
    address: Option<IpAddr>,
    /// Connections no thread is using, most recently used last. A thread
    /// takes the last one, so a single reader keeps using one connection
    /// and pays for one TLS handshake.
    idle: Mutex<Vec<Client>>,
    cache: Mutex<Cache>,
    root: Mutex<Option<Root>>,
    /// When set, the only root hash accepted from the server.
    pinned: Option<Hash>,
    started: Instant,
}

impl Store {
    /// `cache_bytes` caps the blob cache. `address`, when set, is connected
    /// to in place of the address of the host in `url`.
    pub fn new(
        url: Url,
        trust: TrustStore,
        address: Option<IpAddr>,
        cache_bytes: usize,
        pinned: Option<Hash>,
    ) -> Self {
        Store {
            url,
            trust: Arc::new(trust),
            address,
            idle: Mutex::new(Vec::new()),
            cache: Mutex::new(Cache {
                blobs: HashMap::new(),
                bytes: 0,
                cap: cache_bytes,
                tick: 0,
            }),
            root: Mutex::new(None),
            pinned,
            started: Instant::now(),
        }
    }

    /// GET `rel` below the base URL on an idle connection, or a new one
    /// when every connection is in use.
    fn get(&self, rel: &str, limit: usize) -> io::Result<Vec<u8>> {
        let idle = lock(&self.idle).pop();
        let mut client =
            idle.unwrap_or_else(|| Client::new(self.url.clone(), self.trust.clone(), self.address));
        let result = client.get(rel, limit);
        lock(&self.idle).push(client);
        result
    }

    fn fetch_root(&self) -> Result<Root> {
        let data = self.get("root", 256)?;
        let root = format::parse_root(&data)
            .ok_or_else(|| Error::Io("the root file is not a webfs1 root".into()))?;
        if self.pinned.is_some_and(|pinned| pinned != root.hash) {
            return Err(Error::Io("the root hash differs from the pinned one".into()));
        }
        Ok(root)
    }

    /// The tree this client reads. Fetched on first use and then kept, so
    /// one run sees one snapshot. Other threads wait while one fetches it.
    pub fn root(&self) -> Result<Root> {
        let mut root = lock(&self.root);
        loop {
            if let Some(root) = *root {
                return Ok(root);
            }
            match self.fetch_root() {
                Ok(fetched) => {
                    #[cfg(target_os = "redox")]
                    eprintln!("webfs: mounted {} at root {}", self.url.host, format::hex(&fetched.hash));
                    *root = Some(fetched);
                }
                Err(err) if self.started.elapsed() < MOUNT_PATIENCE => {
                    eprintln!("webfs: mount failed, trying again: {err}");
                    thread::sleep(Duration::from_secs(2));
                }
                Err(err) => return Err(err),
            }
        }
    }

    /// The blob named `hash`, from the cache or the server. The content is
    /// checked against the name before it is used.
    pub fn blob(&self, hash: &Hash) -> Result<Arc<[u8]>> {
        if let Some(blob) = lock(&self.cache).get(hash) {
            return Ok(blob);
        }
        // Two threads that miss on the same blob both fetch it; the second
        // insert replaces the first.
        let data = self.get(&format::blob_path(hash), MAX_BLOB)?;
        if format::sha256(&data) != *hash {
            return Err(Error::Io(format!("blob {} does not match its name", format::hex(hash))));
        }
        let blob: Arc<[u8]> = data.into();
        lock(&self.cache).insert(*hash, blob.clone());
        Ok(blob)
    }

    /// Entries of a directory.
    pub fn list(&self, dir: &Entry) -> Result<Vec<Entry>> {
        if dir.kind != Kind::Dir {
            return Err(Error::NotDir);
        }
        let blob = self.blob(&dir.hash)?;
        format::parse_dir(&blob).ok_or_else(|| Error::Io("malformed directory blob".into()))
    }

    /// Target of a symbolic link.
    pub fn target(&self, link: &Entry) -> Result<String> {
        let blob = self.blob(&link.hash)?;
        String::from_utf8(blob.to_vec()).map_err(|_| Error::Io("malformed link blob".into()))
    }

    /// The entry at `path`, relative to the root of the tree. Links are
    /// followed inside the tree; `follow_last` says whether a link in the
    /// last component is followed too.
    pub fn resolve(&self, path: &str, follow_last: bool) -> Result<Entry> {
        let root = self.root()?;
        let mut stack = vec![Entry {
            kind: Kind::Dir,
            mode: 0o755,
            size: 0,
            hash: root.hash,
            name: String::new(),
        }];
        let mut todo: VecDeque<String> = path.split('/').map(str::to_string).collect();
        let mut links = 0;
        while let Some(part) = todo.pop_front() {
            match part.as_str() {
                "" | "." => continue,
                ".." => {
                    if stack.len() > 1 {
                        stack.pop();
                    }
                    continue;
                }
                _ => {}
            }
            let dir = stack.last().expect("the root stays on the stack");
            if dir.kind != Kind::Dir {
                return Err(Error::NotDir);
            }
            let blob = self.blob(&dir.hash)?;
            let entry = format::find_in_dir(&blob, &part).ok_or(Error::NotFound)?;
            if entry.kind == Kind::Link && (follow_last || todo.iter().any(|p| !p.is_empty())) {
                links += 1;
                if links > MAX_LINKS {
                    return Err(Error::Loop);
                }
                let target = self.target(&entry)?;
                if target.starts_with('/') {
                    stack.truncate(1);
                }
                for part in target.rsplit('/') {
                    todo.push_front(part.to_string());
                }
            } else {
                stack.push(entry);
            }
        }
        Ok(stack.pop().expect("the root stays on the stack"))
    }

    /// Read from a file at `offset`. Fills `buf` unless the file ends first.
    pub fn read(&self, file: &Entry, offset: u64, buf: &mut [u8]) -> Result<usize> {
        match file.kind {
            Kind::Dir => return Err(Error::IsDir),
            Kind::Link => {
                let target = self.blob(&file.hash)?;
                return Ok(copy_from(&target, offset, buf));
            }
            Kind::File => {}
        }
        if offset >= file.size || buf.is_empty() {
            return Ok(0);
        }
        if file.size <= CHUNK as u64 {
            let content = self.blob(&file.hash)?;
            if content.len() as u64 != file.size {
                return Err(Error::Io("file blob has the wrong size".into()));
            }
            return Ok(copy_from(&content, offset, buf));
        }

        let want = buf.len().min((file.size - offset) as usize);
        let mut done = 0;
        while done < want {
            let position = offset + done as u64;
            let index = (position / CHUNK as u64) as usize;
            // Looked up for every piece: the cache may have dropped the list
            // while the previous piece was fetched.
            let list = self.blob(&file.hash)?;
            let hash: Hash = list
                .get(index * HASH_LEN..(index + 1) * HASH_LEN)
                .and_then(|bytes| bytes.try_into().ok())
                .ok_or_else(|| Error::Io("piece list is shorter than the file".into()))?;
            let piece = self.blob(&hash)?;
            let copied = copy_from(&piece, position % CHUNK as u64, &mut buf[done..want]);
            if copied == 0 {
                return Err(Error::Io("piece is shorter than the file".into()));
            }
            done += copied;
        }
        Ok(done)
    }
}

fn copy_from(data: &[u8], offset: u64, buf: &mut [u8]) -> usize {
    let Some(rest) = usize::try_from(offset).ok().and_then(|o| data.get(o..)) else {
        return 0;
    };
    let n = rest.len().min(buf.len());
    buf[..n].copy_from_slice(&rest[..n]);
    n
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn cache_evicts_least_recently_used() {
        let mut cache = Cache {
            blobs: HashMap::new(),
            bytes: 0,
            cap: 10,
            tick: 0,
        };
        let blob = |n: usize| -> Arc<[u8]> { vec![0u8; n].into() };
        cache.insert([1; 32], blob(4));
        cache.insert([2; 32], blob(4));
        assert!(cache.get(&[1; 32]).is_some());
        cache.insert([3; 32], blob(4));
        assert!(cache.get(&[2; 32]).is_none());
        assert!(cache.get(&[1; 32]).is_some());
        assert_eq!(cache.bytes, 8);
        cache.insert([4; 32], blob(50));
        assert_eq!(cache.blobs.len(), 1);
        assert_eq!(cache.bytes, 50);
    }
}
