//! TLS 1.3 client stream over a TCP connection, with `embedded-tls`.
//!
//! One cipher suite (TLS_AES_128_GCM_SHA256) and one key exchange group
//! (P-256) are offered. Certificates are checked by `trust`.

use std::fs::File;
use std::io::{self, Read, Write};
use std::mem::ManuallyDrop;
use std::net::TcpStream;
use std::time::{SystemTime, UNIX_EPOCH};

use embedded_tls::blocking::{
    Aes128GcmSha256, CertificateEntryRef, CertificateRef, CertificateVerifyRef, CryptoProvider,
    TlsConfig, TlsConnection, TlsContext, TlsVerifier,
};
use embedded_tls::{SignatureScheme, TlsError};
use rand_core::{CryptoRng, RngCore};
use sha2::{Digest, Sha256};

use crate::trust::{HandshakeScheme, PublicKey, TrustError, TrustStore};

/// Largest TLS record on the wire.
const READ_BUFFER: usize = 16640;
const WRITE_BUFFER: usize = 4096;
/// A clock reading before this (2026-01-01) means the machine has no set
/// clock, and certificate dates are not checked.
const CLOCK_FLOOR: u64 = 1_767_225_600;

#[cfg(target_os = "redox")]
const RANDOM: &str = "/scheme/rand";
#[cfg(not(target_os = "redox"))]
const RANDOM: &str = "/dev/urandom";

/// Seconds since 1970, or `None` when the clock is not set.
pub fn clock() -> Option<u64> {
    let now = SystemTime::now().duration_since(UNIX_EPOCH).ok()?.as_secs();
    (now >= CLOCK_FLOOR).then_some(now)
}

struct Socket(TcpStream);

impl embedded_io::ErrorType for Socket {
    type Error = io::Error;
}

impl embedded_io::Read for Socket {
    fn read(&mut self, buf: &mut [u8]) -> io::Result<usize> {
        self.0.read(buf)
    }
}

impl embedded_io::Write for Socket {
    fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
        self.0.write(buf)
    }

    fn flush(&mut self) -> io::Result<()> {
        self.0.flush()
    }
}

struct SystemRng(File);

impl RngCore for SystemRng {
    fn next_u32(&mut self) -> u32 {
        let mut bytes = [0; 4];
        self.fill_bytes(&mut bytes);
        u32::from_ne_bytes(bytes)
    }

    fn next_u64(&mut self) -> u64 {
        let mut bytes = [0; 8];
        self.fill_bytes(&mut bytes);
        u64::from_ne_bytes(bytes)
    }

    fn fill_bytes(&mut self, dest: &mut [u8]) {
        self.0.read_exact(dest).expect("the system random source reads");
    }

    fn try_fill_bytes(&mut self, dest: &mut [u8]) -> Result<(), rand_core::Error> {
        self.fill_bytes(dest);
        Ok(())
    }
}

impl CryptoRng for SystemRng {}

struct Verifier<'a> {
    trust: &'a TrustStore,
    host: String,
    /// Handshake transcript up to the server's certificate.
    transcript: Option<Sha256>,
    key: Option<PublicKey>,
    /// Why the last check failed; `TlsError` cannot carry it.
    failure: Option<TrustError>,
}

impl TlsVerifier<Aes128GcmSha256> for Verifier<'_> {
    fn set_hostname_verification(&mut self, hostname: &str) -> Result<(), TlsError> {
        self.host = hostname.to_string();
        Ok(())
    }

    fn verify_certificate(&mut self, transcript: &Sha256, cert: CertificateRef) -> Result<(), TlsError> {
        let chain: Vec<&[u8]> = cert
            .entries
            .iter()
            .filter_map(|entry| match entry {
                CertificateEntryRef::X509(der) => Some(*der),
                CertificateEntryRef::RawPublicKey(_) => None,
            })
            .collect();
        match self.trust.verify_chain(&chain, &self.host, clock()) {
            Ok(key) => {
                self.key = Some(key);
                self.transcript = Some(transcript.clone());
                Ok(())
            }
            Err(err) => {
                self.failure = Some(err);
                Err(TlsError::InvalidCertificate)
            }
        }
    }

    fn verify_signature(&mut self, verify: CertificateVerifyRef) -> Result<(), TlsError> {
        let (Some(key), Some(transcript)) = (self.key.as_ref(), self.transcript.take()) else {
            return Err(TlsError::InvalidHandshake);
        };
        let scheme = match verify.signature_scheme {
            SignatureScheme::EcdsaSecp256r1Sha256 => HandshakeScheme::EcdsaP256Sha256,
            SignatureScheme::EcdsaSecp384r1Sha384 => HandshakeScheme::EcdsaP384Sha384,
            SignatureScheme::RsaPssRsaeSha256 => HandshakeScheme::RsaPssSha256,
            SignatureScheme::RsaPssRsaeSha384 => HandshakeScheme::RsaPssSha384,
            SignatureScheme::RsaPssRsaeSha512 => HandshakeScheme::RsaPssSha512,
            _ => {
                self.failure = Some(TrustError::Unsupported);
                return Err(TlsError::InvalidSignatureScheme);
            }
        };
        // RFC 8446 section 4.4.3: 64 spaces, the context string, a zero
        // byte, then the transcript hash.
        let mut message = vec![0x20; 64];
        message.extend_from_slice(b"TLS 1.3, server CertificateVerify\0");
        message.extend_from_slice(&transcript.finalize());
        key.verify_handshake(scheme, &message, verify.signature).map_err(|err| {
            self.failure = Some(err);
            TlsError::InvalidSignature
        })
    }
}

