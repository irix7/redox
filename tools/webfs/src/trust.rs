//! Certificate checks for the TLS client: trust anchors, the chain from the
//! server's certificate to one of them, the host name, validity dates and the
//! handshake signature.
//!
//! Supported keys and signatures are the ones public web certificates use:
//! ECDSA on P-256 and P-384 with SHA-256 or SHA-384, RSA PKCS#1 v1.5 with
//! SHA-256, SHA-384 or SHA-512 in certificates, and RSA-PSS in the handshake.
//! Revocation is not checked.

use std::net::IpAddr;

use p256::ecdsa::signature::Verifier;
use p256::ecdsa::signature::hazmat::PrehashVerifier;
use rsa::RsaPublicKey;
use rsa::pkcs1::DecodeRsaPublicKey;
use sha2::{Digest, Sha256, Sha384, Sha512};
use x509_cert::Certificate;
use x509_cert::der::asn1::ObjectIdentifier;
use x509_cert::der::{Decode, Encode};
use x509_cert::ext::pkix::name::GeneralName;
use x509_cert::ext::pkix::{BasicConstraints, SubjectAltName};
use x509_cert::spki::SubjectPublicKeyInfoOwned;

const EC_PUBLIC_KEY: ObjectIdentifier = ObjectIdentifier::new_unwrap("1.2.840.10045.2.1");
const CURVE_P256: ObjectIdentifier = ObjectIdentifier::new_unwrap("1.2.840.10045.3.1.7");
const CURVE_P384: ObjectIdentifier = ObjectIdentifier::new_unwrap("1.3.132.0.34");
const RSA_ENCRYPTION: ObjectIdentifier = ObjectIdentifier::new_unwrap("1.2.840.113549.1.1.1");
const ECDSA_SHA256: ObjectIdentifier = ObjectIdentifier::new_unwrap("1.2.840.10045.4.3.2");
const ECDSA_SHA384: ObjectIdentifier = ObjectIdentifier::new_unwrap("1.2.840.10045.4.3.3");
const RSA_SHA256: ObjectIdentifier = ObjectIdentifier::new_unwrap("1.2.840.113549.1.1.11");
const RSA_SHA384: ObjectIdentifier = ObjectIdentifier::new_unwrap("1.2.840.113549.1.1.12");
const RSA_SHA512: ObjectIdentifier = ObjectIdentifier::new_unwrap("1.2.840.113549.1.1.13");

/// Longest chain followed from the server's certificate to an anchor.
const MAX_DEPTH: usize = 8;

/// The roots compiled into the client: Let's Encrypt and Google Trust
/// Services, the two authorities Cloudflare's Universal SSL issues from.
const BUILT_IN: [&[u8]; 4] = [
    include_bytes!("../roots/isrg-root-x1.der"),
    include_bytes!("../roots/isrg-root-x2.der"),
    include_bytes!("../roots/gts-root-r1.der"),
    include_bytes!("../roots/gts-root-r4.der"),
];

#[derive(Debug, PartialEq, Eq)]
pub enum TrustError {
    /// A certificate or key could not be decoded.
    Malformed,
    /// A key type or signature algorithm outside the supported set.
    Unsupported,
    /// No chain from the server's certificate ends at a trust anchor.
    UnknownIssuer,
    /// The certificate does not name the host.
    WrongHost,
    /// A certificate in the chain is not valid at the current time.
    Expired,
    BadSignature,
}

pub enum PublicKey {
    P256(p256::ecdsa::VerifyingKey),
    P384(p384::ecdsa::VerifyingKey),
    Rsa(RsaPublicKey),
}

