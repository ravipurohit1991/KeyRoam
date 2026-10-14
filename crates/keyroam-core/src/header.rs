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
//!
//! Rotating the vault key ([`VaultHeader::rotate`]) creates a fresh key with
//! the next `key_epoch` and a version-2 header. The earlier keys travel inside
//! it, encrypted under the new key (`previous_keys`), so data that devices
//! wrote before adopting the new key stays readable. The new key is never
//! encrypted under an old one: whoever holds an old key or an old password
//! learns nothing about it. Other devices need the master password to adopt
//! a rotated header.

use serde::{Deserialize, Serialize};
use subtle::ConstantTimeEq;
use uuid::Uuid;

use crate::crypto::{self, B64Bytes, Sealed, SecretKey, Transcript};
use crate::error::{Error, Result};
use crate::kdf::{KdfCost, KdfParams};
use crate::now_millis;

pub const HEADER_FORMAT: &str = "keyroam-vault-header";
/// Version 1: never rotated. Version 2: rotated (`key_epoch` ≥ 2).
pub const HEADER_VERSION: u32 = 2;

const PURPOSE_HEADER_AUTH: &[u8] = b"keyroam/v1/header-auth";
const PURPOSE_KEYRING: &[u8] = b"keyroam/v2/keyring";

fn first_epoch() -> u32 {
    1
}
fn is_first_epoch(e: &u32) -> bool {
    *e == 1
}

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
    /// Generation of the vault key, incremented by every key rotation.
    #[serde(default = "first_epoch", skip_serializing_if = "is_first_epoch")]
    pub key_epoch: u32,
    /// Earlier vault keys with their epochs, encrypted under the current key.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub previous_keys: Option<Sealed>,
    /// HMAC-SHA256 over all fields above, keyed by a vault-key sub-key.
    pub auth: B64Bytes,
}

/// An earlier vault key and its epoch.
pub type PreviousKey = (u32, SecretKey);

impl VaultHeader {
    /// Create a new vault key protected by `password`.
    pub fn create(password: &str, cost: KdfCost) -> Result<(VaultHeader, SecretKey)> {
        check_password_policy(password)?;
        let key = SecretKey::generate();
        let now = now_millis();
        let header = Self::build(Uuid::new_v4(), 1, now, now, password, cost, &key, 1, &[])?;
        Ok((header, key))
    }

