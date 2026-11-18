//! The Redox scheme: a read-only file tree backed by a `Store`.
//!
//! The main thread reads requests from the scheme socket and hands each call
//! to a worker thread, so a read that waits for the network does not hold up
//! other callers. Workers share the store, its cache and the handle table.

use std::collections::BTreeMap;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::mpsc::{self, Receiver};
use std::sync::{Arc, Mutex};
use std::thread;

use redox_scheme::scheme::{Op, SchemeResponse, SchemeState, SchemeSync};
use redox_scheme::{CallerCtx, OpenResult, RequestKind, Response, SignalBehavior, Socket};
use scheme_utils::FpathWriter;
use syscall::data::{Stat, StatVfs};
use syscall::dirent::{DirEntry, DirentBuf, DirentKind};
use syscall::error::{
    EAGAIN, EBADF, EIO, EISDIR, ELOOP, ENOENT, ENOSYS, ENOTDIR, EOPNOTSUPP, EROFS, EXDEV, Error, Result,
};
use syscall::flag::{
    MODE_DIR, MODE_FILE, MODE_SYMLINK, O_ACCMODE, O_CREAT, O_DIRECTORY, O_NOFOLLOW, O_RDONLY, O_STAT,
    O_SYMLINK, O_TRUNC,
};
use syscall::schemev2::NewFdFlags;

use crate::format::{CHUNK, Entry, Kind};
use crate::store::{self, Store};

const ROOT_ID: usize = 1;

struct Handle {
    /// Path from the root of the tree, without a leading slash.
    path: String,
    entry: Entry,
    /// Set when the path crosses a link into another scheme before its last
    /// component (`/tmp/x` with `/tmp` -> `/scheme/tmp`): the path outside,
    /// which `fpath` reports.
    outside: Option<String>,
}

/// Worker stack: the TLS handshake and RSA-4096 checks keep their buffers
/// on the stack.
const WORKER_STACK: usize = 512 * 1024;

struct Shared {
    name: String,
    store: Store,
    handles: Mutex<BTreeMap<usize, Handle>>,
    next_id: AtomicUsize,
}

/// One per thread; all of them point at the same `Shared`.
#[derive(Clone)]
pub struct WebScheme {
    shared: Arc<Shared>,
    /// False on the dispatch thread: it answers what the cache holds and
    /// hands the rest to a worker (`run`).
    fetch: bool,
}

fn errno(err: store::Error) -> Error {
    Error::new(match err {
        store::Error::NotFound => ENOENT,
        store::Error::NotDir => ENOTDIR,
        store::Error::IsDir => EISDIR,
        store::Error::Loop => ELOOP,
        store::Error::CrossScheme(_) => EXDEV,
        // Never reaches a caller: `run` passes the request to a worker.
        store::Error::WouldFetch => EAGAIN,
        store::Error::Io(text) => {
            eprintln!("webfs: {text}");
            EIO
        }
    })
}

impl WebScheme {
    pub fn new(name: String, store: Store) -> Self {
        WebScheme {
            shared: Arc::new(Shared {
                name,
                store,
                handles: Mutex::new(BTreeMap::new()),
                next_id: AtomicUsize::new(ROOT_ID + 1),
            }),
            fetch: true,
        }
    }

