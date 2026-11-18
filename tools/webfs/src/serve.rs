//! Origin server for a store: plain HTTP/1.1 for a reverse proxy or a
//! Cloudflare Tunnel to sit in front of.
//!
//! Two kinds of path are served and nothing else: `/root` and
//! `/blob/ab/<64 hex digits>`. Blobs carry a one-year immutable
//! `Cache-Control`; `root` carries a short one, so a published tree reaches
//! clients within `root_max_age` seconds.

use std::fs;
use std::io::{self, BufRead, BufReader, Write};
use std::net::{TcpListener, TcpStream};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::thread;
use std::time::Duration;

use crate::format;

const IDLE: Duration = Duration::from_secs(60);

pub struct Config {
    pub dir: PathBuf,
    pub root_max_age: u32,
    pub log: bool,
}

/// The file and `Cache-Control` value for a request path, or `None` when the
/// path is not part of a store.
fn route(config: &Config, path: &str) -> Option<(PathBuf, String)> {
    let path = path.split('?').next().unwrap_or("");
    if path == "/root" {
        let cache = format!("public, max-age={}", config.root_max_age);
        return Some((config.dir.join("root"), cache));
    }
    let mut parts = path.strip_prefix("/blob/")?.split('/');
    let (fan, name) = (parts.next()?, parts.next()?);
    if parts.next().is_some() || format::parse_hex(name).is_none() || fan != &name[..2] {
        return None;
    }
    let cache = "public, max-age=31536000, immutable".to_string();
    Some((config.dir.join("blob").join(fan).join(name), cache))
}

fn respond(
    stream: &mut TcpStream,
    status: &str,
    cache: &str,
    body: &[u8],
    send_body: bool,
) -> io::Result<()> {
    let head = format!(
        "HTTP/1.1 {status}\r\nContent-Type: application/octet-stream\r\nContent-Length: {}\r\nCache-Control: {cache}\r\n\r\n",
        body.len()
    );
    // One write: a header sent alone makes the body's last, partial segment
    // wait for the client's acknowledgement, which a client that delays
    // acknowledgements sends late.
    let mut response = head.into_bytes();
    if send_body {
        response.extend_from_slice(body);
    }
    stream.write_all(&response)?;
    stream.flush()
}

fn serve_connection(config: &Config, stream: TcpStream) -> io::Result<()> {
    stream.set_read_timeout(Some(IDLE))?;
    stream.set_nodelay(true)?;
    let peer = stream.peer_addr()?;
    let mut writer = stream.try_clone()?;
    let mut reader = BufReader::new(stream);
    loop {
        let mut request = String::new();
        if reader.read_line(&mut request)? == 0 {
            return Ok(());
        }
        let mut close = false;
        loop {
            let mut header = String::new();
            if reader.read_line(&mut header)? == 0 {
                return Ok(());
            }
            let header = header.trim_end();
            if header.is_empty() {
                break;
            }
            if let Some((name, value)) = header.split_once(':') {
                close |= name.eq_ignore_ascii_case("connection")
                    && value.trim().eq_ignore_ascii_case("close");
            }
        }
        let mut words = request.split_whitespace();
        let (method, path) = (words.next().unwrap_or(""), words.next().unwrap_or(""));
        let send_body = method != "HEAD";
        let status = if method != "GET" && method != "HEAD" {
            respond(&mut writer, "405 Method Not Allowed", "no-store", b"", send_body)?;
            405
        } else {
            match route(config, path).and_then(|(file, cache)| Some((fs::read(file).ok()?, cache))) {
                Some((body, cache)) => {
                    respond(&mut writer, "200 OK", &cache, &body, send_body)?;
                    200
                }
                None => {
                    respond(&mut writer, "404 Not Found", "no-store", b"", send_body)?;
                    404
                }
            }
        };
        if config.log {
            eprintln!("webfs: {peer} {method} {path} {status}");
        }
        if close {
            return Ok(());
        }
    }
}

/// Serve the store in `config.dir` on `listen` until the process is killed.
pub fn serve(config: Config, listen: &str) -> io::Result<()> {
    if !Path::new(&config.dir).join("root").is_file() {
        return Err(io::Error::other(format!("{}: no root file; run webfs pack first", config.dir.display())));
    }
    let listener = TcpListener::bind(listen)?;
    eprintln!("webfs: serving {} on http://{}", config.dir.display(), listener.local_addr()?);
    let config = Arc::new(config);
    for stream in listener.incoming() {
        let Ok(stream) = stream else { continue };
        let config = config.clone();
        thread::spawn(move || {
            // A client that goes away mid-request is not worth a message.
            let _ = serve_connection(&config, stream);
        });
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn only_store_paths_route() {
        let config = Config {
            dir: PathBuf::from("/srv/store"),
            root_max_age: 60,
            log: false,
        };
        let name = format::hex(&format::sha256(b"x"));
        let good = format!("/blob/{}/{name}", &name[..2]);
        assert!(route(&config, &good).unwrap().1.contains("immutable"));
        assert_eq!(route(&config, "/root?x=1").unwrap().1, "public, max-age=60");
        assert!(route(&config, &format!("/blob/zz/{name}")).is_none());
        assert!(route(&config, "/blob/../root").is_none());
        assert!(route(&config, "/etc/passwd").is_none());
    }
}
