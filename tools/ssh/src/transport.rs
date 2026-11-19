//! Binary packet protocol (RFC 4253 section 6) with the
//! chacha20-poly1305@openssh.com cipher (PROTOCOL.chacha20poly1305).
//!
//! Before the keys are set, packets are cleartext with no MAC. After
//! NEWKEYS each packet is: encrypted 4-byte length, encrypted payload, and a
//! 16-byte Poly1305 tag over both encrypted parts. The length is encrypted
//! with the header key K1 (ChaCha20 block 0); the payload with the main key
//! K2 (ChaCha20 from block 1). The nonce is the 64-bit packet sequence
//! number. The Poly1305 key is the first 32 bytes of K2's block 0.

use std::io::{Read, Write};

use chacha20::cipher::{KeyIvInit, StreamCipher, StreamCipherSeek};
use chacha20::ChaCha20Legacy;
use poly1305::universal_hash::KeyInit;
use poly1305::Poly1305;

use crate::wire::{invalid, Result};

/// Largest packet payload accepted (OpenSSH uses 256 KiB).
const MAX_PACKET: usize = 256 * 1024;
const BLOCK: usize = 8;
const TAG: usize = 16;

/// Keys of one direction: K2 (payload, Poly1305) and K1 (length).
#[derive(Clone)]
pub struct Keys {
    pub main: [u8; 32],
    pub header: [u8; 32],
}

impl Keys {
    /// From the 64-byte key material `key = K2 || K1`.
    pub fn from_material(material: &[u8]) -> Self {
        let mut main = [0u8; 32];
        let mut header = [0u8; 32];
        main.copy_from_slice(&material[..32]);
        header.copy_from_slice(&material[32..64]);
        Self { main, header }
    }
}

/// XOR `data` with ChaCha20 keystream from `key`, nonce `seq`, starting at
/// `block` (64-byte blocks).
fn xor_stream(key: &[u8; 32], seq: u64, block: u64, data: &mut [u8]) {
    let mut cipher = ChaCha20Legacy::new(key.into(), &seq.to_be_bytes().into());
    cipher.seek(block * 64);
    cipher.apply_keystream(data);
}

pub struct Transport<S: Read + Write> {
    pub stream: S,
    out: Option<Keys>,
    inc: Option<Keys>,
    seq_out: u64,
    seq_in: u64,
}

impl<S: Read + Write> Transport<S> {
    pub fn new(stream: S) -> Self {
        Self { stream, out: None, inc: None, seq_out: 0, seq_in: 0 }
    }

    /// Start encrypting outgoing packets with `keys` (after our NEWKEYS).
    pub fn set_out_keys(&mut self, keys: Keys) {
        self.out = Some(keys);
    }

    /// Start decrypting incoming packets with `keys` (after theirs).
    pub fn set_in_keys(&mut self, keys: Keys) {
        self.inc = Some(keys);
    }

    pub fn send(&mut self, payload: &[u8]) -> Result<()> {
        let seq = self.seq_out;
        self.seq_out += 1;
        // Padding: 4 to 255 bytes. Cleartext packets count the 4-byte length
        // in the block size (RFC 4253). The AEAD cipher encrypts the length
        // on its own, so there the padding block covers only the padding
        // length byte, payload and padding (PROTOCOL.chacha20poly1305).
        let counted = if self.out.is_some() { 1 } else { 5 };
        let mut pad = BLOCK - ((counted + payload.len()) % BLOCK);
        if pad < 4 {
            pad += BLOCK;
        }
        let mut plain = Vec::with_capacity(1 + payload.len() + pad);
        plain.push(pad as u8);
        plain.extend_from_slice(payload);
        let mut random = vec![0u8; pad];
        rand_core::RngCore::fill_bytes(&mut rand_core::OsRng, &mut random);
        plain.extend_from_slice(&random);

        match self.out.clone() {
            None => {
                let packet_len = plain.len() as u32;
                let mut frame = Vec::with_capacity(4 + plain.len());
                frame.extend_from_slice(&packet_len.to_be_bytes());
                frame.extend_from_slice(&plain);
                self.stream.write_all(&frame)?;
            }
            Some(keys) => {
                let mut len = (plain.len() as u32).to_be_bytes();
                xor_stream(&keys.header, seq, 0, &mut len);
                xor_stream(&keys.main, seq, 1, &mut plain);
                let mut frame = Vec::with_capacity(4 + plain.len() + TAG);
                frame.extend_from_slice(&len);
                frame.extend_from_slice(&plain);
                let tag = mac(&keys.main, seq, &frame);
                frame.extend_from_slice(&tag);
                self.stream.write_all(&frame)?;
            }
        }
        self.stream.flush()
    }

    pub fn recv(&mut self) -> Result<Vec<u8>> {
        let seq = self.seq_in;
        self.seq_in += 1;
        match self.inc.clone() {
            None => {
                let mut len = [0u8; 4];
                self.stream.read_exact(&mut len)?;
                let packet_len = u32::from_be_bytes(len) as usize;
                if packet_len < 2 || packet_len > MAX_PACKET {
                    return Err(invalid("bad packet length"));
                }
                let mut plain = vec![0u8; packet_len];
                self.stream.read_exact(&mut plain)?;
                unframe(&plain)
            }
            Some(keys) => {
                let mut enc_len = [0u8; 4];
                self.stream.read_exact(&mut enc_len)?;
                let mut dec_len = enc_len;
                xor_stream(&keys.header, seq, 0, &mut dec_len);
                let packet_len = u32::from_be_bytes(dec_len) as usize;
                if packet_len < 2 || packet_len > MAX_PACKET {
                    return Err(invalid("bad packet length"));
                }
                let mut body = vec![0u8; packet_len + TAG];
                self.stream.read_exact(&mut body)?;
                // The MAC covers the length as sent (encrypted).
                let mut signed = Vec::with_capacity(4 + packet_len);
                signed.extend_from_slice(&enc_len);
                signed.extend_from_slice(&body[..packet_len]);
                let want = mac(&keys.main, seq, &signed);
                if !ct_eq(&want, &body[packet_len..]) {
                    return Err(invalid("packet authentication failed"));
                }
                let mut plain = body[..packet_len].to_vec();
                xor_stream(&keys.main, seq, 1, &mut plain);
                unframe(&plain)
            }
        }
    }
}

/// Poly1305 over `data`, keyed by the first 32 bytes of block 0 of K2.
fn mac(main: &[u8; 32], seq: u64, data: &[u8]) -> [u8; TAG] {
    let mut poly_key = [0u8; 32];
    xor_stream(main, seq, 0, &mut poly_key);
    let tag = Poly1305::new(&poly_key.into()).compute_unpadded(data);
    let mut out = [0u8; TAG];
    out.copy_from_slice(&tag);
    out
}

/// Remove the padding of `plain` (padding length byte, payload, padding).
fn unframe(plain: &[u8]) -> Result<Vec<u8>> {
    let pad = plain[0] as usize;
    if pad < 4 || 1 + pad > plain.len() {
        return Err(invalid("bad padding length"));
    }
    Ok(plain[1..plain.len() - pad].to_vec())
}

fn ct_eq(a: &[u8], b: &[u8]) -> bool {
    a.len() == b.len() && a.iter().zip(b).fold(0u8, |acc, (x, y)| acc | (x ^ y)) == 0
}
