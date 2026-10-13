use std::sync::Arc;

use keyroam_sync::testing::MemoryRemote;
use keyroam_sync::{RemoteFile, RemoteStore};
use serde_json::json;
use tempfile::TempDir;

use super::*;

const PW: &str = "synthetic-master-pw-1";

/// Shares one in-memory Drive folder between several engines ("devices").
#[derive(Clone)]
struct Shared(Arc<MemoryRemote>);

impl RemoteStore for Shared {
    fn list(&self) -> keyroam_sync::Result<Vec<RemoteFile>> {
        self.0.list()
    }
    fn download(&self, id: &str) -> keyroam_sync::Result<Vec<u8>> {
        self.0.download(id)
    }
    fn create(&self, name: &str, bytes: &[u8]) -> keyroam_sync::Result<RemoteFile> {
        self.0.create(name, bytes)
    }
    fn update(&self, id: &str, bytes: &[u8]) -> keyroam_sync::Result<RemoteFile> {
        self.0.update(id, bytes)
    }
}

struct Device {
    engine: Engine,
    _dir: TempDir,
}

impl Device {
    fn new() -> Device {
        let dir = tempfile::tempdir().unwrap();
        let engine = Engine::with_kdf_cost(KdfCost::MINIMUM);
        engine
            .dispatch("open", json!({ "data_dir": dir.path().to_str().unwrap() }))
            .unwrap();
        Device { engine, _dir: dir }
    }
    fn call(&self, method: &str, params: Value) -> ApiResult {
        self.engine.dispatch(method, params)
    }
    fn ok(&self, method: &str, params: Value) -> Value {
        self.call(method, params).unwrap_or_else(|e| panic!("{method}: {e:?}"))
    }
    fn err(&self, method: &str, params: Value) -> &'static str {
        self.call(method, params).expect_err(method).code
    }
    fn sign_in(&self, remote: &Shared) {
        self.engine
            .sign_in_with_remote(Box::new(remote.clone()), "synthetic-refresh-token")
            .unwrap();
    }
    fn add(&self, title: &str, pw: &str) -> String {
        let r = self.ok(
            "add_entry",
            json!({ "data": { "title": title, "username": "u@example.test", "password": pw } }),
        );
        r["id"].as_str().unwrap().to_string()
    }
    fn password(&self, id: &str) -> String {
        let e = self.ok("get_entry", json!({ "id": id }));
        e["versions"][0]["data"]["password"].as_str().unwrap().to_string()
    }
}

