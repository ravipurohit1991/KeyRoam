//! The vault header: KDF parameters plus the vault key wrapped by the
//! master-password-derived key.
//!
//! Key hierarchy:
//!
//! ```text
//! master password --Argon2id(salt)--> KEK --XChaCha20-Poly1305--> wraps vault key (random 256-bit)
//! vault key --HKDF("keyroam/v1/payload")-----> payload encryption key
//! vault key --HKDF("keyroam/v1/header-auth")--> header MAC key
//! ```
//!
//! Changing the master password only re-wraps the vault key, so encrypted
//! replicas on other devices stay readable. The header MAC (keyed by the vault
//! key) lets a device verify that a header downloaded from the cloud was
//! produced by a holder of the vault key before adopting it.

use serde::{Deserialize, Serialize};
use subtle::ConstantTimeEq;
use uuid::Uuid;

use crate::crypto::{self, B64Bytes, Sealed, SecretKey, Transcript};
use crate::error::{Error, Result};
use crate::kdf::{KdfCost, KdfParams};
use crate::now_millis;

pub const HEADER_FORMAT: &str = "keyroam-vault-header";
pub const HEADER_VERSION: u32 = 1;

const PURPOSE_HEADER_AUTH: &[u8] = b"keyroam/v1/header-auth";

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct VaultHeader {
    pub format: String,
    pub version: u32,
    pub vault_id: Uuid,
    /// Incremented on every master-password change.
    pub revision: u64,
    pub created_at: i64,
    pub updated_at: i64,
    pub kdf: KdfParams,
    pub wrapped_key: Sealed,
    /// HMAC-SHA256 over all fields above, keyed by a vault-key sub-key.
    pub auth: B64Bytes,
}

impl VaultHeader {
    /// Create a new vault key protected by `password`.
    pub fn create(password: &str, cost: KdfCost) -> Result<(VaultHeader, SecretKey)> {
        check_password_policy(password)?;
        let key = SecretKey::generate();
        let now = now_millis();
        let header = Self::build(Uuid::new_v4(), 1, now, now, password, cost, &key)?;
        Ok((header, key))
    }

    fn build(
        vault_id: Uuid,
        revision: u64,
        created_at: i64,
        updated_at: i64,
        password: &str,
        cost: KdfCost,
        key: &SecretKey,
    ) -> Result<VaultHeader> {
        let kdf = KdfParams::new_random(cost)?;
        let kek = kdf.derive_key(password)?;
        let wrapped_key = crypto::seal(&kek, key.as_bytes(), &wrap_aad(&vault_id, &kdf))?;
        let mut header = VaultHeader {
            format: HEADER_FORMAT.into(),
            version: HEADER_VERSION,
            vault_id,
            revision,
            created_at,
            updated_at,
            kdf,
            wrapped_key,
            auth: B64Bytes::default(),
        };
        header.auth = B64Bytes(crypto::hmac_sha256(
            &key.derive(PURPOSE_HEADER_AUTH),
            &header.auth_transcript(),
        ));
        Ok(header)
    }

    fn check_format(&self) -> Result<()> {
        if self.format != HEADER_FORMAT {
            return Err(Error::Format("not a KeyRoam vault header".into()));
        }
        if self.version > HEADER_VERSION || self.version == 0 {
            return Err(Error::UnsupportedVersion {
                what: "vault header",
                found: self.version,
                supported: HEADER_VERSION,
            });
        }
        Ok(())
    }

    /// Derive the KEK from `password`, unwrap the vault key and verify the
    /// header's integrity.
    pub fn unlock(&self, password: &str) -> Result<SecretKey> {
        self.check_format()?;
        let kek = self.kdf.derive_key(password)?;
        let raw = crypto::open(&kek, &self.wrapped_key, &wrap_aad(&self.vault_id, &self.kdf))
            .map_err(|_| Error::WrongPassword)?;
        let key = SecretKey::from_bytes(&raw)?;
        self.verify(&key)?;
        Ok(key)
    }

    /// Verify the header MAC with an already-known vault key.
    pub fn verify(&self, key: &SecretKey) -> Result<()> {
        self.check_format()?;
        crypto::hmac_sha256_verify(&key.derive(PURPOSE_HEADER_AUTH), &self.auth_transcript(), &self.auth.0)
    }

