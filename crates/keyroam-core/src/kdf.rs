//! Master-password key derivation (Argon2id).

use argon2::{Algorithm, Argon2, Params, Version};
use serde::{Deserialize, Serialize};
use unicode_normalization::UnicodeNormalization;
use zeroize::Zeroizing;

use crate::crypto::{random_bytes, B64Bytes, SecretKey, KEY_LEN};
use crate::error::{Error, Result};

pub const SALT_LEN: usize = 16;

/// Minimum parameters accepted when *reading* a vault. They follow the OWASP
/// Argon2id baseline (19 MiB, t=2, p=1) and prevent an attacker who can write
/// to cloud storage from downgrading the KDF.
pub const MIN_MEMORY_KIB: u32 = 19 * 1024;
pub const MIN_ITERATIONS: u32 = 2;
/// Upper bounds stop a malicious header from exhausting memory/CPU.
pub const MAX_MEMORY_KIB: u32 = 1024 * 1024;
pub const MAX_ITERATIONS: u32 = 20;
pub const MAX_PARALLELISM: u32 = 16;

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct KdfParams {
    pub algorithm: String,
    pub memory_kib: u32,
    pub iterations: u32,
    pub parallelism: u32,
    pub salt: B64Bytes,
}

/// Cost settings without a salt, used when creating new key material.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct KdfCost {
    pub memory_kib: u32,
    pub iterations: u32,
    pub parallelism: u32,
}

impl KdfCost {
    /// Default for new vaults: 64 MiB, 3 passes, 1 lane.
    pub const DEFAULT: KdfCost = KdfCost {
        memory_kib: 64 * 1024,
        iterations: 3,
        parallelism: 1,
    };

    /// The weakest parameters the reader accepts. Still production-grade; used
    /// by tests to keep the suite fast without weakening validation.
    pub const MINIMUM: KdfCost = KdfCost {
        memory_kib: MIN_MEMORY_KIB,
        iterations: MIN_ITERATIONS,
        parallelism: 1,
    };
}

impl KdfParams {
    pub fn new_random(cost: KdfCost) -> Result<Self> {
        let p = KdfParams {
            algorithm: "argon2id".into(),
            memory_kib: cost.memory_kib,
            iterations: cost.iterations,
            parallelism: cost.parallelism,
            salt: B64Bytes(random_bytes::<SALT_LEN>().to_vec()),
        };
        p.validate()?;
        Ok(p)
    }

    pub fn validate(&self) -> Result<()> {
        let bad = |m: &str| Err(Error::InvalidKdfParams(m.to_string()));
        if self.algorithm != "argon2id" {
            return bad("only argon2id is supported");
        }
        if self.memory_kib < MIN_MEMORY_KIB || self.memory_kib > MAX_MEMORY_KIB {
            return bad("memory cost out of accepted range");
        }
        if self.iterations < MIN_ITERATIONS || self.iterations > MAX_ITERATIONS {
            return bad("iteration count out of accepted range");
        }
        if self.parallelism < 1 || self.parallelism > MAX_PARALLELISM {
            return bad("parallelism out of accepted range");
        }
        if self.salt.0.len() < SALT_LEN {
            return bad("salt too short");
        }
        Ok(())
    }

    /// Derive the key-encryption key from a master password.
    ///
    /// The password is Unicode NFC-normalised first so the same password typed
    /// on different operating systems produces the same key.
    pub fn derive_key(&self, password: &str) -> Result<SecretKey> {
        self.validate()?;
        let normalized: Zeroizing<String> = Zeroizing::new(password.nfc().collect());
        let params = Params::new(self.memory_kib, self.iterations, self.parallelism, Some(KEY_LEN))
            .map_err(|e| Error::InvalidKdfParams(e.to_string()))?;
        let argon = Argon2::new(Algorithm::Argon2id, Version::V0x13, params);
        let mut out = Zeroizing::new([0u8; KEY_LEN]);
        argon
            .hash_password_into(normalized.as_bytes(), &self.salt.0, out.as_mut())
            .map_err(|e| Error::InvalidKdfParams(e.to_string()))?;
        SecretKey::from_bytes(out.as_ref())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn deterministic_and_salt_dependent() {
        let p = KdfParams::new_random(KdfCost::MINIMUM).unwrap();
        let k1 = p.derive_key("correct horse battery staple").unwrap();
        let k2 = p.derive_key("correct horse battery staple").unwrap();
        assert_eq!(k1.as_bytes(), k2.as_bytes());
        let k3 = p.derive_key("correct horse battery stapl3").unwrap();
        assert_ne!(k1.as_bytes(), k3.as_bytes());

        let other = KdfParams::new_random(KdfCost::MINIMUM).unwrap();
        assert_ne!(other.salt, p.salt);
        assert_ne!(
            other.derive_key("correct horse battery staple").unwrap().as_bytes(),
            k1.as_bytes()
        );
    }

    #[test]
    fn unicode_normalisation() {
        let p = KdfParams::new_random(KdfCost::MINIMUM).unwrap();
        // "é" precomposed vs. "e" + combining acute accent.
        let a = p.derive_key("caf\u{e9}").unwrap();
        let b = p.derive_key("cafe\u{301}").unwrap();
        assert_eq!(a.as_bytes(), b.as_bytes());
    }

    #[test]
    fn rejects_downgraded_or_abusive_params() {
        let good = KdfParams::new_random(KdfCost::MINIMUM).unwrap();
        let mut p = good.clone();
        p.memory_kib = 1024;
        assert!(p.derive_key("x").is_err());
        let mut p = good.clone();
        p.iterations = 1;
        assert!(p.validate().is_err());
        let mut p = good.clone();
        p.memory_kib = MAX_MEMORY_KIB + 1;
        assert!(p.validate().is_err());
        let mut p = good.clone();
        p.algorithm = "pbkdf2".into();
        assert!(p.validate().is_err());
        let mut p = good;
        p.salt.0.truncate(8);
        assert!(p.validate().is_err());
    }
}
