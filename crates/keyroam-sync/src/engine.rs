//! Conflict-safe synchronisation over Google Drive's `appDataFolder`.
//!
//! Drive offers no atomic compare-and-swap, so KeyRoam avoids shared mutable
//! files for vault data altogether:
//!
//! * `keyroam.header.json` — the vault header (KDF params + wrapped vault key).
//!   Written once at creation and only rewritten on a master-password change.
//! * `keyroam.replica.<device-id>.json` — one encrypted replica per device.
//!   **Only that device ever writes it**, so concurrent writers cannot clobber
//!   each other.
//!
//! A sync downloads every changed replica, merges it into the local state with
//! the CRDT merge in `keyroam_core::model` (concurrent edits become visible
//! conflicts, never silent overwrites), saves locally (keeping a rotating
//! backup of the previous file) and uploads this device's replica if anything
//! changed. See `docs/sync-protocol.md`.

use std::collections::BTreeMap;

use keyroam_core::{now_millis, LocalStore, ReplicaFile, Vault, VaultHeader};
use serde::{Deserialize, Serialize};
use uuid::Uuid;

use crate::error::{Result, SyncError};
use crate::remote::{RemoteFile, RemoteStore};

pub const HEADER_NAME: &str = "keyroam.header.json";
const REPLICA_PREFIX: &str = "keyroam.replica.";
const REPLICA_SUFFIX: &str = ".json";
pub const SYNC_STATE_FILE: &str = "sync-state.json";

pub fn replica_name(device: Uuid) -> String {
    format!("{REPLICA_PREFIX}{device}{REPLICA_SUFFIX}")
}

fn parse_replica_name(name: &str) -> Option<Uuid> {
    name.strip_prefix(REPLICA_PREFIX)?
        .strip_suffix(REPLICA_SUFFIX)?
        .parse()
        .ok()
}

#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct RemoteRef {
    pub id: String,
    pub version: String,
}

/// Non-secret sync bookkeeping persisted next to the vault.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct SyncState {
    pub vault_id: Option<Uuid>,
    /// Local changes not yet uploaded.
    #[serde(default)]
    pub pending_upload: bool,
    #[serde(default)]
    pub last_attempt_at: Option<i64>,
    #[serde(default)]
    pub last_success_at: Option<i64>,
    #[serde(default)]
    pub last_error: Option<String>,
    #[serde(default)]
    pub last_error_offline: bool,
    #[serde(default)]
    pub header_file_id: Option<String>,
    #[serde(default)]
    pub own_replica: Option<RemoteRef>,
    /// Drive file id → last merged version, to skip unchanged replicas.
    #[serde(default)]
    pub seen_versions: BTreeMap<String, String>,
    /// Incremented by every local commit. A sync round that started at an
    /// older generation knows another process committed meanwhile and keeps
    /// `pending_upload` set.
    #[serde(default)]
    pub local_generation: u64,
}

impl SyncState {
    pub fn load(store: &LocalStore) -> Result<SyncState> {
        Ok(store.read_json(SYNC_STATE_FILE)?.unwrap_or_default())
    }

    pub fn save(&self, store: &LocalStore) -> Result<()> {
        Ok(store.write_json(SYNC_STATE_FILE, self)?)
    }

    fn for_vault(store: &LocalStore, vault: &Vault) -> Result<SyncState> {
        let st = Self::load(store)?;
        if st.vault_id == Some(vault.vault_id()) {
            Ok(st)
        } else {
            Ok(SyncState {
                vault_id: Some(vault.vault_id()),
                pending_upload: true,
                ..Default::default()
            })
        }
    }
}

/// User-facing synchronisation status.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum SyncStatus {
    NeverSynced,
    /// Everything uploaded; `at` is the last successful sync.
    UpToDate {
        at: i64,
    },
    /// Local changes waiting for connectivity / the next sync.
    Pending {
        last_success_at: Option<i64>,
    },
    Offline {
        last_success_at: Option<i64>,
    },
    Error {
        message: String,
        last_success_at: Option<i64>,
    },
}

