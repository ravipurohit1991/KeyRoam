//! The unlocked vault: the single object through which UIs read and modify
//! entries, and through which the sync engine exchanges encrypted state.

use serde::{Deserialize, Serialize};
use uuid::Uuid;
use zeroize::{Zeroize, ZeroizeOnDrop};

use crate::crypto::SecretKey;
use crate::envelope::{open_json, seal_json, PayloadContext, PayloadKind};
use crate::error::{Error, Result};
use crate::files::*;
use crate::header::{PreviousKey, VaultHeader};
use crate::kdf::KdfCost;
use crate::model::{DeviceId, EntryData, EntryId, EntryRecord, VaultState};
use crate::now_millis;

/// Secrets that belong to this device only and are never synchronised.
#[derive(Clone, Default, PartialEq, Eq, Serialize, Deserialize, Zeroize, ZeroizeOnDrop)]
pub struct LocalSecrets {
    #[serde(default)]
    pub google_refresh_token: Option<String>,
}

impl std::fmt::Debug for LocalSecrets {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("LocalSecrets")
            .field(
                "google_refresh_token",
                &self.google_refresh_token.as_ref().map(|_| "<redacted>"),
            )
            .finish()
    }
}

pub struct Vault {
    header: VaultHeader,
    key: SecretKey,
    /// Earlier vault keys (after key rotations), to read data written before.
    previous: Vec<PreviousKey>,
    device_id: DeviceId,
    state: VaultState,
}

impl std::fmt::Debug for Vault {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Vault")
            .field("vault_id", &self.header.vault_id)
            .field("device_id", &self.device_id)
            .field("entries", &self.state.entries.len())
            .finish_non_exhaustive()
    }
}

impl Drop for Vault {
    fn drop(&mut self) {
        self.state.wipe();
    }
}

impl Vault {
    /// Create a brand-new, empty vault on this device.
    pub fn create(password: &str, cost: KdfCost) -> Result<Vault> {
        let (header, key) = VaultHeader::create(password, cost)?;
        Ok(Vault {
            header,
            key,
            previous: Vec::new(),
            device_id: Uuid::new_v4(),
            state: VaultState::default(),
        })
    }

    /// Unlock this device's local vault file.
    pub fn open(file: &LocalVaultFile, password: &str) -> Result<Vault> {
        let key = file.header.unlock(password)?;
        let previous = file.header.previous_keys(&key)?;
        let state: VaultState = open_json(&key, &local_ctx(file.header.vault_id, file.device_id), &file.payload)?;
        state.validate()?;
        Ok(Vault {
            header: file.header.clone(),
            key,
            previous,
            device_id: file.device_id,
            state,
        })
    }

    /// Join an existing vault (downloaded header) as a new device.
    pub fn join(header: VaultHeader, password: &str) -> Result<Vault> {
        let key = header.unlock(password)?;
        let previous = header.previous_keys(&key)?;
        Ok(Vault {
            header,
            key,
            previous,
            device_id: Uuid::new_v4(),
            state: VaultState::default(),
        })
    }

    pub fn key_epoch(&self) -> u32 {
        self.header.key_epoch
    }

    /// The vault key of `epoch`: the current one or an earlier one. A newer
    /// epoch means the key was rotated elsewhere and this device must adopt it.
    fn key_for(&self, epoch: u32) -> Result<&SecretKey> {
        if epoch == self.header.key_epoch {
            return Ok(&self.key);
        }
        if epoch > self.header.key_epoch {
            return Err(Error::KeyRotated);
        }
        self.previous
            .iter()
            .find(|(e, _)| *e == epoch)
            .map(|(_, k)| k)
            .ok_or(Error::Authentication)
    }

    pub fn header(&self) -> &VaultHeader {
        &self.header
    }
    pub fn vault_id(&self) -> Uuid {
        self.header.vault_id
    }
    pub fn device_id(&self) -> DeviceId {
        self.device_id
    }
    pub fn state(&self) -> &VaultState {
        &self.state
    }

