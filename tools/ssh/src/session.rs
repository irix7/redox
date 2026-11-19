//! Connection after key exchange: service request, user authentication
//! (publickey with ed25519, or password) and one `session` channel that runs
//! a command (RFC 4252 and RFC 4254).

use std::io::{self, Read, Write};

use ed25519_dalek::{Signer, SigningKey};

use crate::kex::{self, Exchanged};
use crate::transport::Transport;
use crate::wire::{invalid, Reader, Result, Writer};

pub const VERSION: &str = "SSH-2.0-redox_ssh_0.1";

const MSG_DISCONNECT: u8 = 1;
const MSG_SERVICE_REQUEST: u8 = 5;
const MSG_SERVICE_ACCEPT: u8 = 6;
const MSG_USERAUTH_REQUEST: u8 = 50;
const MSG_USERAUTH_FAILURE: u8 = 51;
const MSG_USERAUTH_SUCCESS: u8 = 52;
const MSG_USERAUTH_BANNER: u8 = 53;
const MSG_USERAUTH_PK_OK: u8 = 60;
const MSG_GLOBAL_REQUEST: u8 = 80;
const MSG_REQUEST_FAILURE: u8 = 82;
const MSG_CHANNEL_OPEN: u8 = 90;
const MSG_CHANNEL_OPEN_CONFIRMATION: u8 = 91;
const MSG_CHANNEL_OPEN_FAILURE: u8 = 92;
const MSG_CHANNEL_WINDOW_ADJUST: u8 = 93;
const MSG_CHANNEL_DATA: u8 = 94;
const MSG_CHANNEL_EXTENDED_DATA: u8 = 95;
const MSG_CHANNEL_EOF: u8 = 96;
const MSG_CHANNEL_CLOSE: u8 = 97;
const MSG_CHANNEL_REQUEST: u8 = 98;
const MSG_CHANNEL_SUCCESS: u8 = 99;
const MSG_CHANNEL_FAILURE: u8 = 100;

const WINDOW: u32 = 2 * 1024 * 1024;
const MAX_PACKET_SIZE: u32 = 32 * 1024;

pub enum Auth {
    Key(SigningKey),
    Password(String),
}

/// Read the server's identification line (skipping any banner lines before
/// it), byte by byte, so nothing after it is consumed.
pub fn read_version<S: Read>(s: &mut S) -> Result<String> {
    loop {
        let mut line = Vec::new();
        loop {
            let mut b = [0u8; 1];
            s.read_exact(&mut b)?;
            if b[0] == b'\n' {
                break;
            }
            if line.len() > 255 {
                return Err(invalid("identification line too long"));
            }
            line.push(b[0]);
        }
        if line.last() == Some(&b'\r') {
            line.pop();
        }
        let text = String::from_utf8(line).map_err(|_| invalid("identification is not UTF-8"))?;
        if text.starts_with("SSH-2.0-") {
            return Ok(text);
        }
        if text.starts_with("SSH-") {
            return Err(invalid("server does not speak SSH-2.0"));
        }
    }
}

/// Send our identification line.
pub fn send_version<S: Write>(s: &mut S) -> Result<()> {
    s.write_all(format!("{VERSION}\r\n").as_bytes())?;
    s.flush()
}

/// The packet the client sent or received, for the exchange hash.
pub struct Connected<S: Read + Write> {
    pub transport: Transport<S>,
    pub exchanged: Exchanged,
}

pub fn request_service<S: Read + Write>(t: &mut Transport<S>, name: &str) -> Result<()> {
    let mut w = Writer::new();
    w.u8(MSG_SERVICE_REQUEST).str(name);
    t.send(&w.buf)?;
    let msg = recv_expect(t, &[MSG_SERVICE_ACCEPT])?;
    let _ = msg;
    Ok(())
}

/// Receive the next message that is not a banner, global request or
/// window adjust. `want` lists the numbers the caller accepts.
fn recv_expect<S: Read + Write>(t: &mut Transport<S>, want: &[u8]) -> Result<Vec<u8>> {
    loop {
        let msg = t.recv()?;
        let kind = *msg.first().ok_or_else(|| invalid("empty packet"))?;
        if kind == MSG_DISCONNECT {
            let mut r = Reader::new(&msg[1..]);
            let _code = r.u32()?;
            let text = r.str().unwrap_or("");
            return Err(io::Error::other(format!("server disconnected: {text}")));
        }
        if want.contains(&kind) {
            return Ok(msg);
        }
        // IGNORE (2), DEBUG (4), banners, and requests we do not act on.
        if matches!(kind, 2 | 4) || kind == MSG_USERAUTH_BANNER || kind == MSG_GLOBAL_REQUEST || kind == MSG_REQUEST_FAILURE {
            continue;
        }
        return Err(invalid(&format!("unexpected message {kind}")));
    }
}

