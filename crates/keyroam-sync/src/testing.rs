//! Test support: an in-memory `appDataFolder`. Only compiled for tests or
//! with the `test-util` feature; it is not a sync provider.

use std::sync::Mutex;

use crate::error::{Result, SyncError};
use crate::remote::{RemoteFile, RemoteStore};

/// In-memory `appDataFolder` used to drive the protocol through edge cases
/// (offline, corruption, copied profiles) quickly.
#[derive(Default)]
pub struct MemoryRemote {
    inner: Mutex<MemInner>,
}

#[derive(Default)]
pub struct MemInner {
    files: Vec<(RemoteFile, Vec<u8>)>,
    next: u64,
    offline: bool,
    downloads: usize,
}

impl MemoryRemote {
    pub fn set_offline(&self, v: bool) {
        self.inner.lock().unwrap().offline = v;
    }
    pub fn downloads(&self) -> usize {
        self.inner.lock().unwrap().downloads
    }
    pub fn names(&self) -> Vec<String> {
        self.inner
            .lock()
            .unwrap()
            .files
            .iter()
            .map(|(f, _)| f.name.clone())
            .collect()
    }
    pub fn tamper(&self, name_prefix: &str, f: impl Fn(&mut Vec<u8>)) {
        let mut g = self.inner.lock().unwrap();
        for (meta, content) in g.files.iter_mut() {
            if meta.name.starts_with(name_prefix) {
                f(content);
                meta.version = (meta.version.parse::<u64>().unwrap() + 1).to_string();
            }
        }
    }
    pub fn put_raw(&self, name: &str, bytes: Vec<u8>) {
        self.create(name, &bytes).unwrap();
    }
    fn check(&self) -> Result<std::sync::MutexGuard<'_, MemInner>> {
        let g = self.inner.lock().unwrap();
        if g.offline {
            return Err(SyncError::Network("simulated offline".into()));
        }
        Ok(g)
    }
}

impl RemoteStore for MemoryRemote {
    fn list(&self) -> Result<Vec<RemoteFile>> {
        Ok(self.check()?.files.iter().map(|(f, _)| f.clone()).collect())
    }
    fn download(&self, id: &str) -> Result<Vec<u8>> {
        let mut g = self.check()?;
        g.downloads += 1;
        g.files
            .iter()
            .find(|(f, _)| f.id == id)
            .map(|(_, c)| c.clone())
            .ok_or(SyncError::Http {
                status: 404,
                message: "not found".into(),
            })
    }
    fn create(&self, name: &str, bytes: &[u8]) -> Result<RemoteFile> {
        let mut g = self.check()?;
        g.next += 1;
        let f = RemoteFile {
            id: format!("id{}", g.next),
            name: name.into(),
            version: "1".into(),
            created_time: format!("t{:05}", g.next),
        };
        g.files.push((f.clone(), bytes.to_vec()));
        Ok(f)
    }
    fn update(&self, id: &str, bytes: &[u8]) -> Result<RemoteFile> {
        let mut g = self.check()?;
        let (f, c) = g.files.iter_mut().find(|(f, _)| f.id == id).unwrap();
        *c = bytes.to_vec();
        f.version = (f.version.parse::<u64>().unwrap() + 1).to_string();
        Ok(f.clone())
    }
}
