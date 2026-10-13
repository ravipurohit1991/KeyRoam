//! The minimal storage interface the sync engine needs from Google Drive's
//! `appDataFolder`. [`crate::drive::DriveClient`] is the only production
//! implementation.

use crate::error::Result;

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct RemoteFile {
    pub id: String,
    pub name: String,
    /// Drive's monotonically increasing file version (changes on every write).
    pub version: String,
    /// RFC 3339 creation time, used for deterministic tie-breaking.
    pub created_time: String,
}

pub trait RemoteStore {
    fn list(&self) -> Result<Vec<RemoteFile>>;
    fn download(&self, id: &str) -> Result<Vec<u8>>;
    fn create(&self, name: &str, bytes: &[u8]) -> Result<RemoteFile>;
    fn update(&self, id: &str, bytes: &[u8]) -> Result<RemoteFile>;
}

impl<T: RemoteStore + ?Sized> RemoteStore for &T {
    fn list(&self) -> Result<Vec<RemoteFile>> {
        (**self).list()
    }
    fn download(&self, id: &str) -> Result<Vec<u8>> {
        (**self).download(id)
    }
    fn create(&self, name: &str, bytes: &[u8]) -> Result<RemoteFile> {
        (**self).create(name, bytes)
    }
    fn update(&self, id: &str, bytes: &[u8]) -> Result<RemoteFile> {
        (**self).update(id, bytes)
    }
}