/// Authenticate as `user`. For publickey, first ask whether the key is
/// acceptable (probe), then send the signed request.
pub fn authenticate<S: Read + Write>(
    t: &mut Transport<S>,
    user: &str,
    auth: &Auth,
    session_id: &[u8; 32],
) -> Result<()> {
    match auth {
        Auth::Key(key) => {
            let public = key.verifying_key().to_bytes();
            let mut pk_blob = Writer::new();
            pk_blob.str("ssh-ed25519").string(&public);
            let pk_blob = pk_blob.buf;

            let mut probe = Writer::new();
            probe
                .u8(MSG_USERAUTH_REQUEST)
                .str(user)
                .str("ssh-connection")
                .str("publickey")
                .bool(false)
                .str("ssh-ed25519")
                .string(&pk_blob);
            t.send(&probe.buf)?;
            let answer = recv_expect(t, &[MSG_USERAUTH_PK_OK, MSG_USERAUTH_FAILURE])?;
            if answer[0] == MSG_USERAUTH_FAILURE {
                return Err(io::Error::new(io::ErrorKind::PermissionDenied, "key not accepted"));
            }

            // The signature covers the session id and the request without
            // its signature (RFC 4252 section 7).
            let mut signed = Writer::new();
            signed
                .string(session_id)
                .u8(MSG_USERAUTH_REQUEST)
                .str(user)
                .str("ssh-connection")
                .str("publickey")
                .bool(true)
                .str("ssh-ed25519")
                .string(&pk_blob);
            let sig = key.sign(&signed.buf).to_bytes();
            let mut sig_blob = Writer::new();
            sig_blob.str("ssh-ed25519").string(&sig);

            let mut req = Writer::new();
            req.u8(MSG_USERAUTH_REQUEST)
                .str(user)
                .str("ssh-connection")
                .str("publickey")
                .bool(true)
                .str("ssh-ed25519")
                .string(&pk_blob)
                .string(&sig_blob.buf);
            t.send(&req.buf)?;
        }
        Auth::Password(password) => {
            let mut req = Writer::new();
            req.u8(MSG_USERAUTH_REQUEST)
                .str(user)
                .str("ssh-connection")
                .str("password")
                .bool(false)
                .str(password);
            t.send(&req.buf)?;
        }
    }
    recv_expect(t, &[MSG_USERAUTH_SUCCESS, MSG_USERAUTH_FAILURE]).and_then(|m| {
        if m[0] == MSG_USERAUTH_SUCCESS {
            Ok(())
        } else {
            Err(io::Error::new(io::ErrorKind::PermissionDenied, "authentication failed"))
        }
    })
}

/// Run `command` on one session channel. Returns the remote exit status.
pub fn run_command<S: Read + Write>(t: &mut Transport<S>, command: &str) -> Result<i32> {
    let mut open = Writer::new();
    open.u8(MSG_CHANNEL_OPEN).str("session").u32(0).u32(WINDOW).u32(MAX_PACKET_SIZE);
    t.send(&open.buf)?;
    let confirm = recv_expect(t, &[MSG_CHANNEL_OPEN_CONFIRMATION, MSG_CHANNEL_OPEN_FAILURE])?;
    if confirm[0] == MSG_CHANNEL_OPEN_FAILURE {
        return Err(io::Error::other("server refused the session channel"));
    }
    let mut r = Reader::new(&confirm[1..]);
    let _our = r.u32()?;
    let server = r.u32()?;

    let mut req = Writer::new();
    req.u8(MSG_CHANNEL_REQUEST).u32(server).str("exec").bool(true).str(command);
    t.send(&req.buf)?;
    let mut status = 0;
    loop {
        let msg = t.recv()?;
        let kind = *msg.first().ok_or_else(|| invalid("empty packet"))?;
        match kind {
            MSG_CHANNEL_DATA => {
                let mut r = Reader::new(&msg[1..]);
                let _recipient = r.u32()?;
                let data = r.string()?;
                io::stdout().write_all(data)?;
                io::stdout().flush()?;
                adjust(t, server, data.len())?;
            }
            MSG_CHANNEL_EXTENDED_DATA => {
                let mut r = Reader::new(&msg[1..]);
                let _recipient = r.u32()?;
                let _data_type = r.u32()?;
                let data = r.string()?;
                io::stderr().write_all(data)?;
                io::stderr().flush()?;
                adjust(t, server, data.len())?;
            }
            MSG_CHANNEL_REQUEST => {
                let mut r = Reader::new(&msg[1..]);
                let _recipient = r.u32()?;
                let name = r.str()?;
                let _want_reply = r.bool()?;
                if name == "exit-status" {
                    status = r.u32()? as i32;
                }
            }
            MSG_CHANNEL_SUCCESS => {}
            MSG_CHANNEL_FAILURE => return Err(io::Error::other("server refused the command")),
            MSG_CHANNEL_WINDOW_ADJUST | MSG_CHANNEL_EOF => {}
            MSG_CHANNEL_CLOSE => {
                let mut close = Writer::new();
                close.u8(MSG_CHANNEL_CLOSE).u32(server);
                t.send(&close.buf)?;
                return Ok(status);
            }
            other => return Err(invalid(&format!("unexpected channel message {other}"))),
        }
    }
}

/// Tell the server we consumed `n` bytes, so it keeps sending.
fn adjust<S: Read + Write>(t: &mut Transport<S>, server: u32, n: usize) -> Result<()> {
    let mut w = Writer::new();
    w.u8(MSG_CHANNEL_WINDOW_ADJUST).u32(server).u32(n as u32);
    t.send(&w.buf)
}

/// Identification exchange is done by the caller. This sends KEXINIT,
/// checks the server's, and runs the key exchange.
pub fn connect<S: Read + Write>(stream: S, v_c: &str, v_s: &str) -> Result<Connected<S>> {
    let mut t = Transport::new(stream);
    let mut cookie = [0u8; 16];
    rand_core::RngCore::fill_bytes(&mut rand_core::OsRng, &mut cookie);
    let i_c = kex::kexinit_payload(&cookie);
    t.send(&i_c)?;
    let i_s = t.recv()?;
    kex::check_server_kexinit(&i_s)?;
    let exchanged = kex::exchange(&mut t, v_c, v_s, &i_c, &i_s)?;
    Ok(Connected { transport: t, exchanged })
}