impl PublicKey {
    fn from_spki(spki: &SubjectPublicKeyInfoOwned) -> Result<Self, TrustError> {
        let bytes = spki.subject_public_key.as_bytes().ok_or(TrustError::Malformed)?;
        if spki.algorithm.oid == EC_PUBLIC_KEY {
            let curve = spki
                .algorithm
                .parameters
                .as_ref()
                .and_then(|any| any.decode_as::<ObjectIdentifier>().ok())
                .ok_or(TrustError::Malformed)?;
            if curve == CURVE_P256 {
                p256::ecdsa::VerifyingKey::from_sec1_bytes(bytes)
                    .map(PublicKey::P256)
                    .map_err(|_| TrustError::Malformed)
            } else if curve == CURVE_P384 {
                p384::ecdsa::VerifyingKey::from_sec1_bytes(bytes)
                    .map(PublicKey::P384)
                    .map_err(|_| TrustError::Malformed)
            } else {
                Err(TrustError::Unsupported)
            }
        } else if spki.algorithm.oid == RSA_ENCRYPTION {
            RsaPublicKey::from_pkcs1_der(bytes)
                .map(PublicKey::Rsa)
                .map_err(|_| TrustError::Malformed)
        } else {
            Err(TrustError::Unsupported)
        }
    }

    /// Check an ECDSA signature over an already hashed message.
    fn verify_ecdsa(&self, prehash: &[u8], signature: &[u8]) -> Result<(), TrustError> {
        let ok = match self {
            PublicKey::P256(key) => p256::ecdsa::Signature::from_der(signature)
                .map_err(|_| TrustError::Malformed)
                .map(|sig| key.verify_prehash(prehash, &sig).is_ok())?,
            PublicKey::P384(key) => p384::ecdsa::Signature::from_der(signature)
                .map_err(|_| TrustError::Malformed)
                .map(|sig| key.verify_prehash(prehash, &sig).is_ok())?,
            PublicKey::Rsa(_) => return Err(TrustError::Unsupported),
        };
        if ok { Ok(()) } else { Err(TrustError::BadSignature) }
    }

    /// Check the signature a certificate carries, made with this key.
    fn verify_certificate(
        &self,
        algorithm: ObjectIdentifier,
        tbs: &[u8],
        signature: &[u8],
    ) -> Result<(), TrustError> {
        if algorithm == ECDSA_SHA256 {
            return self.verify_ecdsa(&Sha256::digest(tbs), signature);
        }
        if algorithm == ECDSA_SHA384 {
            return self.verify_ecdsa(&Sha384::digest(tbs), signature);
        }
        let PublicKey::Rsa(key) = self else {
            return Err(TrustError::Unsupported);
        };
        let signature =
            rsa::pkcs1v15::Signature::try_from(signature).map_err(|_| TrustError::Malformed)?;
        let key = key.clone();
        let ok = if algorithm == RSA_SHA256 {
            rsa::pkcs1v15::VerifyingKey::<Sha256>::new(key).verify(tbs, &signature).is_ok()
        } else if algorithm == RSA_SHA384 {
            rsa::pkcs1v15::VerifyingKey::<Sha384>::new(key).verify(tbs, &signature).is_ok()
        } else if algorithm == RSA_SHA512 {
            rsa::pkcs1v15::VerifyingKey::<Sha512>::new(key).verify(tbs, &signature).is_ok()
        } else {
            return Err(TrustError::Unsupported);
        };
        if ok { Ok(()) } else { Err(TrustError::BadSignature) }
    }