    /// Produce a new header revision protecting the same vault key with a new
    /// password (and fresh salt).
    pub fn rewrap(&self, key: &SecretKey, new_password: &str, cost: KdfCost) -> Result<VaultHeader> {
        check_password_policy(new_password)?;
        self.verify(key)?;
        Self::build(
            self.vault_id,
            self.revision + 1,
            self.created_at,
            now_millis().max(self.updated_at),
            new_password,
            cost,
            key,
        )
    }

    /// True when `password` unlocks this header to exactly `key`.
    pub fn password_matches(&self, key: &SecretKey, password: &str) -> bool {
        match self.unlock(password) {
            Ok(k) => bool::from(k.as_bytes().ct_eq(key.as_bytes())),
            Err(_) => false,
        }
    }

    fn auth_transcript(&self) -> Vec<u8> {
        Transcript::new("keyroam/v1/header")
            .str(&self.format)
            .u64(self.version as u64)
            .str(&self.vault_id.to_string())
            .u64(self.revision)
            .u64(self.created_at as u64)
            .u64(self.updated_at as u64)
            .bytes(&kdf_transcript(&self.kdf))
            .bytes(&self.wrapped_key.nonce.0)
            .bytes(&self.wrapped_key.ciphertext.0)
            .finish()
    }
}

fn kdf_transcript(kdf: &KdfParams) -> Vec<u8> {
    Transcript::new("keyroam/v1/kdf")
        .str(&kdf.algorithm)
        .u64(kdf.memory_kib as u64)
        .u64(kdf.iterations as u64)
        .u64(kdf.parallelism as u64)
        .bytes(&kdf.salt.0)
        .finish()
}

fn wrap_aad(vault_id: &Uuid, kdf: &KdfParams) -> Vec<u8> {
    Transcript::new("keyroam/v1/key-wrap")
        .str(&vault_id.to_string())
        .bytes(&kdf_transcript(kdf))
        .finish()
}

pub const MIN_PASSWORD_CHARS: usize = 12;

/// Minimal master-password policy enforced by the engine. UIs should add a
/// strength meter on top.
pub fn check_password_policy(password: &str) -> Result<()> {
    if password.chars().count() < MIN_PASSWORD_CHARS {
        return Err(Error::InvalidInput(format!(
            "master password must be at least {MIN_PASSWORD_CHARS} characters"
        )));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    const PW: &str = "synthetic-master-pw-1";

    #[test]
    fn create_and_unlock() {
        let (h, key) = VaultHeader::create(PW, KdfCost::MINIMUM).unwrap();
        let k2 = h.unlock(PW).unwrap();
        assert_eq!(key.as_bytes(), k2.as_bytes());
        assert!(matches!(h.unlock("wrong-password-123"), Err(Error::WrongPassword)));
    }

    #[test]
    fn rejects_short_password() {
        assert!(VaultHeader::create("short", KdfCost::MINIMUM).is_err());
    }

    #[test]
    fn tampered_metadata_is_detected() {
        let (h, key) = VaultHeader::create(PW, KdfCost::MINIMUM).unwrap();

        let mut t = h.clone();
        t.revision = 99;
        assert!(matches!(t.unlock(PW), Err(Error::Authentication)));
        assert!(t.verify(&key).is_err());

        let mut t = h.clone();
        t.vault_id = Uuid::new_v4();
        assert!(t.unlock(PW).is_err());

        let mut t = h.clone();
        t.kdf.iterations += 1;
        assert!(t.unlock(PW).is_err());

        let mut t = h;
        t.updated_at += 1;
        assert!(t.verify(&key).is_err());
    }

    #[test]
    fn rewrap_keeps_key_and_bumps_revision() {
        let (h, key) = VaultHeader::create(PW, KdfCost::MINIMUM).unwrap();
        let h2 = h.rewrap(&key, "another-synthetic-pw", KdfCost::MINIMUM).unwrap();
        assert_eq!(h2.revision, 2);
        assert_eq!(h2.vault_id, h.vault_id);
        assert_ne!(h2.kdf.salt, h.kdf.salt);
        assert!(h2.unlock(PW).is_err());
        let k2 = h2.unlock("another-synthetic-pw").unwrap();
        assert_eq!(k2.as_bytes(), key.as_bytes());
        assert!(h2.password_matches(&key, "another-synthetic-pw"));
        assert!(!h2.password_matches(&key, PW));
    }

    #[test]
    fn header_from_other_vault_fails_verification() {
        let (h, _) = VaultHeader::create(PW, KdfCost::MINIMUM).unwrap();
        let (_, other_key) = VaultHeader::create(PW, KdfCost::MINIMUM).unwrap();
        assert!(h.verify(&other_key).is_err());
    }
}