    pub fn to_local_file(&self) -> Result<LocalVaultFile> {
        Ok(LocalVaultFile {
            format: LOCAL_VAULT_FORMAT.into(),
            version: FILE_VERSION,
            device_id: self.device_id,
            header: self.header.clone(),
            payload: seal_json(&self.key, &local_ctx(self.vault_id(), self.device_id), &self.state)?,
        })
    }

    // ----- Reading -----

    /// Entries that are visible to the user: not deleted, or in conflict.
    pub fn entries(&self) -> impl Iterator<Item = &EntryRecord> {
        self.state
            .entries
            .values()
            .filter(|r| !r.is_deleted() || r.has_conflict())
    }

    /// Deleted entries that can still be restored.
    pub fn deleted_entries(&self) -> impl Iterator<Item = &EntryRecord> {
        self.state
            .entries
            .values()
            .filter(|r| r.is_deleted() && !r.has_conflict())
    }

    pub fn conflicts(&self) -> impl Iterator<Item = &EntryRecord> {
        self.state.entries.values().filter(|r| r.has_conflict())
    }

    pub fn get(&self, id: EntryId) -> Option<&EntryRecord> {
        self.state.entries.get(&id)
    }

    /// Case-insensitive search over title, username, URLs and tags.
    pub fn search(&self, query: &str) -> Vec<&EntryRecord> {
        let q = query.to_lowercase();
        self.entries()
            .filter(|r| {
                r.versions.iter().filter_map(|v| v.data.as_ref()).any(|d| {
                    d.title.to_lowercase().contains(&q)
                        || d.username.to_lowercase().contains(&q)
                        || d.urls.iter().any(|u| u.to_lowercase().contains(&q))
                        || d.tags.iter().any(|t| t.to_lowercase().contains(&q))
                })
            })
            .collect()
    }

    /// Resolve an id or unique id prefix (as shown by the CLI) to an entry id.
    pub fn resolve_id(&self, prefix: &str) -> Result<EntryId> {
        let p = prefix.to_lowercase();
        let matches: Vec<EntryId> = self
            .state
            .entries
            .keys()
            .filter(|id| id.to_string().starts_with(&p))
            .copied()
            .collect();
        match matches.as_slice() {
            [one] => Ok(*one),
            [] => Err(Error::EntryNotFound),
            _ => Err(Error::InvalidInput("id prefix is ambiguous".into())),
        }
    }

    // ----- Local modifications -----

    pub fn add_entry(&mut self, data: EntryData) -> Result<EntryId> {
        data.validate()?;
        let id = Uuid::new_v4();
        let rec = EntryRecord::new_local(id, self.device_id, now_millis(), data);
        self.state.entries.insert(id, rec);
        Ok(id)
    }

    /// Replace an entry's content. Editing an entry that has a conflict
    /// resolves the conflict in favour of the new content.
    pub fn update_entry(&mut self, id: EntryId, data: EntryData) -> Result<()> {
        data.validate()?;
        let device = self.device_id;
        let rec = self.state.entries.get_mut(&id).ok_or(Error::EntryNotFound)?;
        rec.supersede(device, now_millis(), Some(data));
        self.state.enforce_purge(id);
        Ok(())
    }

    pub fn delete_entry(&mut self, id: EntryId) -> Result<()> {
        let device = self.device_id;
        let rec = self.state.entries.get_mut(&id).ok_or(Error::EntryNotFound)?;
        if rec.is_deleted() && !rec.has_conflict() {
            return Ok(());
        }
        rec.supersede(device, now_millis(), None);
        self.state.enforce_purge(id);
        Ok(())
    }

    /// Undelete an entry using its most recent non-deleted content.
    pub fn restore_entry(&mut self, id: EntryId) -> Result<()> {
        let rec = self.state.entries.get(&id).ok_or(Error::EntryNotFound)?;
        let data = rec
            .versions
            .iter()
            .chain(&rec.history)
            .find_map(|v| v.data.clone())
            .ok_or_else(|| Error::InvalidInput("no content to restore".into()))?;
        self.update_entry(id, data)
    }