impl SyncState {
    pub fn status(&self) -> SyncStatus {
        let last_success_at = self.last_success_at;
        if let Some(message) = &self.last_error {
            return if self.last_error_offline {
                SyncStatus::Offline { last_success_at }
            } else {
                SyncStatus::Error {
                    message: message.clone(),
                    last_success_at,
                }
            };
        }
        if self.pending_upload {
            return SyncStatus::Pending { last_success_at };
        }
        match last_success_at {
            Some(at) => SyncStatus::UpToDate { at },
            None => SyncStatus::NeverSynced,
        }
    }
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct SyncReport {
    /// Replicas downloaded and merged.
    pub replicas_merged: usize,
    /// Whether remote data changed the local vault.
    pub local_changed: bool,
    pub uploaded: bool,
    pub header_uploaded: bool,
    /// The master password was changed on another device; the new password
    /// is required for future unlocks.
    pub header_adopted: bool,
    pub conflicts: usize,
    /// Non-fatal problems (e.g. a corrupted replica from another device).
    pub warnings: Vec<String>,
}

/// Make a local modification: under the data-directory lock, merge whatever
/// another KeyRoam process saved, apply `change`, persist the vault and mark
/// it for upload. With `backup_previous`, the previous vault file is kept in
/// the rotating local backups (used for imports and password changes).
pub fn commit_local_change<T>(
    vault: &mut Vault,
    store: &LocalStore,
    backup_previous: bool,
    change: impl FnOnce(&mut Vault) -> keyroam_core::Result<T>,
) -> Result<T> {
    let _lock = store.lock()?;
    store.absorb(vault)?;
    let out = change(vault)?;
    store.save_vault(&vault.to_local_file()?, backup_previous)?;
    let mut st = SyncState::for_vault(store, vault)?;
    st.pending_upload = true;
    st.local_generation += 1;
    st.save(store)?;
    Ok(out)
}

/// Run one synchronisation round ("Sync Now"). On failure the local vault is
/// untouched apart from merges already saved, and the error is recorded in
/// the sync state for display.
pub fn sync(vault: &mut Vault, store: &LocalStore, remote: &dyn RemoteStore) -> Result<SyncReport> {
    let mut st = {
        let _lock = store.lock()?;
        store.absorb(vault)?;
        SyncState::for_vault(store, vault)?
    };
    st.last_attempt_at = Some(now_millis());
    let result = sync_inner(vault, store, remote, &mut st);
    match &result {
        Ok(_) => {
            st.last_success_at = st.last_attempt_at;
            st.last_error = None;
            st.last_error_offline = false;
        }
        Err(e) => {
            st.last_error = Some(e.to_string());
            st.last_error_offline = e.is_offline();
        }
    }
    {
        let _lock = store.lock()?;
        let on_disk = SyncState::load(store)?;
        if on_disk.vault_id == st.vault_id && on_disk.local_generation != st.local_generation {
            // Another process committed during this round; its change may not
            // be in the replica just uploaded.
            st.pending_upload = true;
            st.local_generation = on_disk.local_generation;
        }
        st.save(store)?;
    }
    let mut report = result?;
    report.conflicts = vault.state().conflict_count();
    Ok(report)
}

fn sync_inner(
    vault: &mut Vault,
    store: &LocalStore,
    remote: &dyn RemoteStore,
    st: &mut SyncState,
) -> Result<SyncReport> {
    let mut report = SyncReport::default();
    let files = remote.list()?;
    let mut save_needed = false;

    // 1. Header.
    match canonical_header(&files) {
        None => {
            let f = remote.create(
                HEADER_NAME,
                &serde_json::to_vec_pretty(vault.header()).map_err(core_err)?,
            )?;
            st.header_file_id = Some(f.id);
            report.header_uploaded = true;
        }
        Some(hf) => {
            let remote_header = parse_header(&remote.download(&hf.id)?)?;
            if remote_header.vault_id != vault.vault_id() {
                return Err(keyroam_core::Error::VaultMismatch.into());
            }
            if vault.adopt_header(&remote_header)? {
                report.header_adopted = true;
                save_needed = true;
            } else if remote_header.revision < vault.header().revision {
                remote.update(&hf.id, &serde_json::to_vec_pretty(vault.header()).map_err(core_err)?)?;
                report.header_uploaded = true;
            }
            st.header_file_id = Some(hf.id.clone());
        }
    }

    // 2. Pull and merge replicas.
    let own = vault.device_id();
    let mut own_file: Option<&RemoteFile> = None;
    for f in &files {
        let Some(device) = parse_replica_name(&f.name) else {
            continue;
        };
        if device == own {
            // Prefer the file we wrote last; any other copy is merged below.
            let is_ours = st.own_replica.as_ref().is_some_and(|r| r.id == f.id);
            if own_file.is_none() || is_ours {
                own_file = Some(f);
            }
            // Someone else wrote "our" file (e.g. a copied profile) or we lost
            // our bookkeeping: merge it so nothing is overwritten unseen.
            let unchanged = st
                .own_replica
                .as_ref()
                .is_some_and(|r| r.id == f.id && r.version == f.version);
            if unchanged {
                continue;
            }
        } else if st.seen_versions.get(&f.id) == Some(&f.version) {
            continue;
        }
        match pull_replica(vault, remote, f, device) {
            Ok(changed) => {
                report.replicas_merged += 1;
                if changed {
                    report.local_changed = true;
                    save_needed = true;
                }
                st.seen_versions.insert(f.id.clone(), f.version.clone());
            }
            Err(SyncError::Core(e)) if !matches!(e, keyroam_core::Error::UnsupportedVersion { .. }) => {
                report.warnings.push(format!("skipped replica {}: {e}", f.name));
            }
            Err(e) => return Err(e),
        }
    }
    let live: std::collections::BTreeSet<&str> = files.iter().map(|f| f.id.as_str()).collect();
    st.seen_versions.retain(|id, _| live.contains(id.as_str()));

    // 3. Persist merged state locally before touching the remote copy.
    if save_needed {
        let _lock = store.lock()?;
        store.absorb(vault)?;
        store.save_vault(&vault.to_local_file()?, true)?;
    }
    if report.local_changed {
        st.pending_upload = true;
    }

    // 4. Upload this device's replica.
    if st.pending_upload || own_file.is_none() {
        let bytes = vault.seal_replica()?.to_bytes()?;
        let written = match own_file {
            Some(f) => remote.update(&f.id, &bytes)?,
            None => remote.create(&replica_name(own), &bytes)?,
        };
        st.own_replica = Some(RemoteRef {
            id: written.id,
            version: written.version,
        });
        st.pending_upload = false;
        report.uploaded = true;
    } else if let Some(f) = own_file {
        st.own_replica = Some(RemoteRef {
            id: f.id.clone(),
            version: f.version.clone(),
        });
    }
    Ok(report)
}

fn core_err(e: serde_json::Error) -> SyncError {
    SyncError::Core(e.into())
}

fn pull_replica(vault: &mut Vault, remote: &dyn RemoteStore, f: &RemoteFile, device: Uuid) -> Result<bool> {
    let replica = ReplicaFile::from_bytes(&remote.download(&f.id)?)?;
    if replica.device_id != device {
        return Err(keyroam_core::Error::Format("replica name does not match its device".into()).into());
    }
    let state = vault.open_replica(&replica)?;
    Ok(vault.merge_state(&state)?)
}

/// If several headers exist (two devices created a vault at the same moment),
/// the oldest one is canonical on every device.
fn canonical_header(files: &[RemoteFile]) -> Option<&RemoteFile> {
    files
        .iter()
        .filter(|f| f.name == HEADER_NAME)
        .min_by(|a, b| (&a.created_time, &a.id).cmp(&(&b.created_time, &b.id)))
}

fn parse_header(bytes: &[u8]) -> Result<VaultHeader> {
    serde_json::from_slice(bytes).map_err(|e| SyncError::Protocol(format!("vault header: {e}")))
}

/// Download the vault header from Drive (existing-user onboarding step).
pub fn fetch_remote_header(remote: &dyn RemoteStore) -> Result<VaultHeader> {
    let files = remote.list()?;
    let hf = canonical_header(&files).ok_or(SyncError::NoRemoteVault)?;
    parse_header(&remote.download(&hf.id)?)
}

/// New-user onboarding: create the vault, save it locally together with the
/// device secrets, then upload the header and first replica.
///
/// If the upload fails (e.g. offline) the vault is still created locally and
/// the error is returned alongside so the UI can show "pending sync".
pub fn create_vault(
    store: &LocalStore,
    remote: &dyn RemoteStore,
    password: &str,
    cost: keyroam_core::KdfCost,
    secrets: &keyroam_core::LocalSecrets,
) -> Result<(Vault, Result<SyncReport>)> {
    if store.has_vault() {
        return Err(keyroam_core::Error::VaultExists.into());
    }
    if remote.list()?.iter().any(|f| f.name == HEADER_NAME) {
        return Err(SyncError::RemoteVaultExists);
    }
    let mut vault = Vault::create(password, cost)?;
    store.save_new_vault(&vault.to_local_file()?)?;
    store.save_secrets(&vault.seal_secrets(secrets)?)?;
    let report = sync(&mut vault, store, remote);
    Ok((vault, report))
}

/// Existing-user onboarding on a new device: unlock the downloaded header with
/// the master password (authenticating it), save the device's vault, then pull
/// every replica. A failed pull is returned alongside the vault; because sync
/// only ever merges, the next sync completes the download safely.
pub fn join_vault(
    store: &LocalStore,
    remote: &dyn RemoteStore,
    header: VaultHeader,
    password: &str,
    secrets: &keyroam_core::LocalSecrets,
) -> Result<(Vault, Result<SyncReport>)> {
    if store.has_vault() {
        return Err(keyroam_core::Error::VaultExists.into());
    }
    let mut vault = Vault::join(header, password)?;
    store.save_new_vault(&vault.to_local_file()?)?;
    store.save_secrets(&vault.seal_secrets(secrets)?)?;
    let report = sync(&mut vault, store, remote);
    Ok((vault, report))
}

#[cfg(test)]
mod tests;