struct Provider<'a> {
    rng: SystemRng,
    verifier: Verifier<'a>,
}

impl CryptoProvider for Provider<'_> {
    type CipherSuite = Aes128GcmSha256;
    type Signature = [u8; 0];

    fn rng(&mut self) -> impl embedded_tls::CryptoRngCore {
        &mut self.rng
    }

    fn verifier(&mut self) -> Result<&mut impl TlsVerifier<Aes128GcmSha256>, TlsError> {
        Ok(&mut self.verifier)
    }
}

fn tls_error(err: TlsError) -> io::Error {
    match err {
        TlsError::Io(kind) => io::Error::from(io::ErrorKind::from(kind)),
        other => io::Error::other(format!("TLS: {other:?}")),
    }
}

pub struct TlsStream {
    /// Borrows the two record buffers below; dropped before they are freed.
    conn: ManuallyDrop<TlsConnection<'static, Socket, Aes128GcmSha256>>,
    read_buffer: *mut [u8],
    write_buffer: *mut [u8],
}

// SAFETY: the stream owns the two buffers behind the raw pointers; nothing
// else refers to them, so the whole stream can move to another thread.
unsafe impl Send for TlsStream {}

impl TlsStream {
    /// Run the handshake on `tcp` and check the server's certificate for
    /// `host` against `trust`.
    pub fn connect(tcp: TcpStream, host: &str, trust: &TrustStore) -> io::Result<Self> {
        let rng = SystemRng(File::open(RANDOM)?);
        let read_buffer = Box::into_raw(vec![0u8; READ_BUFFER].into_boxed_slice());
        let write_buffer = Box::into_raw(vec![0u8; WRITE_BUFFER].into_boxed_slice());
        // SAFETY: both pointers come from `Box::into_raw` just above, are
        // used by nothing else, and are freed in `drop` after the connection
        // that borrows them.
        let conn = unsafe { TlsConnection::new(Socket(tcp), &mut *read_buffer, &mut *write_buffer) };
        let mut stream = TlsStream {
            conn: ManuallyDrop::new(conn),
            read_buffer,
            write_buffer,
        };
        let config = TlsConfig::new().with_server_name(host);
        let mut provider = Provider {
            rng,
            verifier: Verifier {
                trust,
                host: host.to_string(),
                transcript: None,
                key: None,
                failure: None,
            },
        };
        match stream.conn.open(TlsContext::new(&config, &mut provider)) {
            Ok(()) => Ok(stream),
            Err(err) => Err(match provider.verifier.failure {
                Some(why) => io::Error::other(format!("TLS: certificate for {host}: {why:?}")),
                None => tls_error(err),
            }),
        }
    }
}

impl Drop for TlsStream {
    fn drop(&mut self) {
        // SAFETY: the connection is dropped once, here, before the buffers
        // it borrows; the buffers were boxed in `connect`.
        unsafe {
            ManuallyDrop::drop(&mut self.conn);
            drop(Box::from_raw(self.read_buffer));
            drop(Box::from_raw(self.write_buffer));
        }
    }
}

impl Read for TlsStream {
    fn read(&mut self, buf: &mut [u8]) -> io::Result<usize> {
        match self.conn.read(buf) {
            Ok(n) => Ok(n),
            Err(TlsError::ConnectionClosed) => Ok(0),
            Err(err) => Err(tls_error(err)),
        }
    }
}

impl Write for TlsStream {
    fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
        self.conn.write(buf).map_err(tls_error)
    }

    fn flush(&mut self) -> io::Result<()> {
        self.conn.flush().map_err(tls_error)
    }
}
