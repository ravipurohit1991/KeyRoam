//! Low-level cryptographic building blocks.
//!
//! * AEAD: XChaCha20-Poly1305 (192-bit random nonces, safe to generate randomly).
//! * Sub-key derivation: HKDF-SHA256 from the 256-bit vault key.
//! * MAC: HMAC-SHA256.
//!
//! All primitives come from the RustCrypto project; nothing here is hand-rolled
//! beyond composing them.

use base64::engine::general_purpose::STANDARD as B64;
use base64::Engine;
use chacha20poly1305::aead::{Aead, KeyInit, Payload};
use chacha20poly1305::{XChaCha20Poly1305, XNonce};
use hkdf::Hkdf;
use hmac::{Hmac, Mac};
use rand::rngs::OsRng;
use rand::RngCore;
use serde::{Deserialize, Serialize};
use sha2::Sha256;
use zeroize::{Zeroize, Zeroizing};

use crate::error::{Error, Result};

pub const KEY_LEN: usize = 32;
pub const NONCE_LEN: usize = 24;

/// A 256-bit symmetric key, wiped from memory on drop.
#[derive(Clone)]
pub struct SecretKey(Zeroizing<[u8; KEY_LEN]>);

impl SecretKey {
    pub fn generate() -> Self {
        let mut k = Zeroizing::new([0u8; KEY_LEN]);
        OsRng.fill_bytes(k.as_mut());
        SecretKey(k)
    }

    pub fn from_bytes(bytes: &[u8]) -> Result<Self> {
        if bytes.len() != KEY_LEN {
            return Err(Error::Format("key has wrong length".into()));
        }
        let mut k = Zeroizing::new([0u8; KEY_LEN]);
        k.copy_from_slice(bytes);
        Ok(SecretKey(k))
    }

    pub fn as_bytes(&self) -> &[u8; KEY_LEN] {
        &self.0
    }

    /// Derive an independent sub-key for a specific purpose (HKDF-SHA256).
    pub fn derive(&self, info: &[u8]) -> SecretKey {
        let hk = Hkdf::<Sha256>::new(None, self.0.as_ref());
        let mut out = Zeroizing::new([0u8; KEY_LEN]);
        hk.expand(info, out.as_mut())
            .expect("32 bytes is a valid HKDF-SHA256 output length");
        SecretKey(out)
    }
}

impl std::fmt::Debug for SecretKey {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("SecretKey(<redacted>)")
    }
}

/// Bytes serialised as standard base64 in JSON.
#[derive(Clone, PartialEq, Eq, Default)]
pub struct B64Bytes(pub Vec<u8>);

impl std::fmt::Debug for B64Bytes {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "B64Bytes({} bytes)", self.0.len())
    }
}

impl Serialize for B64Bytes {
    fn serialize<S: serde::Serializer>(&self, s: S) -> std::result::Result<S::Ok, S::Error> {
        s.serialize_str(&B64.encode(&self.0))
    }
}

impl<'de> Deserialize<'de> for B64Bytes {
    fn deserialize<D: serde::Deserializer<'de>>(d: D) -> std::result::Result<Self, D::Error> {
        let s = String::deserialize(d)?;
        B64.decode(s.as_bytes()).map(B64Bytes).map_err(serde::de::Error::custom)
    }
}

/// An AEAD ciphertext together with its nonce.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Sealed {
    pub nonce: B64Bytes,
    pub ciphertext: B64Bytes,
}

pub fn random_bytes<const N: usize>() -> [u8; N] {
    let mut b = [0u8; N];
    OsRng.fill_bytes(&mut b);
    b
}

/// Encrypt `plaintext` under `key`, authenticating `aad` as associated data.
pub fn seal(key: &SecretKey, plaintext: &[u8], aad: &[u8]) -> Result<Sealed> {
    let cipher = XChaCha20Poly1305::new(key.as_bytes().into());
    let nonce = random_bytes::<NONCE_LEN>();
    let ciphertext = cipher
        .encrypt(XNonce::from_slice(&nonce), Payload { msg: plaintext, aad })
        .map_err(|_| Error::Format("encryption failed".into()))?;
    Ok(Sealed {
        nonce: B64Bytes(nonce.to_vec()),
        ciphertext: B64Bytes(ciphertext),
    })
}

/// Decrypt and authenticate. Fails with [`Error::Authentication`] on any mismatch.
pub fn open(key: &SecretKey, sealed: &Sealed, aad: &[u8]) -> Result<Zeroizing<Vec<u8>>> {
    if sealed.nonce.0.len() != NONCE_LEN {
        return Err(Error::Authentication);
    }
    let cipher = XChaCha20Poly1305::new(key.as_bytes().into());
    cipher
        .decrypt(
            XNonce::from_slice(&sealed.nonce.0),
            Payload {
                msg: &sealed.ciphertext.0,
                aad,
            },
        )
        .map(Zeroizing::new)
        .map_err(|_| Error::Authentication)
}

type HmacSha256 = Hmac<Sha256>;

pub fn hmac_sha256(key: &SecretKey, data: &[u8]) -> Vec<u8> {
    let mut mac = <HmacSha256 as Mac>::new_from_slice(key.as_bytes()).expect("HMAC accepts any key length");
    mac.update(data);
    mac.finalize().into_bytes().to_vec()
}

