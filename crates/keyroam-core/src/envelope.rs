//! Encrypted payload envelopes.
//!
//! Every encrypted blob is bound (via AEAD associated data) to its purpose, the
//! vault it belongs to and, where relevant, the device that wrote it. A blob
//! cannot be moved between vaults, devices or purposes without failing
//! authentication.

use serde::de::DeserializeOwned;
use serde::Serialize;
use uuid::Uuid;
use zeroize::Zeroizing;

use crate::crypto::{self, Sealed, SecretKey, Transcript};
use crate::error::Result;

const PURPOSE_PAYLOAD: &[u8] = b"keyroam/v1/payload";

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PayloadKind {
    /// The device's local vault copy.
    Local,
    /// A device's replica uploaded to Google Drive.
    Replica,
    /// An exported backup file.
    Backup,
    /// Device-local secrets such as the Google refresh token.
    LocalSecrets,
}

impl PayloadKind {
    fn label(self) -> &'static str {
        match self {
            PayloadKind::Local => "local",
            PayloadKind::Replica => "replica",
            PayloadKind::Backup => "backup",
            PayloadKind::LocalSecrets => "local-secrets",
        }
    }
}

#[derive(Clone, Copy, Debug)]
pub struct PayloadContext {
    pub kind: PayloadKind,
    pub vault_id: Uuid,
    pub device_id: Option<Uuid>,
}

impl PayloadContext {
    fn aad(&self) -> Vec<u8> {
        Transcript::new("keyroam/v1/payload-aad")
            .str(self.kind.label())
            .str(&self.vault_id.to_string())
            .str(&self.device_id.map(|d| d.to_string()).unwrap_or_default())
            .finish()
    }
}

/// Serialise `value` as JSON, pad it and encrypt it with a sub-key of `vault_key`.
pub fn seal_json<T: Serialize>(vault_key: &SecretKey, ctx: &PayloadContext, value: &T) -> Result<Sealed> {
    let json = Zeroizing::new(serde_json::to_vec(value)?);
    let padded = crypto::pad(&json);
    crypto::seal(&vault_key.derive(PURPOSE_PAYLOAD), &padded, &ctx.aad())
}

/// Decrypt, authenticate and deserialise a payload produced by [`seal_json`].
pub fn open_json<T: DeserializeOwned>(vault_key: &SecretKey, ctx: &PayloadContext, sealed: &Sealed) -> Result<T> {
    let padded = crypto::open(&vault_key.derive(PURPOSE_PAYLOAD), sealed, &ctx.aad())?;
    let json = crypto::unpad(&padded)?;
    Ok(serde_json::from_slice(json)?)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::error::Error;

    fn ctx(kind: PayloadKind, vault: Uuid, dev: Option<Uuid>) -> PayloadContext {
        PayloadContext {
            kind,
            vault_id: vault,
            device_id: dev,
        }
    }

    #[test]
    fn bound_to_context() {
        let key = SecretKey::generate();
        let (v, d) = (Uuid::new_v4(), Uuid::new_v4());
        let c = ctx(PayloadKind::Replica, v, Some(d));
        let sealed = seal_json(&key, &c, &vec!["synthetic".to_string()]).unwrap();
        let back: Vec<String> = open_json(&key, &c, &sealed).unwrap();
        assert_eq!(back, vec!["synthetic".to_string()]);

        for wrong in [
            ctx(PayloadKind::Local, v, Some(d)),
            ctx(PayloadKind::Replica, Uuid::new_v4(), Some(d)),
            ctx(PayloadKind::Replica, v, Some(Uuid::new_v4())),
            ctx(PayloadKind::Replica, v, None),
        ] {
            let r: Result<Vec<String>> = open_json(&key, &wrong, &sealed);
            assert!(matches!(r, Err(Error::Authentication)));
        }
    }
}