    fn handles(&self) -> std::sync::MutexGuard<'_, BTreeMap<usize, Handle>> {
        self.shared.handles.lock().unwrap_or_else(|poisoned| poisoned.into_inner())
    }

    fn path(&self, id: usize) -> Result<String> {
        if id == ROOT_ID {
            return Ok(String::new());
        }
        Ok(self.handles().get(&id).ok_or(Error::new(EBADF))?.path.clone())
    }

    /// The entry behind an id. The root id is resolved on each use: it
    /// exists before the store has been reached.
    fn entry(&self, id: usize) -> Result<Entry> {
        if id == ROOT_ID {
            return self.shared.store.resolve("", true, self.fetch).map_err(errno);
        }
        Ok(self.handles().get(&id).ok_or(Error::new(EBADF))?.entry.clone())
    }

    /// Serve the scheme on `socket` with `threads` workers. Does not return.
    ///
    /// This thread reads the requests. It answers a request itself when the
    /// cache holds everything the answer needs, which costs the caller two
    /// context switches. A request that needs the server goes to a worker,
    /// so a read that waits for the network does not hold up other callers.
    pub fn run(self, socket: Socket, threads: usize) -> ! {
        let socket = Arc::new(socket);
        let (sender, receiver) = mpsc::channel::<Job>();
        let receiver = Arc::new(Mutex::new(receiver));
        for index in 0..threads.max(1) {
            let (mut scheme, socket, receiver) = (self.clone(), socket.clone(), receiver.clone());
            thread::Builder::new()
                .name(format!("webfs-{index}"))
                .stack_size(WORKER_STACK)
                .spawn(move || worker(&mut scheme, &socket, &receiver))
                .expect("webfs: failed to start a worker thread");
        }
        let mut scheme = self;
        scheme.fetch = false;
        let mut state = SchemeState::new();
        loop {
            let request = match socket.next_request(SignalBehavior::Restart) {
                Ok(Some(request)) => request,
                Ok(None) => std::process::exit(0),
                Err(err) => panic!("webfs: failed to read a scheme request: {err}"),
            };
            let response = match request.kind() {
                RequestKind::Call(call) => {
                    let caller = call.caller();
                    match call.op() {
                        Ok(mut op) => {
                            let outcome = op.handle_sync_dont_consume(&caller, &mut scheme, &mut state);
                            if needs_fetch(&outcome) {
                                sender.send(Job(caller, op)).expect("webfs: the workers are gone");
                                continue;
                            }
                            match outcome {
                                SchemeResponse::Opened(open) => Response::open_dup_like(open, op),
                                SchemeResponse::RegularAndNotifyOnDetach(status) => {
                                    Response::new_notify_on_detach(status, op)
                                }
                                SchemeResponse::Regular(status) => Response::new(status, op),
                            }
                        }
                        Err(call) => Response::new(Err(Error::new(ENOSYS)), call),
                    }
                }
                RequestKind::OnClose { id } => {
                    scheme.on_close(id);
                    continue;
                }
                RequestKind::SendFd(request) => Response::new(Err(Error::new(EOPNOTSUPP)), request),
                RequestKind::RecvFd(request) => {
                    Response::open_dup_like(Err(Error::new(EOPNOTSUPP)), request)
                }
                _ => continue,
            };
            let _ = socket.write_response(response, SignalBehavior::Restart);
        }
    }
}

/// A request the dispatch thread could not answer from the cache.
struct Job(CallerCtx, Op);

// The request points into the caller's buffers, which the kernel keeps
// mapped until the response is written; one thread at a time holds it.
unsafe impl Send for Job {}

/// True when the handler stopped at a blob the cache does not hold.
fn needs_fetch(outcome: &SchemeResponse) -> bool {
    let errno = match outcome {
        SchemeResponse::Regular(Err(err)) | SchemeResponse::RegularAndNotifyOnDetach(Err(err)) => err.errno,
        SchemeResponse::Opened(Err(err)) => err.errno,
        _ => return false,
    };
    errno == EAGAIN
}

fn worker(scheme: &mut WebScheme, socket: &Socket, receiver: &Mutex<Receiver<Job>>) {
    // File locks are not offered, so each worker can keep its own state.
    let mut state = SchemeState::new();
    loop {
        let job = receiver.lock().unwrap_or_else(|poisoned| poisoned.into_inner()).recv();
        let Ok(Job(caller, op)) = job else { return };
        let response = op.handle_sync(caller, scheme, &mut state);
        if let Err(err) = socket.write_response(response, SignalBehavior::Restart) {
            eprintln!("webfs: failed to write a scheme response: {err}");
        }
    }
}

impl SchemeSync for WebScheme {
    fn scheme_root(&mut self) -> Result<usize> {
        Ok(ROOT_ID)
    }

    fn openat(
        &mut self,
        dirfd: usize,
        path: &str,
        flags: usize,
        _fcntl_flags: u32,
        _ctx: &CallerCtx,
    ) -> Result<OpenResult> {
        let base = self.path(dirfd)?;
        let stat = flags & O_STAT != 0;
        // An open with no access mode (O_SYMLINK alone, to read a link) is
        // not a write.
        let write = !stat && (flags & O_ACCMODE & !O_RDONLY != 0 || flags & (O_CREAT | O_TRUNC) != 0);
        let full = format!("{base}/{path}");
        let follow = flags & (O_SYMLINK | O_NOFOLLOW) == 0;
        // Resolve before refusing a write: a link into another scheme
        // (`/dev/null`) must answer EXDEV so the caller opens the target,
        // which may well be writable.
        let mut outside = None;
        let entry = match self.shared.store.resolve(&full, follow, self.fetch) {
            Ok(entry) => entry,
            // relibc reads a link by opening it with O_SYMLINK and asking for
            // its path. Answer for a link in the middle of the path as well.
            Err(store::Error::CrossScheme(target)) if flags & O_SYMLINK != 0 => {
                let entry = Entry {
                    kind: Kind::Link,
                    mode: 0o777,
                    size: target.len() as u64,
                    hash: Default::default(),
                    name: String::new(),
                };
                outside = Some(target);
                entry
            }
            Err(store::Error::NotFound) if write => return Err(Error::new(EROFS)),
            Err(err) => return Err(errno(err)),
        };
        if write && outside.is_none() {
            return Err(Error::new(EROFS));
        }
        if flags & O_DIRECTORY != 0 && entry.kind != Kind::Dir {
            return Err(Error::new(ENOTDIR));
        }
        let path: Vec<&str> = full.split('/').filter(|part| !part.is_empty() && *part != ".").collect();
        let id = self.shared.next_id.fetch_add(1, Ordering::Relaxed);
        self.handles().insert(
            id,
            Handle {
                path: path.join("/"),
                entry,
                outside,
            },
        );
        Ok(OpenResult::ThisScheme {
            number: id,
            flags: NewFdFlags::POSITIONED,
        })
    }

