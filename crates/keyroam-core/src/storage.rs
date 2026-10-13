//! Device-local persistence: the encrypted local vault, device secrets,
//! non-secret sync metadata and rotating local backups.
//!
//! Writes are atomic (temp file + rename) so a crash never leaves a truncated
//! vault behind.

use std::fs;
use std::io::Write;
use std::path::{Path, PathBuf};

use serde::de::DeserializeOwned;
use serde::Serialize;

use crate::crypto::random_bytes;
use crate::error::{Error, Result};
use crate::files::{LocalVaultFile, SecretsFile};
use crate::now_millis;

pub const VAULT_FILE: &str = "vault.krv";
pub const SECRETS_FILE: &str = "secrets.krs";
pub const BACKUP_DIR: &str = "backups";
pub const LOCAL_BACKUP_LIMIT: usize = 10;

#[derive(Clone, Debug)]
pub struct LocalStore {
    root: PathBuf,
}

impl LocalStore {
    pub fn new(root: impl Into<PathBuf>) -> Self {
        LocalStore { root: root.into() }
    }

    pub fn root(&self) -> &Path {
        &self.root
    }

    fn path(&self, name: &str) -> PathBuf {
        self.root.join(name)
    }

    pub fn has_vault(&self) -> bool {
        self.path(VAULT_FILE).is_file()
    }

    pub fn load_vault(&self) -> Result<LocalVaultFile> {
        match fs::read(self.path(VAULT_FILE)) {
            Ok(bytes) => LocalVaultFile::from_bytes(&bytes),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Err(Error::NoVault),
            Err(e) => Err(e.into()),
        }
    }

    /// Persist a newly created/joined vault. Refuses to replace an existing one.
    pub fn save_new_vault(&self, file: &LocalVaultFile) -> Result<()> {
        if self.has_vault() {
            return Err(Error::VaultExists);
        }
        self.write_atomic(VAULT_FILE, &file.to_bytes()?)
    }

    /// Persist the vault. With `backup_previous`, the current file is first
    /// copied into the rotating backup folder.
    pub fn save_vault(&self, file: &LocalVaultFile, backup_previous: bool) -> Result<()> {
        if backup_previous && self.has_vault() {
            self.backup_current()?;
        }
        self.write_atomic(VAULT_FILE, &file.to_bytes()?)
    }

    fn backup_current(&self) -> Result<()> {
        let dir = self.path(BACKUP_DIR);
        fs::create_dir_all(&dir)?;
        let suffix: [u8; 3] = random_bytes();
        let name = format!(
            "vault-{:013}-{:02x}{:02x}{:02x}.krv",
            now_millis(),
            suffix[0],
            suffix[1],
            suffix[2]
        );
        fs::copy(self.path(VAULT_FILE), dir.join(name))?;
        for old in self.local_backups()?.into_iter().skip(LOCAL_BACKUP_LIMIT) {
            let _ = fs::remove_file(old);
        }
        Ok(())
    }

    /// Local backup files, newest first.
    pub fn local_backups(&self) -> Result<Vec<PathBuf>> {
        let dir = self.path(BACKUP_DIR);
        if !dir.is_dir() {
            return Ok(Vec::new());
        }
        let mut files: Vec<PathBuf> = fs::read_dir(dir)?
            .filter_map(|e| e.ok().map(|e| e.path()))
            .filter(|p| p.extension().is_some_and(|x| x == "krv"))
            .collect();
        files.sort();
        files.reverse();
        Ok(files)
    }

    pub fn load_secrets(&self) -> Result<Option<SecretsFile>> {
        match fs::read(self.path(SECRETS_FILE)) {
            Ok(bytes) => Ok(Some(SecretsFile::from_bytes(&bytes)?)),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(None),
            Err(e) => Err(e.into()),
        }
    }

    pub fn save_secrets(&self, file: &SecretsFile) -> Result<()> {
        self.write_atomic(SECRETS_FILE, &file.to_bytes()?)
    }

    pub fn delete_secrets(&self) -> Result<()> {
        match fs::remove_file(self.path(SECRETS_FILE)) {
            Err(e) if e.kind() != std::io::ErrorKind::NotFound => Err(e.into()),
            _ => Ok(()),
        }
    }