    /// Permanently erase a deleted entry, with its password history, on this
    /// device and, after syncing, on every device. Only entries in "recently
    /// deleted" can be purged.
    pub fn purge_entry(&mut self, id: EntryId) -> Result<()> {
        let rec = self.state.entries.get(&id).ok_or(Error::EntryNotFound)?;
        if !rec.is_deleted() || rec.has_conflict() {
            return Err(Error::InvalidInput("only deleted entries can be purged".into()));
        }
        let clock = rec.joined_clock();
        self.state.add_purge(id, clock);
        Ok(())
    }

    /// Purge every deleted entry. Returns how many were purged.
    pub fn purge_deleted(&mut self) -> Result<usize> {
        let ids: Vec<EntryId> = self.deleted_entries().map(|r| r.id).collect();
        for id in &ids {
            self.purge_entry(*id)?;
        }
        Ok(ids.len())
    }

    /// Resolve a conflict by keeping sibling `choice` (index into `versions`).
    /// The other versions remain available in the entry history.
    pub fn resolve_conflict(&mut self, id: EntryId, choice: usize) -> Result<()> {
        let rec = self.state.entries.get(&id).ok_or(Error::EntryNotFound)?;
        let chosen = rec
            .versions
            .get(choice)
            .ok_or_else(|| Error::InvalidInput("no such version".into()))?
            .data
            .clone();
        let device = self.device_id;
        let rec = self.state.entries.get_mut(&id).expect("checked above");
        rec.supersede(device, now_millis(), chosen);
        self.state.enforce_purge(id);
        Ok(())
    }

    /// Merge another state (from a replica or backup). Returns true if changed.
    pub fn merge_state(&mut self, other: &VaultState) -> Result<bool> {
        other.validate()?;
        Ok(self.state.merge_from(other))
    }

    // ----- Master password -----

    pub fn verify_password(&self, password: &str) -> bool {
        self.header.password_matches(&self.key, password)
    }

    pub fn change_password(&mut self, current: &str, new: &str, cost: KdfCost) -> Result<()> {
        if !self.verify_password(current) {
            return Err(Error::WrongPassword);
        }
        self.header = self.header.rewrap(&self.key, new, cost)?;
        Ok(())
    }

    /// Adopt the canonical header stored in the cloud (e.g. after a master
    /// password change on another device). The header must be for this vault
    /// and authenticated by the vault key. It is adopted when it is newer, or
    /// when it has the same revision but differs (two devices changed the
    /// password concurrently: the cloud copy wins). Returns true if adopted.
    pub fn adopt_header(&mut self, header: &VaultHeader) -> Result<bool> {
        if header.vault_id != self.header.vault_id {
            return Err(Error::VaultMismatch);
        }
        match header.key_epoch.cmp(&self.header.key_epoch) {
            std::cmp::Ordering::Greater => return Err(Error::KeyRotated),
            std::cmp::Ordering::Less => {
                // A header from before our key rotation: stale.
                header.verify(self.key_for(header.key_epoch)?)?;
                return Ok(false);
            }
            std::cmp::Ordering::Equal => {}
        }
        if let Err(e) = header.verify(&self.key) {
            // Two devices rotated concurrently: the cloud copy wins, which
            // needs the master password.
            if header.key_epoch >= 2 && header.revision >= self.header.revision {
                return Err(Error::KeyRotated);
            }
            return Err(e);
        }
        if header.revision > self.header.revision || (header.revision == self.header.revision && header != &self.header)
        {
            self.header = header.clone();
            return Ok(true);
        }
        Ok(false)
    }

    /// Merge this device's local vault file as saved by another process that
    /// shares the data directory. A newer header in it (a master-password
    /// change made by that process) is adopted. Returns true if anything changed.
    pub fn absorb_local(&mut self, file: &LocalVaultFile) -> Result<bool> {
        if file.header.vault_id != self.vault_id() || file.device_id != self.device_id {
            return Err(Error::VaultMismatch);
        }
        let key = self.key_for(file.header.key_epoch)?;
        let state: VaultState = open_json(key, &local_ctx(self.vault_id(), self.device_id), &file.payload)?;
        let header_changed = self.adopt_header(&file.header)?;
        Ok(self.merge_state(&state)? || header_changed)
    }

    // ----- Vault-key rotation -----

