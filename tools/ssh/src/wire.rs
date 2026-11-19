//! SSH wire encoding (RFC 4251 section 5): big-endian u32, one-byte booleans,
//! length-prefixed strings and multiprecision integers.

use std::io;

pub type Result<T> = io::Result<T>;

pub fn invalid(msg: &str) -> io::Error {
    io::Error::new(io::ErrorKind::InvalidData, msg.to_string())
}

/// Builds one message payload.
#[derive(Default)]
pub struct Writer {
    pub buf: Vec<u8>,
}

impl Writer {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn u8(&mut self, v: u8) -> &mut Self {
        self.buf.push(v);
        self
    }

    pub fn bool(&mut self, v: bool) -> &mut Self {
        self.u8(v as u8)
    }

    pub fn u32(&mut self, v: u32) -> &mut Self {
        self.buf.extend_from_slice(&v.to_be_bytes());
        self
    }

    pub fn bytes(&mut self, v: &[u8]) -> &mut Self {
        self.buf.extend_from_slice(v);
        self
    }

    pub fn string(&mut self, v: &[u8]) -> &mut Self {
        self.u32(v.len() as u32);
        self.bytes(v)
    }

    pub fn str(&mut self, v: &str) -> &mut Self {
        self.string(v.as_bytes())
    }

    /// An mpint for a non-negative value given as big-endian bytes: leading
    /// zeros are dropped, and a 0x00 is added when the high bit is set.
    pub fn mpint(&mut self, v: &[u8]) -> &mut Self {
        let mut v = v;
        while let [0, rest @ ..] = v {
            v = rest;
        }
        if v.is_empty() {
            return self.u32(0);
        }
        if v[0] & 0x80 != 0 {
            self.u32(v.len() as u32 + 1);
            self.u8(0);
        } else {
            self.u32(v.len() as u32);
        }
        self.bytes(v)
    }
}

/// Reads one message payload.
pub struct Reader<'a> {
    data: &'a [u8],
    pos: usize,
}

impl<'a> Reader<'a> {
    pub fn new(data: &'a [u8]) -> Self {
        Self { data, pos: 0 }
    }

    fn take(&mut self, n: usize) -> Result<&'a [u8]> {
        let end = self.pos.checked_add(n).ok_or_else(|| invalid("length overflow"))?;
        if end > self.data.len() {
            return Err(invalid("message truncated"));
        }
        let out = &self.data[self.pos..end];
        self.pos = end;
        Ok(out)
    }

    pub fn u8(&mut self) -> Result<u8> {
        Ok(self.take(1)?[0])
    }

    pub fn skip(&mut self, n: usize) -> Result<()> {
        self.take(n).map(|_| ())
    }

    pub fn bool(&mut self) -> Result<bool> {
        Ok(self.u8()? != 0)
    }

    pub fn u32(&mut self) -> Result<u32> {
        let b = self.take(4)?;
        Ok(u32::from_be_bytes([b[0], b[1], b[2], b[3]]))
    }

    pub fn string(&mut self) -> Result<&'a [u8]> {
        let n = self.u32()? as usize;
        self.take(n)
    }

    pub fn str(&mut self) -> Result<&'a str> {
        std::str::from_utf8(self.string()?).map_err(|_| invalid("string is not UTF-8"))
    }

    pub fn rest(&mut self) -> &'a [u8] {
        let out = &self.data[self.pos..];
        self.pos = self.data.len();
        out
    }
}
