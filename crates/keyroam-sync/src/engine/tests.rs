use std::sync::Mutex;

use keyroam_core::{EntryData, KdfCost, LocalSecrets, LocalStore, Vault};
use tempfile::TempDir;

use super::*;
use crate::drive::fake;

const PW: &str = "synthetic-master-pw-1";

/// In-memory `appDataFolder` used to drive the protocol through edge cases
/// (offline, corruption, copied profiles) quickly.
#[derive(Default)]
struct MemoryRemote {
    inner: Mutex<MemInner>,
}

#[derive(Default)]
struct MemInner {
    files: Vec<(RemoteFile, Vec<u8>)>,
    next: u64,
    offline: bool,
    downloads: usize,
}

impl MemoryRemote {
    fn set_offline(&self, v: bool) {
        self.inner.lock().unwrap().offline = v;
    }
    fn downloads(&self) -> usize {
        self.inner.lock().unwrap().downloads
    }
    fn names(&self) -> Vec<String> {
        self.inner
            .lock()
            .unwrap()
            .files
            .iter()
            .map(|(f, _)| f.name.clone())
            .collect()
    }
    fn tamper(&self, name_prefix: &str, f: impl Fn(&mut Vec<u8>)) {
        let mut g = self.inner.lock().unwrap();
        for (meta, content) in g.files.iter_mut() {
            if meta.name.starts_with(name_prefix) {
                f(content);
                meta.version = (meta.version.parse::<u64>().unwrap() + 1).to_string();
            }
        }
    }
    fn put_raw(&self, name: &str, bytes: Vec<u8>) {
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

struct Device {
    _dir: TempDir,
    store: LocalStore,
    vault: Vault,
}

impl Device {
    fn add(&mut self, title: &str, pw: &str) -> keyroam_core::EntryId {
        let id = self.vault.add_entry(entry(title, pw)).unwrap();
        commit_local_change(&self.vault, &self.store).unwrap();
        id
    }
    fn edit(&mut self, id: keyroam_core::EntryId, pw: &str) {
        let title = self
            .vault
            .get(id)
            .unwrap()
            .current()
            .data
            .as_ref()
            .unwrap()
            .title
            .clone();
        self.vault.update_entry(id, entry(&title, pw)).unwrap();
        commit_local_change(&self.vault, &self.store).unwrap();
    }
    fn sync(&mut self, remote: &dyn RemoteStore) -> Result<SyncReport> {
        sync(&mut self.vault, &self.store, remote)
    }
    fn password_of(&self, id: keyroam_core::EntryId) -> String {
        self.vault
            .get(id)
            .unwrap()
            .current()
            .data
            .as_ref()
            .unwrap()
            .password
            .clone()
    }
    fn reopen(&self, pw: &str) -> keyroam_core::Result<Vault> {
        Vault::open(&self.store.load_vault().unwrap(), pw)
    }
}

fn entry(title: &str, pw: &str) -> EntryData {
    EntryData {
        title: title.into(),
        username: "user@example.test".into(),
        password: pw.into(),
        ..Default::default()
    }
}

fn secrets() -> LocalSecrets {
    LocalSecrets {
        google_refresh_token: Some("synthetic-refresh".into()),
    }
}

fn new_user(remote: &dyn RemoteStore) -> Device {
    let dir = tempfile::tempdir().unwrap();
    let store = LocalStore::new(dir.path());
    let (vault, report) = create_vault(&store, remote, PW, KdfCost::MINIMUM, &secrets()).unwrap();
    let report = report.unwrap();
    assert!(report.header_uploaded && report.uploaded);
    Device {
        _dir: dir,
        store,
        vault,
    }
}

fn join(remote: &dyn RemoteStore, pw: &str) -> Device {
    let dir = tempfile::tempdir().unwrap();
    let store = LocalStore::new(dir.path());
    let header = fetch_remote_header(remote).unwrap();
    let (vault, report) = join_vault(&store, remote, header, pw, &secrets()).unwrap();
    report.unwrap();
    Device {
        _dir: dir,
        store,
        vault,
    }
}

#[test]
fn onboarding_new_user_and_second_device() {
    let remote = MemoryRemote::default();
    let mut a = new_user(&remote);
    let id = a.add("Example", "synthetic-1");
    a.sync(&remote).unwrap();
    assert_eq!(remote.names().len(), 2);

    let b = join(&remote, PW);
    assert_eq!(b.password_of(id), "synthetic-1");
    assert_ne!(a.vault.device_id(), b.vault.device_id());
    // Joined device persisted its vault and secrets.
    let reopened = b.reopen(PW).unwrap();
    assert_eq!(reopened.entries().count(), 1);
    let secrets_file = b.store.load_secrets().unwrap().unwrap();
    assert_eq!(reopened.open_secrets(&secrets_file).unwrap(), secrets());
    assert_eq!(remote.names().len(), 3);
}

#[test]
fn onboarding_guards() {
    let remote = MemoryRemote::default();
    assert!(matches!(fetch_remote_header(&remote), Err(SyncError::NoRemoteVault)));
    let _a = new_user(&remote);

    // A second "new user" on the same Google account is refused.
    let dir = tempfile::tempdir().unwrap();
    let store = LocalStore::new(dir.path());
    assert!(matches!(
        create_vault(&store, &remote, PW, KdfCost::MINIMUM, &secrets()),
        Err(SyncError::RemoteVaultExists)
    ));
    assert!(!store.has_vault());

    // Wrong master password on join leaves nothing behind.
    let header = fetch_remote_header(&remote).unwrap();
    assert!(matches!(
        join_vault(&store, &remote, header, "wrong-password-xyz", &secrets()),
        Err(SyncError::Core(keyroam_core::Error::WrongPassword))
    ));
    assert!(!store.has_vault());
}

#[test]
fn offline_device_stays_usable_and_catches_up() {
    let remote = MemoryRemote::default();
    let mut a = new_user(&remote);
    let mut b = join(&remote, PW);

    remote.set_offline(true);
    let id = a.add("Offline entry", "synthetic-offline");
    let err = a.sync(&remote).unwrap_err();
    assert!(err.is_offline());
    let st = SyncState::load(&a.store).unwrap();
    assert!(matches!(st.status(), SyncStatus::Offline { .. }));
    assert!(st.pending_upload);
    // Local data survives a restart while offline.
    assert_eq!(a.reopen(PW).unwrap().entries().count(), 1);

    remote.set_offline(false);
    let r = a.sync(&remote).unwrap();
    assert!(r.uploaded);
    assert!(matches!(
        SyncState::load(&a.store).unwrap().status(),
        SyncStatus::UpToDate { .. }
    ));
    let r = b.sync(&remote).unwrap();
    assert!(r.local_changed);
    assert_eq!(b.password_of(id), "synthetic-offline");
}

#[test]
fn concurrent_edits_become_conflicts_not_lost_updates() {
    let remote = MemoryRemote::default();
    let mut a = new_user(&remote);
    let id = a.add("Shared", "base");
    let other = a.add("Other", "o1");
    a.sync(&remote).unwrap();
    let mut b = join(&remote, PW);

    a.edit(id, "from-a");
    b.edit(id, "from-b");
    b.edit(other, "o2"); // non-conflicting change on B
    a.sync(&remote).unwrap();
    b.sync(&remote).unwrap();
    a.sync(&remote).unwrap();

    assert_eq!(a.vault.state(), b.vault.state());
    let rec = a.vault.get(id).unwrap();
    assert!(rec.has_conflict());
    let mut values: Vec<_> = rec
        .versions
        .iter()
        .map(|v| v.data.as_ref().unwrap().password.clone())
        .collect();
    values.sort();
    assert_eq!(values, vec!["from-a", "from-b"]);
    assert_eq!(a.password_of(other), "o2");

    // Resolve on A, propagate to B.
    let idx = rec
        .versions
        .iter()
        .position(|v| v.data.as_ref().unwrap().password == "from-a")
        .unwrap();
    a.vault.resolve_conflict(id, idx).unwrap();
    commit_local_change(&a.vault, &a.store).unwrap();
    let r = a.sync(&remote).unwrap();
    assert_eq!(r.conflicts, 0);
    let r = b.sync(&remote).unwrap();
    assert_eq!(r.conflicts, 0);
    assert_eq!(b.password_of(id), "from-a");
    // The rejected value is still recoverable from history.
    assert!(b
        .vault
        .get(id)
        .unwrap()
        .history
        .iter()
        .any(|v| v.data.as_ref().unwrap().password == "from-b"));
}

#[test]
fn three_devices_converge() {
    let remote = MemoryRemote::default();
    let mut a = new_user(&remote);
    let mut b = join(&remote, PW);
    let mut c = join(&remote, PW);
    let ia = a.add("A", "a");
    let ib = b.add("B", "b");
    let ic = c.add("C", "c");
    for _ in 0..2 {
        a.sync(&remote).unwrap();
        b.sync(&remote).unwrap();
        c.sync(&remote).unwrap();
    }
    for d in [&a, &b, &c] {
        assert_eq!(d.vault.entries().count(), 3);
        assert_eq!(d.password_of(ia), "a");
        assert_eq!(d.password_of(ib), "b");
        assert_eq!(d.password_of(ic), "c");
    }
    assert_eq!(a.vault.state(), c.vault.state());
}

#[test]
fn unchanged_replicas_are_not_redownloaded() {
    let remote = MemoryRemote::default();
    let mut a = new_user(&remote);
    let mut b = join(&remote, PW);
    a.sync(&remote).unwrap();
    b.sync(&remote).unwrap();
    let before = remote.downloads();
    let r = b.sync(&remote).unwrap();
    // Only the header is fetched; no replica downloads, no upload.
    assert_eq!(remote.downloads() - before, 1);
    assert_eq!(r.replicas_merged, 0);
    assert!(!r.uploaded);
}

#[test]
fn password_change_propagates() {
    let remote = MemoryRemote::default();
    let mut a = new_user(&remote);
    let mut b = join(&remote, PW);
    a.vault
        .change_password(PW, "changed-synthetic-pw", KdfCost::MINIMUM)
        .unwrap();
    commit_local_change(&a.vault, &a.store).unwrap();
    assert!(a.sync(&remote).unwrap().header_uploaded);

    let r = b.sync(&remote).unwrap();
    assert!(r.header_adopted);
    assert!(b.reopen(PW).is_err());
    assert!(b.reopen("changed-synthetic-pw").is_ok());
    // A brand-new device needs the new password.
    let dir = tempfile::tempdir().unwrap();
    let header = fetch_remote_header(&remote).unwrap();
    assert!(join_vault(&LocalStore::new(dir.path()), &remote, header.clone(), PW, &secrets()).is_err());
    assert!(join_vault(
        &LocalStore::new(dir.path()),
        &remote,
        header,
        "changed-synthetic-pw",
        &secrets()
    )
    .is_ok());
}

#[test]
fn tampered_and_foreign_replicas_are_skipped_with_warning() {
    let remote = MemoryRemote::default();
    let mut a = new_user(&remote);
    let mut b = join(&remote, PW);
    b.add("B", "b");
    b.sync(&remote).unwrap();

    // Flip a byte inside B's ciphertext.
    let b_name = replica_name(b.vault.device_id());
    remote.tamper(&b_name, |c| {
        let s = String::from_utf8(c.clone()).unwrap();
        let mut v: serde_json::Value = serde_json::from_str(&s).unwrap();
        let ct = v["payload"]["ciphertext"].as_str().unwrap().to_string();
        let flipped = match ct.strip_prefix('A') {
            Some(rest) => format!("B{rest}"),
            None => format!("A{}", &ct[1..]),
        };
        v["payload"]["ciphertext"] = flipped.into();
        *c = serde_json::to_vec(&v).unwrap();
    });
    // A replica from an unrelated vault in the same folder.
    let stranger = Vault::create(PW, KdfCost::MINIMUM).unwrap();
    remote.put_raw(
        &replica_name(stranger.device_id()),
        stranger.seal_replica().unwrap().to_bytes().unwrap(),
    );
    // Garbage under a replica name.
    remote.put_raw(&replica_name(uuid::Uuid::new_v4()), b"not json".to_vec());

    let r = a.sync(&remote).unwrap();
    assert_eq!(r.warnings.len(), 3, "{:?}", r.warnings);
    assert_eq!(a.vault.entries().count(), 0);
    // Corrupt replicas are retried next time rather than marked as seen.
    assert_eq!(a.sync(&remote).unwrap().warnings.len(), 3);
}

#[test]
fn header_from_another_vault_is_fatal() {
    let remote = MemoryRemote::default();
    let mut a = new_user(&remote);
    let other_remote = MemoryRemote::default();
    let _x = new_user(&other_remote);
    // Point A at a folder whose header belongs to a different vault.
    let err = a.sync(&other_remote).unwrap_err();
    assert!(matches!(err, SyncError::Core(keyroam_core::Error::VaultMismatch)));
    assert!(matches!(
        SyncState::load(&a.store).unwrap().status(),
        SyncStatus::Error { .. }
    ));
}

#[test]
fn copied_profile_does_not_lose_updates() {
    // Two machines end up with the same device id (profile copied). Each
    // merges the other's write to the shared replica before overwriting it.
    let remote = MemoryRemote::default();
    let mut a = new_user(&remote);
    a.sync(&remote).unwrap();
    let dir2 = tempfile::tempdir().unwrap();
    for f in ["vault.krv", "sync-state.json", "secrets.krs"] {
        std::fs::copy(a.store.root().join(f), dir2.path().join(f)).unwrap();
    }
    let store2 = LocalStore::new(dir2.path());
    let mut clone = Device {
        vault: Vault::open(&store2.load_vault().unwrap(), PW).unwrap(),
        store: store2,
        _dir: dir2,
    };
    let x = a.add("X", "x");
    let y = clone.add("Y", "y");
    a.sync(&remote).unwrap();
    clone.sync(&remote).unwrap();
    a.sync(&remote).unwrap();
    assert_eq!(a.password_of(y), "y");
    assert_eq!(clone.password_of(x), "x");
}

#[test]
fn merges_keep_local_backups() {
    let remote = MemoryRemote::default();
    let mut a = new_user(&remote);
    let mut b = join(&remote, PW);
    a.add("A", "a");
    a.sync(&remote).unwrap();
    let before = b.store.local_backups().unwrap().len();
    assert!(b.sync(&remote).unwrap().local_changed);
    assert_eq!(b.store.local_backups().unwrap().len(), before + 1);
}

#[test]
fn status_reporting() {
    let mut st = SyncState::default();
    assert_eq!(st.status(), SyncStatus::NeverSynced);
    st.pending_upload = true;
    assert!(matches!(st.status(), SyncStatus::Pending { last_success_at: None }));
    st.pending_upload = false;
    st.last_success_at = Some(5);
    assert_eq!(st.status(), SyncStatus::UpToDate { at: 5 });
    st.last_error = Some("boom".into());
    assert!(matches!(st.status(), SyncStatus::Error { .. }));
}

#[test]
fn end_to_end_over_drive_http_api() {
    // Real DriveClient HTTP code against a local Drive stand-in.
    let (server, state) = fake::start("tok");
    let drive_a = fake::client(&server, fake::StaticToken::new(&["tok"]));
    let drive_b = fake::client(&server, fake::StaticToken::new(&["tok"]));

    let mut a = new_user(&drive_a);
    let id = a.add("Example", "synthetic-1");
    a.sync(&drive_a).unwrap();
    let mut b = join(&drive_b, PW);
    assert_eq!(b.password_of(id), "synthetic-1");

    a.edit(id, "from-a");
    b.edit(id, "from-b");
    a.sync(&drive_a).unwrap();
    b.sync(&drive_b).unwrap();
    a.sync(&drive_a).unwrap();
    assert!(a.vault.get(id).unwrap().has_conflict());
    assert_eq!(a.vault.state(), b.vault.state());

    // Nothing in Drive contains plaintext.
    for f in &state.lock().unwrap().files {
        let s = String::from_utf8_lossy(&f.content);
        assert!(!s.contains("synthetic-1") && !s.contains("from-a") && !s.contains("example.test"));
    }
    let names: Vec<String> = state.lock().unwrap().files.iter().map(|f| f.name.clone()).collect();
    assert!(names.contains(&HEADER_NAME.to_string()));
    assert_eq!(names.iter().filter(|n| n.starts_with("keyroam.replica.")).count(), 2);
}