    /// Replace the vault key with a fresh one. Everything this device writes
    /// from now on is encrypted with it; other devices adopt it with the
    /// master password ([`Vault::adopt_rotated_header`]).
    pub fn rotate_key(&mut self, password: &str, cost: KdfCost) -> Result<()> {
        if !self.verify_password(password) {
            return Err(Error::WrongPassword);
        }
        let (header, key) = self.header.rotate(&self.key, password, cost)?;
        self.previous = header.previous_keys(&key)?;
        self.key = key;
        self.header = header;
        Ok(())
    }

    /// Adopt a header whose vault key was rotated on another device, using
    /// the master password. The header's list of earlier keys must include a
    /// key this device holds, so it really continues this vault.
    pub fn adopt_rotated_header(&mut self, header: &VaultHeader, password: &str) -> Result<()> {
        if header.vault_id != self.vault_id() {
            return Err(Error::VaultMismatch);
        }
        if header.key_epoch < self.header.key_epoch || header == &self.header {
            return Err(Error::InvalidInput(
                "this device already uses the newest vault key".into(),
            ));
        }
        let key = header.unlock(password)?;
        let previous = header.previous_keys(&key)?;
        let known = std::iter::once(&self.key).chain(self.previous.iter().map(|(_, k)| k));
        let continues = known
            .into_iter()
            .any(|mine| previous.iter().any(|(_, k)| k.as_bytes() == mine.as_bytes()));
        if !continues {
            return Err(Error::VaultMismatch);
        }
        self.key = key;
        self.previous = previous;
        self.header = header.clone();
        Ok(())
    }

    // ----- Sync payloads -----

    pub fn seal_replica(&self) -> Result<ReplicaFile> {
        Ok(ReplicaFile {
            format: REPLICA_FORMAT.into(),
            version: FILE_VERSION,
            vault_id: self.vault_id(),
            device_id: self.device_id,
            key_epoch: self.header.key_epoch,
            written_at: now_millis(),
            payload: seal_json(&self.key, &replica_ctx(self.vault_id(), self.device_id), &self.state)?,
        })
    }

    /// Decrypt and authenticate a replica written by any device of this vault.
    pub fn open_replica(&self, file: &ReplicaFile) -> Result<VaultState> {
        if file.vault_id != self.vault_id() {
            return Err(Error::VaultMismatch);
        }
        let key = self.key_for(file.key_epoch)?;
        let state: VaultState = open_json(key, &replica_ctx(file.vault_id, file.device_id), &file.payload)?;
        state.validate()?;
        Ok(state)
    }

    // ----- Backups -----

    /// Export the full vault (including history and tombstones) as a backup
    /// protected by `password` with fresh key material.
    pub fn export_backup(&self, password: &str, cost: KdfCost) -> Result<BackupFile> {
        let (header, key) = VaultHeader::create(password, cost)?;
        let payload = seal_json(&key, &backup_ctx(header.vault_id), &self.state)?;
        Ok(BackupFile {
            format: BACKUP_FORMAT.into(),
            version: FILE_VERSION,
            created_at: now_millis(),
            source_vault_id: self.vault_id(),
            entry_count: self.entries().count(),
            header,
            payload,
        })
    }

    /// Open a previous local vault file (from the local backup folder) of this
    /// same vault, using the password that was valid when it was written.
    pub fn open_local_backup(&self, file: &LocalVaultFile, password: &str) -> Result<VaultState> {
        if file.header.vault_id != self.vault_id() {
            return Err(Error::VaultMismatch);
        }
        Ok(Vault::open(file, password)?.state.clone())
    }

    // ----- Device-local secrets -----

    pub fn seal_secrets(&self, secrets: &LocalSecrets) -> Result<SecretsFile> {
        Ok(SecretsFile {
            format: SECRETS_FORMAT.into(),
            version: FILE_VERSION,
            vault_id: self.vault_id(),
            device_id: self.device_id,
            payload: seal_json(&self.key, &secrets_ctx(self.vault_id(), self.device_id), secrets)?,
        })
    }