    /// Read a non-secret JSON metadata file.
    pub fn read_json<T: DeserializeOwned>(&self, name: &str) -> Result<Option<T>> {
        match fs::read(self.path(name)) {
            Ok(bytes) => Ok(Some(serde_json::from_slice(&bytes)?)),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(None),
            Err(e) => Err(e.into()),
        }
    }

    pub fn write_json<T: Serialize>(&self, name: &str, value: &T) -> Result<()> {
        self.write_atomic(name, &serde_json::to_vec_pretty(value)?)
    }

    pub fn write_atomic(&self, name: &str, bytes: &[u8]) -> Result<()> {
        fs::create_dir_all(&self.root)?;
        let target = self.path(name);
        let suffix: [u8; 4] = random_bytes();
        let tmp = self.path(&format!(".{name}.{}.tmp", u32::from_be_bytes(suffix)));
        {
            let mut opts = fs::OpenOptions::new();
            opts.write(true).create_new(true);
            #[cfg(unix)]
            {
                use std::os::unix::fs::OpenOptionsExt;
                opts.mode(0o600);
            }
            let mut f = opts.open(&tmp)?;
            f.write_all(bytes)?;
            f.sync_all()?;
        }
        if let Err(e) = fs::rename(&tmp, &target) {
            let _ = fs::remove_file(&tmp);
            return Err(e.into());
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::kdf::KdfCost;
    use crate::model::EntryData;
    use crate::vault::{LocalSecrets, Vault};

    const PW: &str = "synthetic-master-pw-1";

    #[test]
    fn save_load_and_rotate_backups() {
        let dir = tempfile::tempdir().unwrap();
        let store = LocalStore::new(dir.path().join("data"));
        assert!(matches!(store.load_vault(), Err(Error::NoVault)));

        let mut v = Vault::create(PW, KdfCost::MINIMUM).unwrap();
        store.save_new_vault(&v.to_local_file().unwrap()).unwrap();
        assert!(matches!(
            store.save_new_vault(&v.to_local_file().unwrap()),
            Err(Error::VaultExists)
        ));

        for i in 0..(LOCAL_BACKUP_LIMIT + 3) {
            v.add_entry(EntryData {
                title: format!("e{i}"),
                ..Default::default()
            })
            .unwrap();
            store.save_vault(&v.to_local_file().unwrap(), true).unwrap();
        }
        assert_eq!(store.local_backups().unwrap().len(), LOCAL_BACKUP_LIMIT);

        let loaded = Vault::open(&store.load_vault().unwrap(), PW).unwrap();
        assert_eq!(loaded.entries().count(), LOCAL_BACKUP_LIMIT + 3);

        // A local backup can be opened and merged.
        let newest = &store.local_backups().unwrap()[0];
        let backup = LocalVaultFile::from_bytes(&std::fs::read(newest).unwrap()).unwrap();
        let state = loaded.open_local_backup(&backup, PW).unwrap();
        assert_eq!(state.entries.len(), LOCAL_BACKUP_LIMIT + 2);

        // No stray temp files.
        let stray = std::fs::read_dir(store.root())
            .unwrap()
            .filter(|e| e.as_ref().unwrap().file_name().to_string_lossy().ends_with(".tmp"))
            .count();
        assert_eq!(stray, 0);
    }

    #[test]
    fn secrets_persistence() {
        let dir = tempfile::tempdir().unwrap();
        let store = LocalStore::new(dir.path());
        let v = Vault::create(PW, KdfCost::MINIMUM).unwrap();
        assert!(store.load_secrets().unwrap().is_none());
        let s = LocalSecrets {
            google_refresh_token: Some("synthetic".into()),
        };
        store.save_secrets(&v.seal_secrets(&s).unwrap()).unwrap();
        assert_eq!(v.open_secrets(&store.load_secrets().unwrap().unwrap()).unwrap(), s);
        store.delete_secrets().unwrap();
        store.delete_secrets().unwrap();
        assert!(store.load_secrets().unwrap().is_none());
    }
}
