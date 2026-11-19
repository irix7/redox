//! `ssh` client for the Redox userland: SSH-2.0, curve25519-sha256,
//! ssh-ed25519 host keys and publickey or password login, one command per
//! session. Every primitive is a pure-Rust crate; the protocol is here.
//!
//!   ssh [-p PORT] [-i KEYFILE] [-K KNOWN_HOSTS] [user@]host command...

mod kex;
mod session;
mod transport;
mod wire;

use std::fs;
use std::io::{self, BufRead, Write};
use std::net::TcpStream;
use std::path::PathBuf;
use std::process::ExitCode;

use base64::engine::general_purpose::STANDARD as B64;
use base64::Engine;
use ed25519_dalek::SigningKey;

use session::{Auth, VERSION};
use wire::{invalid, Reader, Result};

struct Args {
    port: u16,
    key: Option<PathBuf>,
    known_hosts: PathBuf,
    user: String,
    host: String,
    command: Vec<String>,
}

fn usage() -> ! {
    eprintln!("usage: ssh [-p PORT] [-i KEYFILE] [-K KNOWN_HOSTS] [user@]host command...");
    std::process::exit(2);
}

fn parse_args() -> Args {
    let mut port = 22u16;
    let mut key = None;
    let mut known_hosts = std::env::var("HOME")
        .map(|h| PathBuf::from(h).join(".ssh/known_hosts"))
        .unwrap_or_else(|_| PathBuf::from("/root/.ssh/known_hosts"));
    let mut rest = Vec::new();
    let mut it = std::env::args().skip(1);
    while let Some(a) = it.next() {
        match a.as_str() {
            "-p" => port = it.next().and_then(|v| v.parse().ok()).unwrap_or_else(|| usage()),
            "-i" => key = Some(PathBuf::from(it.next().unwrap_or_else(|| usage()))),
            "-K" => known_hosts = PathBuf::from(it.next().unwrap_or_else(|| usage())),
            "-h" | "--help" => usage(),
            _ => {
                rest.push(a);
                rest.extend(it.by_ref());
                break;
            }
        }
    }
    let Some(target) = rest.first().cloned() else { usage() };
    let (user, host) = match target.split_once('@') {
        Some((u, h)) => (u.to_string(), h.to_string()),
        None => (std::env::var("USER").unwrap_or_else(|_| "root".into()), target),
    };
    Args { port, key, known_hosts, user, host, command: rest[1..].to_vec() }
}

/// Parse an unencrypted OpenSSH ed25519 private key file.
fn load_key(path: &PathBuf) -> Result<SigningKey> {
    let text = fs::read_to_string(path)?;
    let body: String = text
        .lines()
        .filter(|l| !l.starts_with("-----"))
        .collect::<Vec<_>>()
        .join("");
    let blob = B64.decode(body).map_err(|_| invalid("key file is not base64"))?;
    let magic = b"openssh-key-v1\0";
    if !blob.starts_with(magic) {
        return Err(invalid("not an OpenSSH private key"));
    }
    let mut r = Reader::new(&blob[magic.len()..]);
    if r.str()? != "none" || r.str()? != "none" {
        return Err(invalid("encrypted keys are not supported"));
    }
    let _kdf_options = r.string()?;
    if r.u32()? != 1 {
        return Err(invalid("key file must hold one key"));
    }
    let _public = r.string()?;
    let private = r.string()?;
    let mut p = Reader::new(private);
    let check1 = p.u32()?;
    let check2 = p.u32()?;
    if check1 != check2 {
        return Err(invalid("key file check bytes differ (wrong passphrase?)"));
    }
    if p.str()? != "ssh-ed25519" {
        return Err(invalid("only ssh-ed25519 keys are supported"));
    }
    let _public = p.string()?;
    let sk = p.string()?;
    if sk.len() != 64 {
        return Err(invalid("bad ed25519 private key length"));
    }
    let seed: [u8; 32] = sk[..32].try_into().map_err(|_| invalid("bad seed"))?;
    Ok(SigningKey::from_bytes(&seed))
}

/// Host key policy: a known host must present the same key. An unknown host
/// is added (trust on first use) and a warning is printed.
fn check_host(path: &PathBuf, host_id: &str, key_blob: &[u8]) -> Result<()> {
    let known = fs::read_to_string(path).unwrap_or_default();
    for line in known.lines() {
        let mut f = line.split_whitespace();
        let (Some(hosts), Some(kind), Some(b64)) = (f.next(), f.next(), f.next()) else {
            continue;
        };
        if kind != "ssh-ed25519" || !hosts.split(',').any(|h| h == host_id) {
            continue;
        }
        let stored = B64.decode(b64).map_err(|_| invalid("known_hosts entry is not base64"))?;
        if stored == key_blob {
            return Ok(());
        }
        return Err(io::Error::new(
            io::ErrorKind::PermissionDenied,
            format!("host key for {host_id} changed; refusing to connect"),
        ));
    }
    if let Some(dir) = path.parent() {
        fs::create_dir_all(dir)?;
    }
    let mut out = fs::OpenOptions::new().create(true).append(true).open(path)?;
    writeln!(out, "{host_id} ssh-ed25519 {}", B64.encode(key_blob))?;
    eprintln!("ssh: added {host_id} to {}", path.display());
    Ok(())
}

fn run(args: Args) -> Result<i32> {
    let addr = format!("{}:{}", args.host, args.port);
    let mut stream = TcpStream::connect(&addr)?;
    session::send_version(&mut stream)?;
    let v_s = session::read_version(&mut stream)?;
    let stream_w = stream.try_clone()?;
    let duplex = Duplex { r: stream, w: stream_w };

    let mut conn = session::connect(duplex, VERSION, &v_s)?;
    let host_id = if args.port == 22 {
        args.host.clone()
    } else {
        format!("[{}]:{}", args.host, args.port)
    };
    check_host(&args.known_hosts, &host_id, &conn.exchanged.host_key)?;

    let auth = match &args.key {
        Some(path) => Auth::Key(load_key(path)?),
        None => {
            eprint!("{}@{}'s password: ", args.user, args.host);
            io::stderr().flush()?;
            let mut line = String::new();
            io::stdin().lock().read_line(&mut line)?;
            Auth::Password(line.trim_end_matches(['\r', '\n']).to_string())
        }
    };
    session::request_service(&mut conn.transport, "ssh-userauth")?;
    session::authenticate(&mut conn.transport, &args.user, &auth, &conn.exchanged.session_id)?;

    if args.command.is_empty() {
        eprintln!("ssh: interactive sessions are not implemented; give a command");
        return Ok(2);
    }
    session::run_command(&mut conn.transport, &args.command.join(" "))
}

/// A TCP stream split into read and write halves, so the transport can
/// implement `Read + Write` over one socket.
struct Duplex {
    r: TcpStream,
    w: TcpStream,
}

impl io::Read for Duplex {
    fn read(&mut self, buf: &mut [u8]) -> io::Result<usize> {
        self.r.read(buf)
    }
}

impl io::Write for Duplex {
    fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
        self.w.write(buf)
    }
    fn flush(&mut self) -> io::Result<()> {
        self.w.flush()
    }
}

fn main() -> ExitCode {
    let args = parse_args();
    match run(args) {
        Ok(status) => ExitCode::from(status as u8),
        Err(e) => {
            eprintln!("ssh: {e}");
            ExitCode::from(255)
        }
    }
}