    pub fn open_secrets(&self, file: &SecretsFile) -> Result<LocalSecrets> {
        if file.vault_id != self.vault_id() || file.device_id != self.device_id {
            return Err(Error::VaultMismatch);
        }
        // Sealed with the current key, or an earlier one before a rotation.
        let ctx = secrets_ctx(file.vault_id, file.device_id);
        let mut result = open_json(&self.key, &ctx, &file.payload);
        for (_, key) in &self.previous {
            if result.is_ok() {
                break;
            }
            result = open_json(key, &ctx, &file.payload);
        }
        result
    }
}

impl BackupFile {
    /// Decrypt a backup with its password. The result is merged into a vault
    /// with [`Vault::merge_state`], which never discards existing data.
    pub fn open(&self, password: &str) -> Result<VaultState> {
        let key = self.header.unlock(password)?;
        let state: VaultState = open_json(&key, &backup_ctx(self.header.vault_id), &self.payload)?;
        state.validate()?;
        Ok(state)
    }
}

fn local_ctx(vault_id: Uuid, device_id: Uuid) -> PayloadContext {
    PayloadContext {
        kind: PayloadKind::Local,
        vault_id,
        device_id: Some(device_id),
    }
}
fn replica_ctx(vault_id: Uuid, device_id: Uuid) -> PayloadContext {
    PayloadContext {
        kind: PayloadKind::Replica,
        vault_id,
        device_id: Some(device_id),
    }
}
fn secrets_ctx(vault_id: Uuid, device_id: Uuid) -> PayloadContext {
    PayloadContext {
        kind: PayloadKind::LocalSecrets,
        vault_id,
        device_id: Some(device_id),
    }
}
fn backup_ctx(backup_id: Uuid) -> PayloadContext {
    PayloadContext {
        kind: PayloadKind::Backup,
        vault_id: backup_id,
        device_id: None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const PW: &str = "synthetic-master-pw-1";

    fn entry(title: &str, pw: &str) -> EntryData {
        EntryData {
            title: title.into(),
            username: "alice@example.test".into(),
            password: pw.into(),
            urls: vec!["https://example.test/login".into()],
            ..Default::default()
        }
    }

    #[test]
    fn local_file_roundtrip() {
        let mut v = Vault::create(PW, KdfCost::MINIMUM).unwrap();
        let id = v.add_entry(entry("Example", "synthetic-1")).unwrap();
        let file = v.to_local_file().unwrap();
        let bytes = file.to_bytes().unwrap();
        // The secret never appears in the serialised file.
        assert!(!String::from_utf8_lossy(&bytes).contains("synthetic-1"));
        assert!(!String::from_utf8_lossy(&bytes).contains("alice@example.test"));

        let reopened = Vault::open(&LocalVaultFile::from_bytes(&bytes).unwrap(), PW).unwrap();
        assert_eq!(reopened.device_id(), v.device_id());
        assert_eq!(
            reopened.get(id).unwrap().current().data.as_ref().unwrap().password,
            "synthetic-1"
        );
        assert!(matches!(
            Vault::open(&file, "wrong-password-xyz"),
            Err(Error::WrongPassword)
        ));
    }

    #[test]
    fn local_file_tampering_detected() {
        let v = Vault::create(PW, KdfCost::MINIMUM).unwrap();
        let mut file = v.to_local_file().unwrap();
        file.device_id = Uuid::new_v4();
        assert!(matches!(Vault::open(&file, PW), Err(Error::Authentication)));
    }

    #[test]
    fn crud_and_restore() {
        let mut v = Vault::create(PW, KdfCost::MINIMUM).unwrap();
        assert!(v.add_entry(entry("  ", "x")).is_err());
        let id = v.add_entry(entry("Example", "synthetic-1")).unwrap();
        v.update_entry(id, entry("Example", "synthetic-2")).unwrap();
        assert_eq!(v.get(id).unwrap().history.len(), 1);
        v.delete_entry(id).unwrap();
        assert_eq!(v.entries().count(), 0);
        assert_eq!(v.deleted_entries().count(), 1);
        v.restore_entry(id).unwrap();
        assert_eq!(
            v.get(id).unwrap().current().data.as_ref().unwrap().password,
            "synthetic-2"
        );
        assert_eq!(v.search("EXAMPLE.test").len(), 1);
        assert_eq!(v.search("nomatch").len(), 0);
        assert_eq!(v.resolve_id(&id.to_string()[..8]).unwrap(), id);
        assert!(v.update_entry(Uuid::new_v4(), entry("x", "y")).is_err());
    }

    #[test]
    fn purge_deleted_entries_across_devices() {
        let mut a = Vault::create(PW, KdfCost::MINIMUM).unwrap();
        let keep = a.add_entry(entry("Keep", "synthetic-keep")).unwrap();
        let gone = a.add_entry(entry("Gone", "synthetic-gone-1")).unwrap();
        a.update_entry(gone, entry("Gone", "synthetic-gone-2")).unwrap();
        let mut b = Vault::join(a.header().clone(), PW).unwrap();
        b.merge_state(&b.open_replica(&a.seal_replica().unwrap()).unwrap())
            .unwrap();

        assert!(
            matches!(a.purge_entry(gone), Err(Error::InvalidInput(_))),
            "live entries cannot be purged"
        );
        a.delete_entry(gone).unwrap();
        a.purge_entry(gone).unwrap();
        assert!(a.get(gone).is_none());
        assert!(a.restore_entry(gone).is_err());
        let replica = a.seal_replica().unwrap();
        assert!(!String::from_utf8_lossy(&replica.to_bytes().unwrap()).contains("synthetic-gone"));

        // b still had the entry with its history; the purge removes it there,
        // and b's stale replica does not bring it back to a.
        let stale_b = b.seal_replica().unwrap();
        b.merge_state(&b.open_replica(&replica).unwrap()).unwrap();
        assert!(b.get(gone).is_none());
        a.merge_state(&a.open_replica(&stale_b).unwrap()).unwrap();
        assert!(a.get(gone).is_none());
        assert!(a.get(keep).is_some());

        b.delete_entry(keep).unwrap();
        assert_eq!(b.purge_deleted().unwrap(), 1);
        assert_eq!(b.entries().count() + b.deleted_entries().count(), 0);
    }

    #[test]
    fn key_rotation_across_devices() {
        let mut a = Vault::create(PW, KdfCost::MINIMUM).unwrap();
        let id = a.add_entry(entry("Example", "synthetic-1")).unwrap();
        let mut b = Vault::join(a.header().clone(), PW).unwrap();
        b.merge_state(&b.open_replica(&a.seal_replica().unwrap()).unwrap())
            .unwrap();
        let old_key_holder = Vault::join(a.header().clone(), PW).unwrap();

        assert!(matches!(
            a.rotate_key("wrong-password-xyz", KdfCost::MINIMUM),
            Err(Error::WrongPassword)
        ));
        a.rotate_key(PW, KdfCost::MINIMUM).unwrap();
        assert_eq!(a.key_epoch(), 2);
        a.update_entry(id, entry("Example", "after-rotation")).unwrap();
        let new_replica = a.seal_replica().unwrap();
        assert_eq!(new_replica.key_epoch, 2);
        // Someone holding the old key cannot read what is written now.
        assert!(old_key_holder.open_replica(&new_replica).is_err());

        // b, still on the old key, keeps working offline; its replica stays
        // readable for a.
        b.update_entry(id, entry("Example", "edit-on-b")).unwrap();
        let lagging = b.seal_replica().unwrap();
        assert_eq!(lagging.key_epoch, 1);
        a.merge_state(&a.open_replica(&lagging).unwrap()).unwrap();
        assert_eq!(a.conflicts().count(), 1);

        // b sees the rotated header: it needs the master password.
        assert!(matches!(b.adopt_header(a.header()), Err(Error::KeyRotated)));
        assert!(matches!(b.open_replica(&new_replica), Err(Error::KeyRotated)));
        assert!(matches!(
            b.adopt_rotated_header(a.header(), "wrong-password-xyz"),
            Err(Error::WrongPassword)
        ));
        b.adopt_rotated_header(a.header(), PW).unwrap();
        assert_eq!(b.key_epoch(), 2);
        b.merge_state(&b.open_replica(&new_replica).unwrap()).unwrap();
        assert_eq!(a.seal_replica().unwrap().key_epoch, b.seal_replica().unwrap().key_epoch);
        // a sees b's old header as stale.
        assert!(!a.adopt_header(old_key_holder.header()).unwrap());

        // Local files: reopen with the password, and old-epoch local files
        // (written by another process before the rotation) still merge.
        let reopened = Vault::open(&b.to_local_file().unwrap(), PW).unwrap();
        assert_eq!(reopened.key_epoch(), 2);
        let mut b2 = Vault::open(&b.to_local_file().unwrap(), PW).unwrap();
        let stale = {
            let mut f = b.to_local_file().unwrap();
            f.header = old_key_holder.header().clone();
            f.payload = seal_json(&old_key_holder.key, &local_ctx(b.vault_id(), b.device_id()), b.state()).unwrap();
            f
        };
        b2.absorb_local(&stale).unwrap();
        assert_eq!(b2.key_epoch(), 2);

        // Secrets sealed before the rotation stay readable.
        let s = LocalSecrets {
            google_refresh_token: Some("synthetic-refresh".into()),
        };
        let sealed_old = old_key_holder.seal_secrets(&s).unwrap();
        let mut c = Vault::join(old_key_holder.header().clone(), PW).unwrap();
        c.device_id = old_key_holder.device_id;
        c.adopt_rotated_header(a.header(), PW).unwrap();
        assert_eq!(c.open_secrets(&sealed_old).unwrap(), s);

        // A rotated header of another vault, or one that does not continue
        // this vault's keys, is refused.
        let mut other = Vault::create(PW, KdfCost::MINIMUM).unwrap();
        other.rotate_key(PW, KdfCost::MINIMUM).unwrap();
        assert!(matches!(
            b.adopt_rotated_header(other.header(), PW),
            Err(Error::VaultMismatch)
        ));
        assert!(b.adopt_rotated_header(a.header(), PW).is_err(), "already adopted");
    }

    #[test]
    fn concurrent_rotations_cloud_copy_wins_with_password() {
        let mut a = Vault::create(PW, KdfCost::MINIMUM).unwrap();
        let mut b = Vault::join(a.header().clone(), PW).unwrap();
        a.rotate_key(PW, KdfCost::MINIMUM).unwrap();
        b.rotate_key(PW, KdfCost::MINIMUM).unwrap();
        // a's header reached Drive first.
        assert!(matches!(b.adopt_header(a.header()), Err(Error::KeyRotated)));
        b.adopt_rotated_header(a.header(), PW).unwrap();
        assert!(!b.adopt_header(a.header()).unwrap());
        assert!(a.open_replica(&b.seal_replica().unwrap()).is_ok());
    }

    #[test]
    fn replica_exchange_and_conflict_resolution() {
        let mut a = Vault::create(PW, KdfCost::MINIMUM).unwrap();
        let id = a.add_entry(entry("Example", "base")).unwrap();
        let mut b = Vault::join(a.header().clone(), PW).unwrap();
        assert_ne!(a.device_id(), b.device_id());
        b.merge_state(&b.open_replica(&a.seal_replica().unwrap()).unwrap())
            .unwrap();

        a.update_entry(id, entry("Example", "edit-on-a")).unwrap();
        b.update_entry(id, entry("Example", "edit-on-b")).unwrap();
        let ra = a.seal_replica().unwrap();
        let rb = b.seal_replica().unwrap();
        a.merge_state(&a.open_replica(&rb).unwrap()).unwrap();
        b.merge_state(&b.open_replica(&ra).unwrap()).unwrap();
        assert_eq!(a.state(), b.state());
        assert_eq!(a.conflicts().count(), 1);

        let rec = a.get(id).unwrap();
        let idx = rec
            .versions
            .iter()
            .position(|v| v.data.as_ref().unwrap().password == "edit-on-b")
            .unwrap();
        a.resolve_conflict(id, idx).unwrap();
        assert_eq!(a.conflicts().count(), 0);
        b.merge_state(&b.open_replica(&a.seal_replica().unwrap()).unwrap())
            .unwrap();
        assert_eq!(b.conflicts().count(), 0);
        assert_eq!(
            b.get(id).unwrap().current().data.as_ref().unwrap().password,
            "edit-on-b"
        );
    }

    #[test]
    fn replica_from_other_vault_or_device_spoof_rejected() {
        let a = Vault::create(PW, KdfCost::MINIMUM).unwrap();
        let other = Vault::create(PW, KdfCost::MINIMUM).unwrap();
        let r = other.seal_replica().unwrap();
        assert!(matches!(a.open_replica(&r), Err(Error::VaultMismatch)));

        let mut spoofed = a.seal_replica().unwrap();
        spoofed.device_id = Uuid::new_v4();
        assert!(matches!(a.open_replica(&spoofed), Err(Error::Authentication)));
    }

    #[test]
    fn password_change_and_header_adoption() {
        let mut a = Vault::create(PW, KdfCost::MINIMUM).unwrap();
        let mut b = Vault::join(a.header().clone(), PW).unwrap();
        assert!(a
            .change_password("wrong-password-xyz", "new-synthetic-pw", KdfCost::MINIMUM)
            .is_err());
        a.change_password(PW, "new-synthetic-pw", KdfCost::MINIMUM).unwrap();
        assert!(b.adopt_header(a.header()).unwrap());
        assert!(!b.adopt_header(a.header()).unwrap());
        assert!(b.verify_password("new-synthetic-pw"));

        // A header for a different vault, or forged without the vault key, is refused.
        let other = Vault::create(PW, KdfCost::MINIMUM).unwrap();
        assert!(matches!(b.adopt_header(other.header()), Err(Error::VaultMismatch)));
        let mut forged = a.header().clone();
        forged.revision = 50;
        assert!(b.adopt_header(&forged).is_err());
    }

    #[test]
    fn backup_export_and_merge_restore() {
        let mut v = Vault::create(PW, KdfCost::MINIMUM).unwrap();
        let id = v.add_entry(entry("Example", "synthetic-1")).unwrap();
        let backup = v.export_backup("backup-synthetic-pw", KdfCost::MINIMUM).unwrap();
        let bytes = backup.to_bytes().unwrap();
        assert!(!String::from_utf8_lossy(&bytes).contains("synthetic-1"));

        v.update_entry(id, entry("Example", "synthetic-2")).unwrap();
        let restored = BackupFile::from_bytes(&bytes)
            .unwrap()
            .open("backup-synthetic-pw")
            .unwrap();
        assert!(BackupFile::from_bytes(&bytes).unwrap().open(PW).is_err());
        // Restoring an older backup never rolls back newer data.
        v.merge_state(&restored).unwrap();
        assert_eq!(
            v.get(id).unwrap().current().data.as_ref().unwrap().password,
            "synthetic-2"
        );

        // Restoring into a fresh vault brings the entries back.
        let mut fresh = Vault::create(PW, KdfCost::MINIMUM).unwrap();
        assert!(fresh.merge_state(&restored).unwrap());
        assert_eq!(fresh.entries().count(), 1);
    }

    #[test]
    fn secrets_roundtrip_bound_to_device() {
        let v = Vault::create(PW, KdfCost::MINIMUM).unwrap();
        let s = LocalSecrets {
            google_refresh_token: Some("synthetic-refresh-token".into()),
        };
        let file = v.seal_secrets(&s).unwrap();
        assert!(!String::from_utf8_lossy(&file.to_bytes().unwrap()).contains("synthetic-refresh"));
        assert_eq!(v.open_secrets(&file).unwrap(), s);
        let other_device = Vault::join(v.header().clone(), PW).unwrap();
        assert!(other_device.open_secrets(&file).is_err());
        assert!(!format!("{s:?}").contains("synthetic-refresh"));
    }

    #[test]
    fn file_format_checks() {
        let v = Vault::create(PW, KdfCost::MINIMUM).unwrap();
        let replica = v.seal_replica().unwrap().to_bytes().unwrap();
        assert!(LocalVaultFile::from_bytes(&replica).is_err());
        assert_eq!(detect_format(&replica).unwrap(), REPLICA_FORMAT);
        let mut r = v.seal_replica().unwrap();
        r.version = 99;
        assert!(matches!(
            ReplicaFile::from_bytes(&r.to_bytes().unwrap()),
            Err(Error::UnsupportedVersion { .. })
        ));
    }
}
