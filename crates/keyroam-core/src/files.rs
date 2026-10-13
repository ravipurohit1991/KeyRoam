//! On-disk / on-Drive file formats. All are JSON envelopes whose sensitive
//! content is an AEAD-sealed payload; the plaintext metadata is bound to the
//! ciphertext as associated data.

use serde::de::DeserializeOwned;
use serde::{Deserialize, Serialize};
use uuid::Uuid;

use crate::crypto::Sealed;
use crate::error::{Error, Result};
use crate::header::VaultHeader;

pub const FILE_VERSION: u32 = 1;
pub const LOCAL_VAULT_FORMAT: &str = "keyroam-local-vault";
pub const REPLICA_FORMAT: &str = "keyroam-replica";
pub const BACKUP_FORMAT: &str = "keyroam-backup";
pub const SECRETS_FORMAT: &str = "keyroam-local-secrets";

/// The device's own encrypted copy of the vault.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct LocalVaultFile {
    pub format: String,
    pub version: u32,
    pub device_id: Uuid,
    pub header: VaultHeader,
    pub payload: Sealed,
}

/// One device's encrypted vault state as stored in Google Drive.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct ReplicaFile {
    pub format: String,
    pub version: u32,
    pub vault_id: Uuid,
    pub device_id: Uuid,
    pub written_at: i64,
    pub payload: Sealed,
}

/// A self-contained, password-protected export that can be restored without
/// Google Drive.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct BackupFile {
    pub format: String,
    pub version: u32,
    pub created_at: i64,
    pub source_vault_id: Uuid,
    pub entry_count: usize,
    pub header: VaultHeader,
    pub payload: Sealed,
}

/// Device-local secrets (e.g. the Google refresh token), encrypted with the
/// vault key so they are only usable after unlocking.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct SecretsFile {
    pub format: String,
    pub version: u32,
    pub vault_id: Uuid,
    pub device_id: Uuid,
    pub payload: Sealed,
}

#[derive(Deserialize)]
struct Probe {
    format: String,
    version: u32,
}

/// Identify a KeyRoam JSON file's format string without fully parsing it.
pub fn detect_format(bytes: &[u8]) -> Result<String> {
    let p: Probe = serde_json::from_slice(bytes).map_err(|_| Error::Format("not a KeyRoam file".into()))?;
    let _ = p.version;
    Ok(p.format)
}

fn parse<T: DeserializeOwned>(bytes: &[u8], format: &str, what: &'static str) -> Result<T> {
    let probe: Probe = serde_json::from_slice(bytes).map_err(|_| Error::Format(format!("not a KeyRoam {what}")))?;
    if probe.format != format {
        return Err(Error::Format(format!("not a KeyRoam {what}")));
    }
    if probe.version == 0 || probe.version > FILE_VERSION {
        return Err(Error::UnsupportedVersion {
            what,
            found: probe.version,
            supported: FILE_VERSION,
        });
    }
    Ok(serde_json::from_slice(bytes)?)
}

macro_rules! file_io {
    ($ty:ty, $fmt:expr, $what:expr) => {
        impl $ty {
            pub fn to_bytes(&self) -> Result<Vec<u8>> {
                Ok(serde_json::to_vec_pretty(self)?)
            }
            pub fn from_bytes(bytes: &[u8]) -> Result<Self> {
                parse(bytes, $fmt, $what)
            }
        }
    };
}

file_io!(LocalVaultFile, LOCAL_VAULT_FORMAT, "local vault");
file_io!(ReplicaFile, REPLICA_FORMAT, "replica");
file_io!(BackupFile, BACKUP_FORMAT, "backup");
file_io!(SecretsFile, SECRETS_FORMAT, "secrets file");
