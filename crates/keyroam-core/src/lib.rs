//! KeyRoam vault engine.
//!
//! This crate contains everything security-critical that does not involve the
//! network: master-password key derivation, authenticated encryption of vault
//! data, the vault data model with its conflict-free merge, encrypted backups
//! and local persistence. It is shared by every KeyRoam front-end.

#![forbid(unsafe_code)]

pub mod crypto;
pub mod envelope;
pub mod error;
pub mod files;
pub mod generator;
pub mod header;
pub mod kdf;
pub mod model;
pub mod storage;
pub mod vault;

pub use error::{Error, Result};
pub use files::{BackupFile, LocalVaultFile, ReplicaFile, SecretsFile};
pub use header::VaultHeader;
pub use kdf::KdfCost;
pub use model::{DeviceId, EntryData, EntryId, EntryRecord, VaultState, Version};
pub use storage::LocalStore;
pub use vault::{LocalSecrets, Vault};

/// Milliseconds since the Unix epoch.
pub fn now_millis() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis() as i64)
        .unwrap_or(0)
}