    fn read(
        &mut self,
        id: usize,
        buf: &mut [u8],
        offset: u64,
        _fcntl_flags: u32,
        _ctx: &CallerCtx,
    ) -> Result<usize> {
        let entry = self.entry(id)?;
        self.shared.store.read(&entry, offset, buf, self.fetch).map_err(errno)
    }

    fn write(
        &mut self,
        _id: usize,
        _buf: &[u8],
        _offset: u64,
        _fcntl_flags: u32,
        _ctx: &CallerCtx,
    ) -> Result<usize> {
        Err(Error::new(EROFS))
    }

    fn fsize(&mut self, id: usize, _ctx: &CallerCtx) -> Result<u64> {
        Ok(self.entry(id)?.size)
    }

    fn fcntl(&mut self, _id: usize, _cmd: usize, _arg: usize, _ctx: &CallerCtx) -> Result<usize> {
        Ok(0)
    }

    fn fpath(&mut self, id: usize, buf: &mut [u8], _ctx: &CallerCtx) -> Result<usize> {
        // relibc follows a link into another scheme by opening the link
        // itself and asking for its path: answer with the target.
        let entry = self.entry(id)?;
        let outside = self.handles().get(&id).and_then(|handle| handle.outside.clone());
        if let Some(target) = outside {
            let bytes = target.as_bytes();
            let n = bytes.len().min(buf.len());
            buf[..n].copy_from_slice(&bytes[..n]);
            return Ok(n);
        }
        if entry.kind == Kind::Link {
            let target = self.shared.store.target(&entry, self.fetch).map_err(errno)?;
            if target.starts_with("/scheme/") {
                let bytes = target.as_bytes();
                let n = bytes.len().min(buf.len());
                buf[..n].copy_from_slice(&bytes[..n]);
                return Ok(n);
            }
        }
        let path = self.path(id)?;
        FpathWriter::with(buf, &self.shared.name, |writer| {
            writer.push_str(&path);
            Ok(())
        })
    }

    fn fstat(&mut self, id: usize, stat: &mut Stat, _ctx: &CallerCtx) -> Result<()> {
        let entry = self.entry(id)?;
        let time = self.shared.store.root(self.fetch).map_err(errno)?.time;
        let kind = match entry.kind {
            Kind::Dir => MODE_DIR,
            Kind::File => MODE_FILE,
            Kind::Link => MODE_SYMLINK,
        };
        *stat = Stat {
            st_ino: u64::from_be_bytes(entry.hash[..8].try_into().expect("a hash has 32 bytes")),
            st_mode: kind | entry.mode,
            st_nlink: 1,
            st_size: entry.size,
            st_blksize: CHUNK as u32,
            st_blocks: entry.size.div_ceil(512),
            st_mtime: time,
            st_atime: time,
            st_ctime: time,
            ..Default::default()
        };
        Ok(())
    }

    fn fstatvfs(&mut self, _id: usize, stat: &mut StatVfs, _ctx: &CallerCtx) -> Result<()> {
        *stat = StatVfs {
            f_bsize: CHUNK as u32,
            f_blocks: 0,
            f_bfree: 0,
            f_bavail: 0,
        };
        Ok(())
    }

    fn getdents<'buf>(
        &mut self,
        id: usize,
        mut buf: DirentBuf<&'buf mut [u8]>,
        opaque_offset: u64,
    ) -> Result<DirentBuf<&'buf mut [u8]>> {
        let dir = self.entry(id)?;
        let entries = self.shared.store.list(&dir, self.fetch).map_err(errno)?;
        let start = usize::try_from(opaque_offset).unwrap_or(usize::MAX);
        for (index, entry) in entries.iter().enumerate().skip(start) {
            let full = buf.entry(DirEntry {
                inode: u64::from_be_bytes(entry.hash[..8].try_into().expect("a hash has 32 bytes")),
                name: &entry.name,
                kind: match entry.kind {
                    Kind::Dir => DirentKind::Directory,
                    Kind::File => DirentKind::Regular,
                    Kind::Link => DirentKind::Symlink,
                },
                next_opaque_id: index as u64 + 1,
            });
            match full {
                Ok(()) => {}
                // The buffer is full: the caller comes back for the rest.
                Err(_) if index > start => break,
                Err(err) => return Err(err),
            }
        }
        Ok(buf)
    }

    fn on_close(&mut self, id: usize) {
        self.handles().remove(&id);
    }
}