/// Constant-time HMAC verification.
pub fn hmac_sha256_verify(key: &SecretKey, data: &[u8], tag: &[u8]) -> Result<()> {
    let mut mac = <HmacSha256 as Mac>::new_from_slice(key.as_bytes()).expect("HMAC accepts any key length");
    mac.update(data);
    mac.verify_slice(tag).map_err(|_| Error::Authentication)
}

/// Builder for unambiguous associated-data / MAC input: a domain label followed
/// by length-prefixed fields.
pub struct Transcript(Vec<u8>);

impl Transcript {
    pub fn new(label: &str) -> Self {
        let mut t = Transcript(Vec::with_capacity(128));
        t.bytes(label.as_bytes());
        t
    }

    pub fn bytes(&mut self, b: &[u8]) -> &mut Self {
        self.0.extend_from_slice(&(b.len() as u64).to_be_bytes());
        self.0.extend_from_slice(b);
        self
    }

    pub fn str(&mut self, s: &str) -> &mut Self {
        self.bytes(s.as_bytes())
    }

    pub fn u64(&mut self, v: u64) -> &mut Self {
        self.bytes(&v.to_be_bytes())
    }

    pub fn finish(&self) -> Vec<u8> {
        self.0.clone()
    }
}

/// Pad plaintext into 1 KiB buckets so ciphertext length reveals only a coarse
/// size class. Layout: u32 big-endian length || data || zero padding.
const PAD_BUCKET: usize = 1024;

pub fn pad(data: &[u8]) -> Zeroizing<Vec<u8>> {
    let total = 4 + data.len();
    let padded_len = total.div_ceil(PAD_BUCKET) * PAD_BUCKET;
    let mut out = Zeroizing::new(Vec::with_capacity(padded_len));
    out.extend_from_slice(&(data.len() as u32).to_be_bytes());
    out.extend_from_slice(data);
    out.resize(padded_len, 0);
    out
}

pub fn unpad(padded: &[u8]) -> Result<&[u8]> {
    if padded.len() < 4 {
        return Err(Error::Format("padded payload too short".into()));
    }
    let len = u32::from_be_bytes(padded[..4].try_into().unwrap()) as usize;
    padded
        .get(4..4 + len)
        .ok_or_else(|| Error::Format("padded payload length out of range".into()))
}

/// Wipe a `String` in place (used for transient secret copies).
pub fn wipe(mut s: String) {
    s.zeroize();
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn seal_open_roundtrip() {
        let key = SecretKey::generate();
        let sealed = seal(&key, b"synthetic secret", b"ctx").unwrap();
        assert_eq!(&open(&key, &sealed, b"ctx").unwrap()[..], b"synthetic secret");
    }

    #[test]
    fn nonces_are_unique() {
        let key = SecretKey::generate();
        let a = seal(&key, b"x", b"").unwrap();
        let b = seal(&key, b"x", b"").unwrap();
        assert_ne!(a.nonce, b.nonce);
        assert_ne!(a.ciphertext, b.ciphertext);
    }

    #[test]
    fn rejects_wrong_key_aad_and_tampering() {
        let key = SecretKey::generate();
        let sealed = seal(&key, b"synthetic secret", b"ctx").unwrap();

        assert!(matches!(
            open(&SecretKey::generate(), &sealed, b"ctx"),
            Err(Error::Authentication)
        ));
        assert!(matches!(open(&key, &sealed, b"other"), Err(Error::Authentication)));

        let mut ct = sealed.clone();
        ct.ciphertext.0[0] ^= 1;
        assert!(matches!(open(&key, &ct, b"ctx"), Err(Error::Authentication)));

        let mut n = sealed.clone();
        n.nonce.0[3] ^= 0x80;
        assert!(matches!(open(&key, &n, b"ctx"), Err(Error::Authentication)));

        let mut short = sealed;
        short.nonce.0.truncate(12);
        assert!(matches!(open(&key, &short, b"ctx"), Err(Error::Authentication)));
    }

    #[test]
    fn subkeys_are_independent_and_deterministic() {
        let key = SecretKey::generate();
        let a1 = key.derive(b"a");
        let a2 = key.derive(b"a");
        let b = key.derive(b"b");
        assert_eq!(a1.as_bytes(), a2.as_bytes());
        assert_ne!(a1.as_bytes(), b.as_bytes());
        assert_ne!(a1.as_bytes(), key.as_bytes());
    }

    #[test]
    fn hmac_verify() {
        let key = SecretKey::generate();
        let tag = hmac_sha256(&key, b"data");
        hmac_sha256_verify(&key, b"data", &tag).unwrap();
        assert!(hmac_sha256_verify(&key, b"datA", &tag).is_err());
        assert!(hmac_sha256_verify(&SecretKey::generate(), b"data", &tag).is_err());
    }

    #[test]
    fn transcript_is_unambiguous() {
        let a = Transcript::new("l").str("ab").str("c").finish();
        let b = Transcript::new("l").str("a").str("bc").finish();
        assert_ne!(a, b);
    }

    #[test]
    fn padding_roundtrip_and_buckets() {
        for len in [0usize, 1, 1019, 1020, 1021, 5000] {
            let data = vec![7u8; len];
            let padded = pad(&data);
            assert_eq!(padded.len() % PAD_BUCKET, 0);
            assert_eq!(unpad(&padded).unwrap(), &data[..]);
        }
        assert!(unpad(&[0, 0, 0, 9, 1]).is_err());
    }
}