    /// Check a TLS 1.3 CertificateVerify signature. `scheme` is the wire
    /// value of the signature scheme.
    pub fn verify_handshake(
        &self,
        scheme: HandshakeScheme,
        message: &[u8],
        signature: &[u8],
    ) -> Result<(), TrustError> {
        let ok = match (self, scheme) {
            (PublicKey::P256(_), HandshakeScheme::EcdsaP256Sha256) => {
                return self.verify_ecdsa(&Sha256::digest(message), signature);
            }
            (PublicKey::P384(_), HandshakeScheme::EcdsaP384Sha384) => {
                return self.verify_ecdsa(&Sha384::digest(message), signature);
            }
            (PublicKey::Rsa(key), scheme) => {
                let signature =
                    rsa::pss::Signature::try_from(signature).map_err(|_| TrustError::Malformed)?;
                let key = key.clone();
                match scheme {
                    HandshakeScheme::RsaPssSha256 => {
                        rsa::pss::VerifyingKey::<Sha256>::new(key).verify(message, &signature).is_ok()
                    }
                    HandshakeScheme::RsaPssSha384 => {
                        rsa::pss::VerifyingKey::<Sha384>::new(key).verify(message, &signature).is_ok()
                    }
                    HandshakeScheme::RsaPssSha512 => {
                        rsa::pss::VerifyingKey::<Sha512>::new(key).verify(message, &signature).is_ok()
                    }
                    _ => return Err(TrustError::Unsupported),
                }
            }
            _ => return Err(TrustError::Unsupported),
        };
        if ok { Ok(()) } else { Err(TrustError::BadSignature) }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum HandshakeScheme {
    EcdsaP256Sha256,
    EcdsaP384Sha384,
    RsaPssSha256,
    RsaPssSha384,
    RsaPssSha512,
}

struct Anchor {
    /// DER of the subject name.
    subject: Vec<u8>,
    key: PublicKey,
}

pub struct TrustStore {
    anchors: Vec<Anchor>,
}

/// The bytes a certificate's signature covers: the first element of the
/// outer SEQUENCE, as sent. Decoding and encoding again could change them.
fn tbs_bytes(der: &[u8]) -> Option<&[u8]> {
    fn header(data: &[u8]) -> Option<(usize, usize)> {
        let first = *data.get(1)?;
        if first < 0x80 {
            return Some((2, usize::from(first)));
        }
        let count = usize::from(first & 0x7f);
        if count == 0 || count > 4 {
            return None;
        }
        let mut len = 0usize;
        for byte in data.get(2..2 + count)? {
            len = len << 8 | usize::from(*byte);
        }
        Some((2 + count, len))
    }
    if *der.first()? != 0x30 {
        return None;
    }
    let (outer, _) = header(der)?;
    let inner = der.get(outer..)?;
    let (head, len) = header(inner)?;
    inner.get(..head.checked_add(len)?)
}

fn name_matches(pattern: &str, host: &str) -> bool {
    let pattern = pattern.trim_end_matches('.');
    if let Some(suffix) = pattern.strip_prefix("*.") {
        // The wildcard stands for exactly one label.
        return host
            .split_once('.')
            .is_some_and(|(label, rest)| !label.is_empty() && rest.eq_ignore_ascii_case(suffix));
    }
    pattern.eq_ignore_ascii_case(host)
}

fn names_host(cert: &Certificate, host: &str) -> bool {
    let Ok(Some((_, san))) = cert.tbs_certificate.get::<SubjectAltName>() else {
        return false;
    };
    let address: Option<IpAddr> = host.parse().ok();
    san.0.iter().any(|name| match (name, address) {
        (GeneralName::DnsName(dns), None) => name_matches(dns.as_str(), host),
        (GeneralName::IpAddress(octets), Some(IpAddr::V4(v4))) => octets.as_bytes() == v4.octets(),
        (GeneralName::IpAddress(octets), Some(IpAddr::V6(v6))) => octets.as_bytes() == v6.octets(),
        _ => false,
    })
}

fn valid_at(cert: &Certificate, now: u64) -> bool {
    let validity = &cert.tbs_certificate.validity;
    validity.not_before.to_unix_duration().as_secs() <= now
        && now <= validity.not_after.to_unix_duration().as_secs()
}

fn is_ca(cert: &Certificate) -> bool {
    matches!(
        cert.tbs_certificate.get::<BasicConstraints>(),
        Ok(Some((_, constraints))) if constraints.ca
    )
}

impl TrustStore {
    pub fn empty() -> Self {
        TrustStore { anchors: Vec::new() }
    }

    /// The compiled-in roots.
    pub fn built_in() -> Self {
        let mut store = Self::empty();
        for der in BUILT_IN {
            store.add_der(der).expect("a built-in root decodes");
        }
        store
    }

    #[cfg(test)]
    fn len(&self) -> usize {
        self.anchors.len()
    }

    pub fn add_der(&mut self, der: &[u8]) -> Result<(), TrustError> {
        let cert = Certificate::from_der(der).map_err(|_| TrustError::Malformed)?;
        self.add(&cert)
    }

    /// Add every certificate in a file: one DER certificate or any number of
    /// PEM blocks.
    pub fn add_file(&mut self, data: &[u8]) -> Result<usize, TrustError> {
        if data.windows(10).any(|w| w == b"-----BEGIN") {
            let certs = Certificate::load_pem_chain(data).map_err(|_| TrustError::Malformed)?;
            for cert in &certs {
                self.add(cert)?;
            }
            Ok(certs.len())
        } else {
            self.add_der(data).map(|()| 1)
        }
    }

    fn add(&mut self, cert: &Certificate) -> Result<(), TrustError> {
        let tbs = &cert.tbs_certificate;
        self.anchors.push(Anchor {
            subject: tbs.subject.to_der().map_err(|_| TrustError::Malformed)?,
            key: PublicKey::from_spki(&tbs.subject_public_key_info)?,
        });
        Ok(())
    }

    /// Check the chain a server sent, its own certificate first, and return
    /// the server's public key for the handshake signature.
    ///
    /// `now` is seconds since 1970, or `None` when the machine has no usable
    /// clock; validity dates are then not checked.
    pub fn verify_chain(
        &self,
        chain: &[&[u8]],
        host: &str,
        now: Option<u64>,
    ) -> Result<PublicKey, TrustError> {
        let mut certs = Vec::with_capacity(chain.len());
        for der in chain {
            let cert = Certificate::from_der(der).map_err(|_| TrustError::Malformed)?;
            certs.push((cert, tbs_bytes(der).ok_or(TrustError::Malformed)?));
        }
        let Some((leaf, _)) = certs.first() else {
            return Err(TrustError::Malformed);
        };
        if !names_host(leaf, host) {
            return Err(TrustError::WrongHost);
        }

        let mut current = 0;
        let mut used = vec![false; certs.len()];
        let mut unsupported = false;
        for _ in 0..MAX_DEPTH {
            used[current] = true;
            let (cert, tbs) = &certs[current];
            if now.is_some_and(|now| !valid_at(cert, now)) {
                return Err(TrustError::Expired);
            }
            let issuer = cert.tbs_certificate.issuer.to_der().map_err(|_| TrustError::Malformed)?;
            let algorithm = cert.signature_algorithm.oid;
            let signature = cert.signature.as_bytes().ok_or(TrustError::Malformed)?;
            let mut signed_by = |key: &PublicKey| match key.verify_certificate(algorithm, tbs, signature) {
                Ok(()) => true,
                Err(TrustError::Unsupported) => {
                    unsupported = true;
                    false
                }
                Err(_) => false,
            };

            if self.anchors.iter().any(|a| a.subject == issuer && signed_by(&a.key)) {
                return PublicKey::from_spki(&leaf.tbs_certificate.subject_public_key_info);
            }
            let next = (0..certs.len()).find(|&i| {
                let (candidate, _) = &certs[i];
                !used[i]
                    && is_ca(candidate)
                    && candidate.tbs_certificate.subject.to_der().is_ok_and(|s| s == issuer)
                    && PublicKey::from_spki(&candidate.tbs_certificate.subject_public_key_info)
                        .is_ok_and(|key| signed_by(&key))
            });
            match next {
                Some(i) => current = i,
                None => break,
            }
        }
        Err(if unsupported { TrustError::Unsupported } else { TrustError::UnknownIssuer })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn built_in_roots_decode() {
        assert_eq!(TrustStore::built_in().len(), 4);
    }

    #[test]
    fn wildcard_covers_one_label() {
        assert!(name_matches("*.example.com", "a.example.com"));
        assert!(name_matches("*.Example.com", "A.example.COM"));
        assert!(!name_matches("*.example.com", "example.com"));
        assert!(!name_matches("*.example.com", "a.b.example.com"));
        assert!(name_matches("example.com", "example.com"));
        assert!(!name_matches("example.com", "example.org"));
    }

    #[test]
    fn tbs_is_the_first_inner_element() {
        // SEQUENCE { SEQUENCE { INTEGER 1 }, NULL }
        let der = [0x30, 0x07, 0x30, 0x03, 0x02, 0x01, 0x01, 0x05, 0x00];
        assert_eq!(tbs_bytes(&der), Some(&der[2..7]));
        assert_eq!(tbs_bytes(&[0x30, 0x07, 0x30, 0x7f]), None);
    }

    #[test]
    fn a_root_is_its_own_chain_only_for_the_right_host() {
        // A self-signed root has no subjectAltName, so no host matches it.
        let store = TrustStore::built_in();
        let root: &[u8] = BUILT_IN[1];
        assert_eq!(
            store.verify_chain(&[root], "example.com", None).err(),
            Some(TrustError::WrongHost)
        );
    }
}