    #[allow(clippy::too_many_arguments)]
    fn build(
        vault_id: Uuid,
        revision: u64,
        created_at: i64,
        updated_at: i64,
        password: &str,
        cost: KdfCost,
        key: &SecretKey,
        key_epoch: u32,
        previous: &[PreviousKey],
    ) -> Result<VaultHeader> {
        let kdf = KdfParams::new_random(cost)?;
        let kek = kdf.derive_key(password)?;
        let wrapped_key = crypto::seal(&kek, key.as_bytes(), &wrap_aad(&vault_id, &kdf))?;
        let previous_keys = if key_epoch == 1 {
            None
        } else {
            let mut raw = zeroize::Zeroizing::new(Vec::with_capacity(previous.len() * 36));
            for (epoch, k) in previous {
                raw.extend_from_slice(&epoch.to_be_bytes());
                raw.extend_from_slice(k.as_bytes());
            }
            Some(crypto::seal(
                &key.derive(PURPOSE_KEYRING),
                &raw,
                &keyring_aad(&vault_id, key_epoch),
            )?)
        };
        let mut header = VaultHeader {
            format: HEADER_FORMAT.into(),
            version: if key_epoch == 1 { 1 } else { 2 },
            vault_id,
            revision,
            created_at,
            updated_at,
            kdf,
            wrapped_key,
            key_epoch,
            previous_keys,
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
        let rotated = self.version >= 2;
        if rotated != (self.key_epoch >= 2) || rotated != self.previous_keys.is_some() || self.key_epoch == 0 {
            return Err(Error::Format("inconsistent vault key epoch".into()));
        }
        Ok(())
    }

    /// The earlier vault keys of a rotated header, newest first. `key` must
    /// be this header's key.
    pub fn previous_keys(&self, key: &SecretKey) -> Result<Vec<PreviousKey>> {
        let Some(sealed) = &self.previous_keys else {
            return Ok(Vec::new());
        };
        let raw = crypto::open(
            &key.derive(PURPOSE_KEYRING),
            sealed,
            &keyring_aad(&self.vault_id, self.key_epoch),
        )?;
        if raw.len() % 36 != 0 {
            return Err(Error::Format("malformed key list".into()));
        }
        raw.chunks(36)
            .map(|c| {
                let epoch = u32::from_be_bytes([c[0], c[1], c[2], c[3]]);
                Ok((epoch, SecretKey::from_bytes(&c[4..])?))
            })
            .collect()
    }

    /// A new header revision with a fresh vault key of the next epoch,
    /// protected by `password`. Returns the header and the new key.
    pub fn rotate(&self, key: &SecretKey, password: &str, cost: KdfCost) -> Result<(VaultHeader, SecretKey)> {
        check_password_policy(password)?;
        self.verify(key)?;
        let mut previous = self.previous_keys(key)?;
        previous.insert(0, (self.key_epoch, key.clone()));
        let new_key = SecretKey::generate();
        let header = Self::build(
            self.vault_id,
            self.revision + 1,
            self.created_at,
            now_millis().max(self.updated_at),
            password,
            cost,
            &new_key,
            self.key_epoch + 1,
            &previous,
        )?;
        Ok((header, new_key))
    }

    /// Derive the KEK from `password`, unwrap the vault key and verify the
    /// header's integrity.
    pub fn unlock(&self, password: &str) -> Result<SecretKey> {
        self.check_format()?;
        self.unlock_with_kek(&self.kdf.derive_key(password)?)
    }

    /// The key-encryption key for `password`, checked against this header.
    ///
    /// A device can keep the KEK under hardware protection (Android
    /// Keystore, biometric) to unlock without typing the password. It is
    /// bound to this header's salt and KDF parameters, so it stops working
    /// when the master password is changed or the vault key is rotated.
    pub fn derive_kek(&self, password: &str) -> Result<SecretKey> {
        self.check_format()?;
        let kek = self.kdf.derive_key(password)?;
        self.unlock_with_kek(&kek)?;
        Ok(kek)
    }

    /// Unwrap the vault key with a KEK from [`VaultHeader::derive_kek`] and
    /// verify the header's integrity. A KEK for an earlier password or salt
    /// fails with [`Error::WrongPassword`].
    pub fn unlock_with_kek(&self, kek: &SecretKey) -> Result<SecretKey> {
        self.check_format()?;
        let raw = crypto::open(kek, &self.wrapped_key, &wrap_aad(&self.vault_id, &self.kdf))
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
        let previous = self.previous_keys(key)?;
        Self::build(
            self.vault_id,
            self.revision + 1,
            self.created_at,
            now_millis().max(self.updated_at),
            new_password,
            cost,
            key,
            self.key_epoch,
            &previous,
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
        let mut t = Transcript::new("keyroam/v1/header");
        t.str(&self.format)
            .u64(self.version as u64)
            .str(&self.vault_id.to_string())
            .u64(self.revision)
            .u64(self.created_at as u64)
            .u64(self.updated_at as u64)
            .bytes(&kdf_transcript(&self.kdf))
            .bytes(&self.wrapped_key.nonce.0)
            .bytes(&self.wrapped_key.ciphertext.0);
        if self.version >= 2 {
            t.u64(self.key_epoch as u64);
            if let Some(p) = &self.previous_keys {
                t.bytes(&p.nonce.0).bytes(&p.ciphertext.0);
            }
        }
        t.finish()
    }
}

fn keyring_aad(vault_id: &Uuid, epoch: u32) -> Vec<u8> {
    Transcript::new("keyroam/v2/keyring")
        .str(&vault_id.to_string())
        .u64(epoch as u64)
        .finish()
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
    fn kek_unlocks_until_the_password_changes() {
        let (h, key) = VaultHeader::create(PW, KdfCost::MINIMUM).unwrap();
        assert!(matches!(h.derive_kek("wrong-password-xyz"), Err(Error::WrongPassword)));
        let kek = h.derive_kek(PW).unwrap();
        assert_eq!(h.unlock_with_kek(&kek).unwrap().as_bytes(), key.as_bytes());
        // A random key is not a KEK.
        assert!(matches!(
            h.unlock_with_kek(&SecretKey::generate()),
            Err(Error::WrongPassword)
        ));
        // After a password change (fresh salt) the stored KEK no longer works,
        // even if the same password is chosen again.
        let rewrapped = h.rewrap(&key, PW, KdfCost::MINIMUM).unwrap();
        assert!(matches!(rewrapped.unlock_with_kek(&kek), Err(Error::WrongPassword)));
        // After a key rotation it no longer works either.
        let (rotated, _) = h.rotate(&key, PW, KdfCost::MINIMUM).unwrap();
        assert!(matches!(rotated.unlock_with_kek(&kek), Err(Error::WrongPassword)));
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
    fn rotation_makes_a_fresh_key_and_keeps_the_old_ones_behind_it() {
        let (h1, k1) = VaultHeader::create(PW, KdfCost::MINIMUM).unwrap();
        let v1 = serde_json::to_string(&h1).unwrap();
        assert!(
            !v1.contains("key_epoch") && !v1.contains("previous_keys"),
            "v1 headers are unchanged"
        );
        assert_eq!(h1.version, 1);

        let (h2, k2) = h1.rotate(&k1, PW, KdfCost::MINIMUM).unwrap();
        assert_ne!(k1.as_bytes(), k2.as_bytes());
        assert_eq!((h2.version, h2.key_epoch, h2.revision), (2, 2, 2));
        assert_eq!(h2.unlock(PW).unwrap().as_bytes(), k2.as_bytes());
        // Old key holders cannot verify (or read anything from) the new header.
        assert!(h2.verify(&k1).is_err());
        assert!(h2.previous_keys(&k1).is_err());
        let prev = h2.previous_keys(&k2).unwrap();
        assert_eq!(prev.len(), 1);
        assert_eq!((prev[0].0, prev[0].1.as_bytes()), (1, k1.as_bytes()));

        // A password change keeps the key and the key list.
        let h2b = h2.rewrap(&k2, "another-synthetic-pw", KdfCost::MINIMUM).unwrap();
        assert_eq!(h2b.key_epoch, 2);
        assert_eq!(h2b.previous_keys(&k2).unwrap()[0].1.as_bytes(), k1.as_bytes());

        let (h3, k3) = h2b.rotate(&k2, "another-synthetic-pw", KdfCost::MINIMUM).unwrap();
        let epochs: Vec<u32> = h3.previous_keys(&k3).unwrap().iter().map(|p| p.0).collect();
        assert_eq!(epochs, vec![2, 1]);
        assert!(h1.rotate(&k1, "short", KdfCost::MINIMUM).is_err());
    }

    #[test]
    fn rotated_header_fields_are_authenticated() {
        let (h1, k1) = VaultHeader::create(PW, KdfCost::MINIMUM).unwrap();
        let (h2, k2) = h1.rotate(&k1, PW, KdfCost::MINIMUM).unwrap();
        let mut t = h2.clone();
        t.key_epoch = 3;
        assert!(t.verify(&k2).is_err());
        let mut t = h2.clone();
        t.previous_keys = h1.rotate(&k1, PW, KdfCost::MINIMUM).unwrap().0.previous_keys;
        assert!(t.verify(&k2).is_err());
        // Downgrading to v1 or mixing fields is refused outright.
        let mut t = h2.clone();
        t.version = 1;
        assert!(matches!(t.verify(&k2), Err(Error::Format(_))));
        let mut t = h1.clone();
        t.key_epoch = 2;
        assert!(matches!(t.verify(&k1), Err(Error::Format(_))));
    }

    #[test]
    fn header_from_other_vault_fails_verification() {
        let (h, _) = VaultHeader::create(PW, KdfCost::MINIMUM).unwrap();
        let (_, other_key) = VaultHeader::create(PW, KdfCost::MINIMUM).unwrap();
        assert!(h.verify(&other_key).is_err());
    }
}