#[test]
fn json_protocol_errors() {
    let e = Engine::with_kdf_cost(KdfCost::MINIMUM);
    let v: Value = serde_json::from_str(&e.handle_json("not json")).unwrap();
    assert_eq!(v["error"]["code"], "invalid_request");
    let v: Value = serde_json::from_str(&e.handle_json(r#"{"method":"nope"}"#)).unwrap();
    assert_eq!(v["error"]["code"], "invalid_request");
    let v: Value = serde_json::from_str(&e.handle_json(r#"{"method":"list_entries","params":{}}"#)).unwrap();
    assert_eq!(v["error"]["code"], "invalid_request"); // not opened
    let v: Value = serde_json::from_str(&e.handle_json(r#"{"method":"add_entry","params":{"x":1}}"#)).unwrap();
    assert_eq!(v["error"]["code"], "invalid_request");
}

#[test]
fn new_user_then_existing_user_onboarding() {
    let remote = Shared(Arc::new(MemoryRemote::default()));
    let a = Device::new();
    let st = a.ok("state", json!({}));
    assert_eq!(st["has_vault"], false);
    assert_eq!(st["signed_in"], false);

    assert_eq!(a.err("check_password", json!({ "password": "short" })), "invalid_input");
    a.ok("check_password", json!({ "password": PW }));
    // Onboarding order: password, then Google, then create.
    assert_eq!(a.err("create_vault", json!({ "password": PW })), "not_signed_in");
    a.sign_in(&remote);
    let r = a.ok("create_vault", json!({ "password": PW }));
    assert_eq!(r["state"]["unlocked"], true);
    assert_eq!(r["state"]["has_vault"], true);
    assert_eq!(r["sync"]["report"]["uploaded"], true);
    let id = a.add("Example", "synthetic-1");
    a.ok("sync", json!({}));

    // Second device.
    let b = Device::new();
    assert_eq!(b.err("fetch_remote_vault", json!({})), "not_signed_in");
    b.sign_in(&remote);
    b.ok("fetch_remote_vault", json!({}));
    assert_eq!(
        b.err("join_vault", json!({ "password": "wrong-password-xyz" })),
        "wrong_password"
    );
    let r = b.ok("join_vault", json!({ "password": PW }));
    assert_eq!(r["state"]["entry_count"], 1);
    assert_eq!(b.password(&id), "synthetic-1");
    assert_eq!(b.ok("state", json!({}))["sync"]["status"], "up_to_date");

    // A third "new user" on the same account is refused.
    let c = Device::new();
    c.sign_in(&remote);
    assert_eq!(c.err("create_vault", json!({ "password": PW })), "remote_vault_exists");
}

#[test]
fn entries_conflicts_and_sync() {
    let remote = Shared(Arc::new(MemoryRemote::default()));
    let a = Device::new();
    a.sign_in(&remote);
    a.ok("create_vault", json!({ "password": PW }));
    let id = a.add("Shared", "base");
    a.add("alpha", "x");
    a.ok("sync", json!({}));
    let b = Device::new();
    b.sign_in(&remote);
    b.ok("fetch_remote_vault", json!({}));
    b.ok("join_vault", json!({ "password": PW }));

    let list = a.ok("list_entries", json!({}));
    let titles: Vec<&str> = list
        .as_array()
        .unwrap()
        .iter()
        .map(|e| e["title"].as_str().unwrap())
        .collect();
    assert_eq!(titles, vec!["alpha", "Shared"]);
    assert_eq!(
        a.ok("list_entries", json!({ "query": "shar" }))
            .as_array()
            .unwrap()
            .len(),
        1
    );

    let data = |pw: &str| json!({ "title": "Shared", "password": pw });
    a.ok("update_entry", json!({ "id": id, "data": data("from-a") }));
    b.ok("update_entry", json!({ "id": id, "data": data("from-b") }));
    a.ok("sync", json!({}));
    let r = b.ok("sync", json!({}));
    assert_eq!(r["report"]["conflicts"], 1);
    assert_eq!(r["state"]["conflicts"], 1);
    let e = b.ok("get_entry", json!({ "id": id }));
    assert_eq!(e["conflict"], true);
    assert_eq!(e["versions"].as_array().unwrap().len(), 2);
    let choice = e["versions"]
        .as_array()
        .unwrap()
        .iter()
        .position(|v| v["data"]["password"] == "from-a")
        .unwrap();
    b.ok("resolve_conflict", json!({ "id": id, "choice": choice }));
    b.ok("sync", json!({}));
    a.ok("sync", json!({}));
    assert_eq!(a.password(&id), "from-a");
    assert_eq!(a.ok("state", json!({}))["conflicts"], 0);

    a.ok("delete_entry", json!({ "id": id }));
    assert_eq!(
        a.ok("list_entries", json!({ "deleted": true }))
            .as_array()
            .unwrap()
            .len(),
        1
    );
    a.ok("restore_entry", json!({ "id": id }));
    assert_eq!(
        a.ok("list_entries", json!({ "deleted": true }))
            .as_array()
            .unwrap()
            .len(),
        0
    );
    assert_eq!(a.err("get_entry", json!({ "id": "nope" })), "invalid_input");
}

#[test]
fn offline_lock_unlock_and_password_change() {
    let remote = Shared(Arc::new(MemoryRemote::default()));
    let a = Device::new();
    a.sign_in(&remote);
    a.ok("create_vault", json!({ "password": PW }));
    remote.0.set_offline(true);
    a.add("Offline", "synthetic");
    assert_eq!(a.err("sync", json!({})), "offline");
    let st = a.ok("state", json!({}));
    assert_eq!(st["sync"]["status"], "offline");
    assert_eq!(st["sync"]["pending_upload"], true);

    a.ok("lock", json!({}));
    assert_eq!(a.err("list_entries", json!({})), "locked");
    assert_eq!(
        a.err("unlock", json!({ "password": "wrong-password-xyz" })),
        "wrong_password"
    );
    // Unlocking reads the encrypted refresh token; without a configured OAuth
    // client in tests there is no Drive client, so sync reports that clearly.
    let st = a.ok("unlock", json!({ "password": PW }));
    assert_eq!(st["entry_count"], 1);
    assert_eq!(st["signed_in"], true);

    a.ok("change_password", json!({ "current": PW, "new": "new-synthetic-pw-2" }));
    a.ok("lock", json!({}));
    assert_eq!(a.err("unlock", json!({ "password": PW })), "wrong_password");
    a.ok("unlock", json!({ "password": "new-synthetic-pw-2" }));
    assert_eq!(
        a.err(
            "change_password",
            json!({ "current": "bad-bad-bad-bad", "new": "x-synthetic-pw-3" })
        ),
        "wrong_password"
    );
}

#[test]
fn backup_export_import() {
    let remote = Shared(Arc::new(MemoryRemote::default()));
    let a = Device::new();
    a.sign_in(&remote);
    a.ok("create_vault", json!({ "password": PW }));
    a.add("Backed up", "synthetic");
    let path = a._dir.path().join("out").join("b.krbackup");
    let p = path.to_str().unwrap();
    assert_eq!(
        a.err("export_backup", json!({ "path": p, "password": "short" })),
        "invalid_input"
    );
    let r = a.ok("export_backup", json!({ "path": p, "password": "backup-synthetic-pw" }));
    assert_eq!(r["entries"], 1);
    assert_eq!(
        a.err("export_backup", json!({ "path": p, "password": "backup-synthetic-pw" })),
        "exists"
    );

    let other = Shared(Arc::new(MemoryRemote::default()));
    let b = Device::new();
    b.sign_in(&other);
    b.ok("create_vault", json!({ "password": PW }));
    assert_eq!(
        b.err("import_backup", json!({ "path": p, "password": "wrong-pw-123456" })),
        "wrong_password"
    );
    let r = b.ok("import_backup", json!({ "path": p, "password": "backup-synthetic-pw" }));
    assert_eq!(r["before"], 0);
    assert_eq!(r["after"], 1);
    let r = b.ok("import_backup", json!({ "path": p, "password": "backup-synthetic-pw" }));
    assert_eq!(r["changed"], false);
}

#[test]
fn sign_out_and_generator() {
    let remote = Shared(Arc::new(MemoryRemote::default()));
    let a = Device::new();
    a.sign_in(&remote);
    a.ok("create_vault", json!({ "password": PW }));
    let st = a.ok("sign_out", json!({}));
    assert_eq!(st["signed_in"], false);
    assert_eq!(a.err("sync", json!({})), "not_signed_in");

    let pw = a.ok("generate_password", json!({ "length": 30, "symbols": false }));
    let pw = pw.as_str().unwrap();
    assert_eq!(pw.len(), 30);
    assert!(pw.chars().all(|c| c.is_ascii_alphanumeric()));
    assert_eq!(a.err("generate_password", json!({ "length": 3 })), "invalid_input");
    assert!(a
        .ok("default_backup_path", json!({}))
        .as_str()
        .unwrap()
        .ends_with(".krbackup"));
    let custom = a.ok("default_backup_path", json!({ "dir": "custom-dir" }));
    assert!(custom.as_str().unwrap().starts_with("custom-dir"));
}

#[test]
fn sign_in_into_a_different_vault_is_refused() {
    let r1 = Shared(Arc::new(MemoryRemote::default()));
    let r2 = Shared(Arc::new(MemoryRemote::default()));
    let other = Device::new();
    other.sign_in(&r2);
    other.ok("create_vault", json!({ "password": PW }));

    let a = Device::new();
    a.sign_in(&r1);
    a.ok("create_vault", json!({ "password": PW }));
    let e = a.engine.sign_in_with_remote(Box::new(r2.clone()), "t").unwrap_err();
    assert_eq!(e.code, "vault_mismatch");
}

#[test]
fn civil_dates() {
    assert_eq!(civil_date(0), (1970, 1, 1));
    assert_eq!(civil_date(19_723), (2024, 1, 1));
    assert_eq!(civil_date(20_739), (2026, 10, 13));
    assert_eq!(civil_date(11_016), (2000, 2, 29));
}
