//! Host name lookup.
//!
//! On Redox the client asks the name server the network stack holds in
//! `/scheme/netcfg/resolv/nameserver` (set by `dhcpd`) for an A record over
//! UDP. relibc's resolver reads `/etc/net/dns`, which a machine booted from
//! the initfs does not have. Elsewhere the system resolver is used.

#![cfg_attr(not(target_os = "redox"), allow(dead_code))]

use std::io;
use std::net::{IpAddr, Ipv4Addr, UdpSocket};
use std::time::Duration;

const TYPE_A: u16 = 1;
const CLASS_IN: u16 = 1;
const ATTEMPTS: u32 = 3;
const TIMEOUT: Duration = Duration::from_secs(3);

fn invalid(message: &'static str) -> io::Error {
    io::Error::new(io::ErrorKind::InvalidData, message)
}

/// A query for the A records of `name`, recursion desired.
pub fn encode_query(id: u16, name: &str) -> io::Result<Vec<u8>> {
    let mut packet = Vec::with_capacity(18 + name.len());
    packet.extend_from_slice(&id.to_be_bytes());
    packet.extend_from_slice(&[0x01, 0x00, 0, 1, 0, 0, 0, 0, 0, 0]);
    for label in name.trim_end_matches('.').split('.') {
        if label.is_empty() || label.len() > 63 {
            return Err(invalid("DNS: bad host name"));
        }
        packet.push(label.len() as u8);
        packet.extend_from_slice(label.as_bytes());
    }
    packet.push(0);
    packet.extend_from_slice(&TYPE_A.to_be_bytes());
    packet.extend_from_slice(&CLASS_IN.to_be_bytes());
    Ok(packet)
}

/// Offset just past the name that starts at `pos`.
fn skip_name(packet: &[u8], mut pos: usize) -> Option<usize> {
    loop {
        let len = *packet.get(pos)?;
        if len & 0xc0 == 0xc0 {
            return Some(pos + 2);
        }
        if len == 0 {
            return Some(pos + 1);
        }
        pos += 1 + usize::from(len);
    }
}

/// The first A record in the answer section of a reply to query `id`.
pub fn parse_reply(id: u16, packet: &[u8]) -> io::Result<Ipv4Addr> {
    let u16_at = |pos: usize| {
        packet
            .get(pos..pos + 2)
            .map(|b| u16::from_be_bytes([b[0], b[1]]))
            .ok_or_else(|| invalid("DNS: short reply"))
    };
    if u16_at(0)? != id {
        return Err(invalid("DNS: reply to another query"));
    }
    let flags = u16_at(2)?;
    if flags & 0x8000 == 0 {
        return Err(invalid("DNS: not a reply"));
    }
    if flags & 0x000f != 0 {
        return Err(io::Error::new(io::ErrorKind::NotFound, "DNS: name not found"));
    }
    let questions = u16_at(4)?;
    let answers = u16_at(6)?;
    let mut pos = 12;
    for _ in 0..questions {
        pos = skip_name(packet, pos).ok_or_else(|| invalid("DNS: short reply"))? + 4;
    }
    for _ in 0..answers {
        pos = skip_name(packet, pos).ok_or_else(|| invalid("DNS: short reply"))?;
        let kind = u16_at(pos)?;
        let class = u16_at(pos + 2)?;
        let len = usize::from(u16_at(pos + 8)?);
        let data = packet.get(pos + 10..pos + 10 + len).ok_or_else(|| invalid("DNS: short reply"))?;
        if kind == TYPE_A && class == CLASS_IN && len == 4 {
            return Ok(Ipv4Addr::new(data[0], data[1], data[2], data[3]));
        }
        pos += 10 + len;
    }
    Err(io::Error::new(io::ErrorKind::NotFound, "DNS: no address for the name"))
}

/// Ask `server` for the IPv4 address of `name`.
pub fn query(server: IpAddr, name: &str) -> io::Result<Ipv4Addr> {
    let socket = UdpSocket::bind((Ipv4Addr::UNSPECIFIED, 0))?;
    socket.set_read_timeout(Some(TIMEOUT))?;
    let clock = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.subsec_nanos())
        .unwrap_or(0);
    let mut last = io::Error::new(io::ErrorKind::TimedOut, "DNS: no reply");
    for attempt in 0..ATTEMPTS {
        let id = (clock >> 4) as u16 ^ (attempt as u16) << 12;
        socket.send_to(&encode_query(id, name)?, (server, 53))?;
        let mut reply = [0; 512];
        match socket.recv_from(&mut reply) {
            Ok((len, _)) => match parse_reply(id, &reply[..len]) {
                Ok(address) => return Ok(address),
                Err(err) if err.kind() == io::ErrorKind::NotFound => return Err(err),
                Err(err) => last = err,
            },
            Err(err) => last = err,
        }
    }
    Err(last)
}

/// The address to connect to for `host`.
#[cfg(target_os = "redox")]
pub fn resolve(host: &str) -> io::Result<IpAddr> {
    if let Ok(address) = host.parse() {
        return Ok(address);
    }
    let server = std::fs::read_to_string("/scheme/netcfg/resolv/nameserver")?;
    let server: IpAddr = server
        .lines()
        .next()
        .and_then(|line| line.trim().parse().ok())
        .ok_or_else(|| io::Error::other("DNS: no name server configured yet"))?;
    query(server, host).map(IpAddr::V4)
}

/// The address to connect to for `host`.
#[cfg(not(target_os = "redox"))]
pub fn resolve(host: &str) -> io::Result<IpAddr> {
    use std::net::ToSocketAddrs;
    if let Ok(address) = host.parse() {
        return Ok(address);
    }
    let addresses: Vec<_> = (host, 0).to_socket_addrs()?.map(|a| a.ip()).collect();
    addresses
        .iter()
        .find(|a| a.is_ipv4())
        .or(addresses.first())
        .copied()
        .ok_or_else(|| io::Error::new(io::ErrorKind::NotFound, "DNS: no address for the name"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn query_layout() {
        let packet = encode_query(0x1234, "a.example").unwrap();
        assert_eq!(&packet[..2], &[0x12, 0x34]);
        assert_eq!(&packet[12..], b"\x01a\x07example\x00\x00\x01\x00\x01");
        assert!(encode_query(1, "a..b").is_err());
    }

    #[test]
    fn reply_with_cname_then_a() {
        let mut reply = encode_query(7, "a.example").unwrap();
        reply[2] = 0x81;
        reply[3] = 0x80;
        reply[7] = 2;
        // CNAME to a compressed name, then the A record.
        reply.extend_from_slice(&[0xc0, 12, 0, 5, 0, 1, 0, 0, 0, 60, 0, 2, 0xc0, 12]);
        reply.extend_from_slice(&[0xc0, 12, 0, 1, 0, 1, 0, 0, 0, 60, 0, 4, 10, 1, 1, 3]);
        assert_eq!(parse_reply(7, &reply).unwrap(), Ipv4Addr::new(10, 1, 1, 3));
        assert!(parse_reply(8, &reply).is_err());
    }

    #[test]
    fn name_error_is_not_found() {
        let mut reply = encode_query(7, "missing.example").unwrap();
        reply[2] = 0x81;
        reply[3] = 0x83;
        assert_eq!(parse_reply(7, &reply).unwrap_err().kind(), io::ErrorKind::NotFound);
    }
}
