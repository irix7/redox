//! Key exchange (RFC 4253 section 7): curve25519-sha256, ssh-ed25519 host
//! keys, and the chacha20-poly1305@openssh.com cipher.

use std::io::{Read, Write};

use ed25519_dalek::{Signature, VerifyingKey};
use rand_core::OsRng;
use sha2::{Digest, Sha256};
use x25519_dalek::{EphemeralSecret, PublicKey};

use crate::transport::{Keys, Transport};
use crate::wire::{invalid, Reader, Result, Writer};

pub const MSG_KEXINIT: u8 = 20;
pub const MSG_NEWKEYS: u8 = 21;
pub const MSG_KEX_ECDH_INIT: u8 = 30;
pub const MSG_KEX_ECDH_REPLY: u8 = 31;

const KEX: &str = "curve25519-sha256";
const HOST_KEY: &str = "ssh-ed25519";
const CIPHER: &str = "chacha20-poly1305@openssh.com";

/// Our KEXINIT payload (message number included). `cookie` is 16 random bytes.
pub fn kexinit_payload(cookie: &[u8; 16]) -> Vec<u8> {
    let mut w = Writer::new();
    w.u8(MSG_KEXINIT).bytes(cookie);
    w.str("curve25519-sha256,curve25519-sha256@libssh.org");
    w.str(HOST_KEY);
    w.str(CIPHER).str(CIPHER);
    w.str("hmac-sha2-256").str("hmac-sha2-256");
    w.str("none").str("none");
    w.str("").str("");
    w.bool(false).u32(0);
    w.buf
}

/// Check that the server offers what we use. The name lists are the first
/// ten fields after the cookie.
pub fn check_server_kexinit(payload: &[u8]) -> Result<()> {
    let mut r = Reader::new(payload);
    if r.u8()? != MSG_KEXINIT {
        return Err(invalid("expected KEXINIT"));
    }
    r.skip(16)?;
    let mut lists = Vec::new();
    for _ in 0..10 {
        lists.push(r.str()?.to_string());
    }
    let has = |list: &str, want: &str| list.split(',').any(|n| n == want);
    if !has(&lists[0], KEX) {
        return Err(invalid("server does not offer curve25519-sha256"));
    }
    if !has(&lists[1], HOST_KEY) {
        return Err(invalid("server does not offer ssh-ed25519"));
    }
    if !has(&lists[2], CIPHER) || !has(&lists[3], CIPHER) {
        return Err(invalid("server does not offer chacha20-poly1305@openssh.com"));
    }
    if !has(&lists[6], "none") || !has(&lists[7], "none") {
        return Err(invalid("server requires compression"));
    }
    Ok(())
}

/// Result of a key exchange: the session id and the host key blob.
pub struct Exchanged {
    pub session_id: [u8; 32],
    pub host_key: Vec<u8>,
}

/// Run the exchange. `v_c` and `v_s` are the version strings without CRLF.
pub fn exchange<S: Read + Write>(
    t: &mut Transport<S>,
    v_c: &str,
    v_s: &str,
    i_c: &[u8],
    i_s: &[u8],
) -> Result<Exchanged> {
    let secret = EphemeralSecret::random_from_rng(OsRng);
    let q_c = PublicKey::from(&secret);

    let mut init = Writer::new();
    init.u8(MSG_KEX_ECDH_INIT).string(q_c.as_bytes());
    t.send(&init.buf)?;

    let reply = t.recv()?;
    let mut r = Reader::new(&reply);
    if r.u8()? != MSG_KEX_ECDH_REPLY {
        return Err(invalid("expected KEX_ECDH_REPLY"));
    }
    let host_key = r.string()?.to_vec();
    let q_s = r.string()?.to_vec();
    let sig_blob = r.string()?.to_vec();
    let q_s: [u8; 32] = q_s.as_slice().try_into().map_err(|_| invalid("bad curve25519 key"))?;

    let shared = secret.diffie_hellman(&PublicKey::from(q_s));
    if shared.as_bytes().iter().all(|&b| b == 0) {
        return Err(invalid("curve25519 shared secret is zero"));
    }
    let k = shared.as_bytes().to_vec();

    let mut h = Sha256::new();
    for part in [v_c.as_bytes(), v_s.as_bytes(), i_c, i_s, &host_key, q_c.as_bytes(), &q_s] {
        h.update((part.len() as u32).to_be_bytes());
        h.update(part);
    }
    let mut kmp = Writer::new();
    kmp.mpint(&k);
    h.update(&kmp.buf);
    let hash: [u8; 32] = h.finalize().into();

    verify_host_signature(&host_key, &sig_blob, &hash)?;

    let session_id = hash;
    let mut newkeys = Writer::new();
    newkeys.u8(MSG_NEWKEYS);
    t.send(&newkeys.buf)?;
    let msg = t.recv()?;
    if msg.first() != Some(&MSG_NEWKEYS) {
        return Err(invalid("expected NEWKEYS"));
    }

    // Keys: C (client to server) and D (server to client), 64 bytes each.
    let c2s = Keys::from_material(&derive(&kmp.buf, &hash, b'C', &session_id));
    let s2c = Keys::from_material(&derive(&kmp.buf, &hash, b'D', &session_id));
    t.set_out_keys(c2s);
    t.set_in_keys(s2c);
    Ok(Exchanged { session_id, host_key })
}

/// `K1 = HASH(K || H || X || session_id)`, then `Kn = HASH(K || H || K1..Kn-1)`
/// until 64 bytes are made.
fn derive(k_mpint: &[u8], h: &[u8; 32], letter: u8, session_id: &[u8; 32]) -> [u8; 64] {
    let mut out = Vec::with_capacity(64);
    let mut first = Sha256::new();
    first.update(k_mpint);
    first.update(h);
    first.update([letter]);
    first.update(session_id);
    out.extend_from_slice(&first.finalize());
    while out.len() < 64 {
        let mut next = Sha256::new();
        next.update(k_mpint);
        next.update(h);
        next.update(&out);
        out.extend_from_slice(&next.finalize());
    }
    let mut key = [0u8; 64];
    key.copy_from_slice(&out[..64]);
    key
}

/// The host key blob is `string "ssh-ed25519", string key[32]`; the
/// signature blob is `string "ssh-ed25519", string sig[64]`.
fn verify_host_signature(host_key: &[u8], sig_blob: &[u8], hash: &[u8; 32]) -> Result<()> {
    let mut r = Reader::new(host_key);
    if r.str()? != HOST_KEY {
        return Err(invalid("host key is not ssh-ed25519"));
    }
    let pk: [u8; 32] = r.string()?.try_into().map_err(|_| invalid("bad host key length"))?;
    let mut r = Reader::new(sig_blob);
    if r.str()? != HOST_KEY {
        return Err(invalid("signature is not ssh-ed25519"));
    }
    let sig: [u8; 64] = r.string()?.try_into().map_err(|_| invalid("bad signature length"))?;
    let key = VerifyingKey::from_bytes(&pk).map_err(|_| invalid("invalid host key"))?;
    key.verify_strict(hash, &Signature::from_bytes(&sig))
        .map_err(|_| invalid("host signature does not verify"))
}
