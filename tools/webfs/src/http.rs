//! HTTP/1.1 GET client: one keep-alive connection, plain or TLS.

use std::io::{self, Read, Write};
use std::net::{IpAddr, TcpStream};
use std::thread;
use std::time::{Duration, Instant};

use crate::dns;
use crate::tls::TlsStream;
use std::sync::Arc;

use crate::trust::TrustStore;

const ATTEMPTS: u32 = 3;
const IO_TIMEOUT: Duration = Duration::from_secs(30);
/// A kept connection is reused only this soon after its last exchange.
const IDLE_REUSE: Duration = Duration::from_secs(2);
/// Longest response header accepted.
const MAX_HEAD: usize = 16 * 1024;

fn invalid(message: &'static str) -> io::Error {
    io::Error::new(io::ErrorKind::InvalidData, message)
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Url {
    pub tls: bool,
    pub host: String,
    pub port: u16,
    /// Absolute path, starting with `/`.
    pub path: String,
}

impl Url {
    /// Parse `http://host[:port][/path]` or `https://...`.
    pub fn parse(text: &str) -> Option<Url> {
        let (tls, rest) = if let Some(rest) = text.strip_prefix("https://") {
            (true, rest)
        } else {
            (false, text.strip_prefix("http://")?)
        };
        let (authority, path) = match rest.find('/') {
            Some(i) => (&rest[..i], &rest[i..]),
            None => (rest, "/"),
        };
        let (host, port) = match authority.rsplit_once(':') {
            Some((host, port)) => (host, port.parse().ok()?),
            None => (authority, if tls { 443 } else { 80 }),
        };
        if host.is_empty() || host.contains(['@', '[']) {
            return None;
        }
        Some(Url {
            tls,
            host: host.to_string(),
            port,
            path: path.to_string(),
        })
    }

    /// Path of `rel` below this URL taken as a directory.
    pub fn join(&self, rel: &str) -> String {
        format!("{}/{rel}", self.path.trim_end_matches('/'))
    }

    fn host_header(&self) -> String {
        let default = if self.tls { 443 } else { 80 };
        if self.port == default {
            self.host.clone()
        } else {
            format!("{}:{}", self.host, self.port)
        }
    }
}

enum Stream {
    Plain(TcpStream),
    Tls(Box<TlsStream>),
}

impl Stream {
    fn read(&mut self, buf: &mut [u8]) -> io::Result<usize> {
        match self {
            Stream::Plain(s) => s.read(buf),
            Stream::Tls(s) => s.read(buf),
        }
    }

    fn send(&mut self, data: &[u8]) -> io::Result<()> {
        match self {
            Stream::Plain(s) => s.write_all(data),
            Stream::Tls(s) => {
                s.write_all(data)?;
                s.flush()
            }
        }
    }
}

/// A connection and the bytes read from it that no response has used yet.
struct Conn {
    stream: Stream,
    buffer: Vec<u8>,
}

impl Conn {
    /// Read more bytes into the buffer; an error at end of stream.
    fn fill(&mut self) -> io::Result<()> {
        let old = self.buffer.len();
        self.buffer.resize(old + 4096, 0);
        let n = self.stream.read(&mut self.buffer[old..]);
        self.buffer.truncate(old + *n.as_ref().unwrap_or(&0));
        match n? {
            0 => Err(io::ErrorKind::UnexpectedEof.into()),
            _ => Ok(()),
        }
    }

    /// Take everything up to and including `end` out of the buffer.
    fn take_through(&mut self, end: &[u8], limit: usize) -> io::Result<Vec<u8>> {
        let mut searched = 0usize;
        loop {
            let from = searched.saturating_sub(end.len());
            if let Some(i) = self.buffer[from..].windows(end.len()).position(|w| w == end) {
                let rest = self.buffer.split_off(from + i + end.len());
                return Ok(std::mem::replace(&mut self.buffer, rest));
            }
            searched = self.buffer.len();
            if searched > limit {
                return Err(invalid("HTTP: header too long"));
            }
            self.fill()?;
        }
    }

    /// Move `len` body bytes to `out`.
    fn take_exact(&mut self, len: usize, out: &mut Vec<u8>) -> io::Result<()> {
        while self.buffer.len() < len {
            self.fill()?;
        }
        out.extend(self.buffer.drain(..len));
        Ok(())
    }

    fn line(&mut self) -> io::Result<String> {
        let line = self.take_through(b"\r\n", MAX_HEAD)?;
        String::from_utf8(line[..line.len() - 2].to_vec()).map_err(|_| invalid("HTTP: bad line"))
    }
}

pub struct Response {
    pub status: u16,
    pub body: Vec<u8>,
}

/// Read one response. The flag is true when the connection can carry
/// another request.
fn read_response(conn: &mut Conn, limit: usize) -> io::Result<(Response, bool)> {
    let head = conn.take_through(b"\r\n\r\n", MAX_HEAD)?;
    let head = std::str::from_utf8(&head).map_err(|_| invalid("HTTP: bad header"))?;
    let mut lines = head.split("\r\n");
    let mut status_line = lines.next().unwrap_or("").split(' ');
    let version = status_line.next().unwrap_or("");
    let status: u16 = status_line
        .next()
        .and_then(|s| s.parse().ok())
        .ok_or_else(|| invalid("HTTP: bad status line"))?;
    if !version.starts_with("HTTP/1.") {
        return Err(invalid("HTTP: not an HTTP/1 response"));
    }

    let mut keep = version == "HTTP/1.1";
    let mut length = None;
    let mut chunked = false;
    for line in lines {
        let Some((name, value)) = line.split_once(':') else {
            continue;
        };
        let value = value.trim();
        if name.eq_ignore_ascii_case("content-length") {
            length = Some(value.parse::<usize>().map_err(|_| invalid("HTTP: bad length"))?);
        } else if name.eq_ignore_ascii_case("transfer-encoding") {
            chunked = value.to_ascii_lowercase().contains("chunked");
        } else if name.eq_ignore_ascii_case("connection") {
            keep = !value.eq_ignore_ascii_case("close");
        }
    }

    let too_large = || invalid("HTTP: body larger than the limit");
    let mut body = Vec::new();
    if chunked {
        loop {
            let line = conn.line()?;
            let size = line.split(';').next().unwrap_or("").trim();
            let size = usize::from_str_radix(size, 16).map_err(|_| invalid("HTTP: bad chunk"))?;
            if size == 0 {
                while !conn.line()?.is_empty() {}
                break;
            }
            if body.len() + size > limit {
                return Err(too_large());
            }
            conn.take_exact(size, &mut body)?;
            conn.line()?;
        }
    } else if let Some(length) = length {
        if length > limit {
            return Err(too_large());
        }
        body.reserve_exact(length);
        conn.take_exact(length, &mut body)?;
    } else {
        // No length: the body runs to the end of the connection.
        keep = false;
        loop {
            if conn.buffer.len() > limit {
                return Err(too_large());
            }
            match conn.fill() {
                Ok(()) => {}
                Err(err) if err.kind() == io::ErrorKind::UnexpectedEof => break,
                Err(err) => return Err(err),
            }
        }
        body = std::mem::take(&mut conn.buffer);
    }
    Ok((Response { status, body }, keep))
}

#[derive(Default, Clone, Copy, Debug)]
pub struct Stats {
    pub requests: u64,
    pub connects: u64,
    pub body_bytes: u64,
}

pub struct Client {
    pub url: Url,
    trust: Arc<TrustStore>,
    /// Connect to this address instead of looking the host up.
    address: Option<IpAddr>,
    conn: Option<Conn>,
    /// When the connection last finished an exchange.
    last_used: Instant,
    pub stats: Stats,
}

impl Client {
    pub fn new(url: Url, trust: Arc<TrustStore>, address: Option<IpAddr>) -> Self {
        Client {
            url,
            trust,
            address,
            conn: None,
            last_used: Instant::now(),
            stats: Stats::default(),
        }
    }

    fn connect(&mut self) -> io::Result<Conn> {
        let address = match self.address {
            Some(address) => address,
            None => dns::resolve(&self.url.host)?,
        };
        let tcp = TcpStream::connect((address, self.url.port))?;
        // Not every network stack has these options; the request still works.
        let _ = tcp.set_read_timeout(Some(IO_TIMEOUT));
        let _ = tcp.set_write_timeout(Some(IO_TIMEOUT));
        let _ = tcp.set_nodelay(true);
        let stream = if self.url.tls {
            Stream::Tls(Box::new(TlsStream::connect(tcp, &self.url.host, &self.trust)?))
        } else {
            Stream::Plain(tcp)
        };
        self.stats.connects += 1;
        Ok(Conn {
            stream,
            buffer: Vec::new(),
        })
    }

    fn attempt(&mut self, path: &str, limit: usize) -> io::Result<Response> {
        // A connection that sat idle is not trusted: a NAT or a server may
        // have dropped it without telling us, and the request would wait for
        // the whole I/O timeout before the retry. A new connection costs one
        // round trip.
        let mut conn = match self.conn.take() {
            Some(conn) if self.last_used.elapsed() < IDLE_REUSE => conn,
            _ => self.connect()?,
        };
        let request = format!(
            "GET {path} HTTP/1.1\r\nHost: {}\r\nUser-Agent: webfs/0.1\r\nAccept-Encoding: identity\r\n\r\n",
            self.url.host_header()
        );
        conn.stream.send(request.as_bytes())?;
        let (response, keep) = read_response(&mut conn, limit)?;
        if keep {
            self.conn = Some(conn);
            self.last_used = Instant::now();
        }
        self.stats.requests += 1;
        self.stats.body_bytes += response.body.len() as u64;
        Ok(response)
    }

    /// GET an absolute path on the server. Any status is returned; a failed
    /// exchange is retried on a new connection.
    pub fn request(&mut self, path: &str, limit: usize) -> io::Result<Response> {
        let mut last = None;
        for attempt in 0..ATTEMPTS {
            if attempt > 0 {
                thread::sleep(Duration::from_millis(250 * u64::from(attempt)));
            }
            match self.attempt(path, limit) {
                Ok(response) if response.status >= 500 => {
                    last = Some(io::Error::other(format!("HTTP status {}", response.status)));
                }
                Ok(response) => return Ok(response),
                Err(err) => last = Some(err),
            }
        }
        Err(last.expect("at least one attempt ran"))
    }

    /// GET `rel` below the base URL and return the body of a 200 response.
    pub fn get(&mut self, rel: &str, limit: usize) -> io::Result<Vec<u8>> {
        let response = self.request(&self.url.join(rel), limit)?;
        match response.status {
            200 => Ok(response.body),
            404 => Err(io::Error::new(io::ErrorKind::NotFound, format!("HTTP 404 for {rel}"))),
            status => Err(io::Error::other(format!("HTTP status {status} for {rel}"))),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn url_forms() {
        let url = Url::parse("https://example.com/indy/").unwrap();
        assert_eq!((url.tls, url.host.as_str(), url.port), (true, "example.com", 443));
        assert_eq!(url.join("root"), "/indy/root");
        let url = Url::parse("http://10.1.1.3:8080").unwrap();
        assert_eq!((url.tls, url.port, url.join("root").as_str()), (false, 8080, "/root"));
        assert_eq!(url.host_header(), "10.1.1.3:8080");
        assert!(Url::parse("ftp://example.com/").is_none());
    }
}
