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

    // Permanent purge reaches the other device.
    assert_eq!(a.err("purge_entry", json!({ "id": id })), "invalid_input");
    a.ok("delete_entry", json!({ "id": id }));
    assert_eq!(a.ok("purge_deleted", json!({}))["purged"], 1);
    assert_eq!(a.err("get_entry", json!({ "id": id })), "not_found");
    a.ok("sync", json!({}));
    b.ok("sync", json!({}));
    assert_eq!(b.err("get_entry", json!({ "id": id })), "not_found");
    assert_eq!(
        b.ok("list_entries", json!({ "deleted": true }))
            .as_array()
            .unwrap()
            .len(),
        0
    );
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
fn vault_key_rotation_between_devices() {
    let remote = Shared(Arc::new(MemoryRemote::default()));
    let a = Device::new();
    a.sign_in(&remote);
    a.ok("create_vault", json!({ "password": PW }));
    let id = a.add("Example", "synthetic-1");
    a.ok("sync", json!({}));
    let b = Device::new();
    b.sign_in(&remote);
    b.ok("fetch_remote_vault", json!({}));
    b.ok("join_vault", json!({ "password": PW }));

    assert_eq!(
        a.err("rotate_vault_key", json!({ "password": "wrong-password-xyz" })),
        "wrong_password"
    );
    a.ok("rotate_vault_key", json!({ "password": PW }));
    a.ok("sync", json!({}));
    assert_eq!(b.err("sync", json!({})), "key_rotated");
    assert_eq!(
        b.err("adopt_rotated_key", json!({ "password": "wrong-password-xyz" })),
        "wrong_password"
    );
    let r = b.ok("adopt_rotated_key", json!({ "password": PW }));
    assert!(r["sync"]["report"].is_object(), "{r}");
    assert_eq!(b.password(&id), "synthetic-1");
    // The re-encrypted local vault and secrets open with the password.
    b.ok("lock", json!({}));
    assert_eq!(b.ok("unlock", json!({ "password": PW }))["signed_in"], true);
    assert_eq!(b.password(&id), "synthetic-1");
}

#[test]
fn csv_import_from_a_browser_export() {
    let remote = Shared(Arc::new(MemoryRemote::default()));
    let a = Device::new();
    a.sign_in(&remote);
    a.ok("create_vault", json!({ "password": PW }));
    let csv = a._dir.path().join("Chrome Passwords.csv");
    std::fs::write(
        &csv,
        "name,url,username,password,note\nexample.com,https://example.com/,alice,synthetic-1,\nexample.com,https://example.com/,alice,synthetic-1,\n,,,,\n",
    )
    .unwrap();
    let r = a.ok(
        "import_csv",
        json!({ "path": csv.to_str().unwrap(), "delete_file": true }),
    );
    assert_eq!(r["format"], "Chrome");
    assert_eq!(
        (r["added"].as_u64(), r["duplicates"].as_u64(), r["skipped"].as_u64()),
        (Some(1), Some(1), Some(1))
    );
    assert_eq!(r["deleted"], true);
    assert!(!csv.exists());
    assert_eq!(a.ok("state", json!({}))["entry_count"], 1);
    assert_eq!(a.ok("state", json!({}))["sync"]["pending_upload"], true);
    assert_eq!(a.err("import_csv", json!({ "path": csv.to_str().unwrap() })), "io");
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

#[test]
fn browser_integration_serves_the_extension_only_while_unlocked() {
    use keyroam_autofill::bridge::{BridgeClient, BRIDGE_FILE};

    let remote = Shared(Arc::new(MemoryRemote::default()));
    let a = Device::new();
    let dir = a._dir.path().to_path_buf();
    a.sign_in(&remote);
    a.ok("create_vault", json!({ "password": PW }));
    let id = a.ok(
        "add_entry",
        json!({ "data": { "title": "Example", "username": "alice", "password": "synthetic-1",
                          "urls": ["https://example.com"] } }),
    )["id"]
        .as_str()
        .unwrap()
        .to_string();

    // Off by default.
    assert_eq!(a.ok("state", json!({}))["browser_integration"], false);
    assert!(!dir.join(BRIDGE_FILE).exists());
    let st = a.ok("set_browser_integration", json!({ "enabled": true }));
    assert_eq!(st["browser_integration"], true);

    let call = |c: &mut BridgeClient, v: Value| -> Value {
        serde_json::from_slice(&c.call(v.to_string().as_bytes()).unwrap()).unwrap()
    };
    let mut c = BridgeClient::connect(&dir).unwrap();
    let page = json!({ "url": "https://login.example.com/", "top_url": "https://login.example.com/" });
    let r = call(
        &mut c,
        json!({ "rid": 1, "type": "credential", "page": page, "id": id }),
    );
    assert_eq!(r["ok"]["password"], "synthetic-1");

    // A save from the browser is persisted, queued for sync and announced.
    let rev = a.ok("state", json!({}))["revision"].as_u64().unwrap();
    let r = call(
        &mut c,
        json!({ "rid": 2, "type": "save", "page": page, "username": "bob", "password": "synthetic-2" }),
    );
    assert_eq!(r["ok"]["updated"], false);
    let st = a.ok("state", json!({}));
    assert_eq!(st["revision"].as_u64().unwrap(), rev + 1);
    assert_eq!(st["entry_count"], 2);
    assert_eq!(st["sync"]["pending_upload"], true);

    // Locked: nothing is released.
    a.ok("lock", json!({}));
    let r = call(
        &mut c,
        json!({ "rid": 3, "type": "credential", "page": page, "id": id }),
    );
    assert_eq!(r["error"]["code"], "locked");
    a.ok("unlock", json!({ "password": PW }));
    assert_eq!(a.ok("list_entries", json!({})).as_array().unwrap().len(), 2);

    // The setting survives a restart of the app.
    let restarted = Engine::with_kdf_cost(KdfCost::MINIMUM);
    restarted
        .dispatch("open", json!({ "data_dir": dir.to_str().unwrap() }))
        .unwrap();
    assert_eq!(
        restarted.dispatch("state", json!({})).unwrap()["browser_integration"],
        true
    );
    drop(restarted);

    a.ok("set_browser_integration", json!({ "enabled": true }));
    a.ok("set_browser_integration", json!({ "enabled": false }));
    assert!(!dir.join(BRIDGE_FILE).exists());
    assert!(BridgeClient::connect(&dir).is_err());
    assert_eq!(a.ok("state", json!({}))["browser_integration"], false);

    assert_eq!(
        a.err(
            "install_browser_host",
            json!({ "host_path": dir.join("missing-host").to_str().unwrap() })
        ),
        "not_found"
    );
}

/// A device with a vault holding one web login and one app login.
fn android_device() -> (Device, Shared) {
    let remote = Shared(Arc::new(MemoryRemote::default()));
    let a = Device::new();
    a.sign_in(&remote);
    a.ok("create_vault", json!({ "password": PW }));
    for (title, user, pw, url) in [
        ("Example", "alice", "synthetic-web", "https://www.example.com"),
        ("Example app", "bob", "synthetic-app", "androidapp://com.example.app"),
    ] {
        a.ok(
            "add_entry",
            json!({ "data": { "title": title, "username": user, "password": pw, "urls": [url] } }),
        );
    }
    (a, remote)
}

#[test]
fn reopening_the_same_folder_keeps_the_session() {
    let (a, _remote) = android_device();
    let dir = a.ok("state", json!({}))["data_dir"].as_str().unwrap().to_string();
    // e.g. the Android autofill service unlocked before the UI opened.
    let st = a.ok("open", json!({ "data_dir": dir }));
    assert_eq!(st["unlocked"], true);
    // Another folder starts a new session.
    let other = tempfile::tempdir().unwrap();
    let st = a.ok("open", json!({ "data_dir": other.path().to_str().unwrap() }));
    assert_eq!(st["unlocked"], false);
    assert_eq!(st["has_vault"], false);
}

#[test]
fn biometric_key_unlocks_until_the_password_changes() {
    let (a, _remote) = android_device();
    assert_eq!(
        a.err("biometric_key", json!({ "password": "wrong-password-xyz" })),
        "wrong_password"
    );
    let key = a.ok("biometric_key", json!({ "password": PW }));
    let kek = key["key"].as_str().unwrap().to_string();
    assert_eq!(key["vault_id"], a.ok("state", json!({}))["vault_id"]);

    a.ok("lock", json!({}));
    let st = a.ok("unlock_with_key", json!({ "key": kek }));
    assert_eq!(st["unlocked"], true);
    assert_eq!(st["signed_in"], true);
    assert_eq!(st["entry_count"], 2);

    assert_eq!(
        a.err("unlock_with_key", json!({ "key": "not base64!" })),
        "invalid_input"
    );
    assert_eq!(a.err("unlock_with_key", json!({ "key": "AAAA" })), "invalid_input");

    // A master-password change makes the stored key useless.
    a.ok("change_password", json!({ "current": PW, "new": "new-synthetic-pw-2" }));
    a.ok("lock", json!({}));
    assert_eq!(a.err("unlock_with_key", json!({ "key": kek })), "wrong_password");
}

#[test]
fn android_autofill_lists_and_releases_only_matching_logins() {
    let (a, _remote) = android_device();
    let login_screen = json!([{ "input_type": 0x21 }, { "input_type": 0x81 }]);

    let r = a.ok(
        "android_logins",
        json!({ "package": "com.android.chrome", "web_domain": "login.example.com", "fields": login_screen }),
    );
    assert_eq!(r["state"], "unlocked");
    assert_eq!(r["target"], json!({ "kind": "web", "label": "example.com" }));
    assert_eq!(
        r["fields"],
        json!({ "username": 0, "password": 1, "new_password": null })
    );
    assert_eq!(r["logins"].as_array().unwrap().len(), 1);
    assert_eq!(r["logins"][0]["password"], "synthetic-web");

    let r = a.ok(
        "android_logins",
        json!({ "package": "com.example.app", "fields": login_screen }),
    );
    assert_eq!(r["logins"][0]["username"], "bob");

    // An app that claims a web domain only sees its own logins.
    let r = a.ok(
        "android_logins",
        json!({ "package": "com.evil.phish", "web_domain": "www.example.com", "fields": login_screen }),
    );
    assert_eq!(r["target"]["kind"], "app");
    assert!(r["logins"].as_array().unwrap().is_empty());

    // A screen without login fields.
    let r = a.ok(
        "android_logins",
        json!({ "package": "com.example.app", "fields": [{ "input_type": 2 }] }),
    );
    assert_eq!(r["fields"], Value::Null);

    // Credential Manager: pick one by id; another app cannot.
    let id = a.ok("android_logins", json!({ "package": "com.example.app" }))["logins"][0]["id"].clone();
    let c = a.ok("android_credential", json!({ "package": "com.example.app", "id": id }));
    assert_eq!(c, json!({ "username": "bob", "password": "synthetic-app" }));
    assert_eq!(
        a.err("android_credential", json!({ "package": "com.other.app", "id": id })),
        "not_found"
    );
    assert_eq!(
        a.err("android_logins", json!({ "package": "not a package" })),
        "invalid_input"
    );

    // Locked: fields only, no logins.
    a.ok("lock", json!({}));
    let r = a.ok(
        "android_logins",
        json!({ "package": "com.example.app", "fields": login_screen }),
    );
    assert_eq!(r["state"], "locked");
    assert!(r["logins"].as_array().unwrap().is_empty());
    assert_eq!(r["fields"]["password"], 1);
    assert_eq!(
        a.err("android_credential", json!({ "package": "com.example.app", "id": id })),
        "locked"
    );
}

#[test]
fn android_save_creates_updates_and_notifies_the_ui() {
    let (a, _remote) = android_device();
    let rev = |a: &Device| a.ok("state", json!({}))["revision"].as_u64().unwrap();
    let before = rev(&a);
    let fields =
        |user: &str, pw: &str| json!([{ "input_type": 0x21, "value": user }, { "input_type": 0x81, "value": pw }]);

    let r = a.ok(
        "android_save",
        json!({ "package": "com.new.app", "app_label": "New App", "fields": fields("carol", "synthetic-1") }),
    );
    assert_eq!(r["result"], "created");
    assert_eq!(rev(&a), before + 1);
    let r = a.ok(
        "android_save",
        json!({ "package": "com.new.app", "fields": fields("carol", "synthetic-1") }),
    );
    assert_eq!(r["result"], "unchanged");
    // Credential Manager passes the login directly.
    let r = a.ok(
        "android_save",
        json!({ "package": "com.new.app", "username": "carol", "password": "synthetic-2" }),
    );
    assert_eq!(r["result"], "updated");
    let listed = a.ok("list_entries", json!({ "query": "New App" }));
    assert_eq!(listed.as_array().unwrap().len(), 1);
    let entry = a.ok("get_entry", json!({ "id": r["id"] }));
    assert_eq!(entry["versions"][0]["data"]["password"], "synthetic-2");
    assert_eq!(entry["versions"][0]["data"]["urls"][0], "androidapp://com.new.app");
    // Saved to disk and queued for sync.
    assert_eq!(a.ok("state", json!({}))["sync"]["pending_upload"], true);

    // A browser's save is titled with the site, not the browser's name.
    let r = a.ok(
        "android_save",
        json!({ "package": "com.android.chrome", "web_domain": "shop.example.org", "app_label": "Chrome",
                "fields": fields("erin", "synthetic-4") }),
    );
    let entry = a.ok("get_entry", json!({ "id": r["id"] }));
    assert_eq!(entry["versions"][0]["data"]["title"], "example.org");
    assert_eq!(entry["versions"][0]["data"]["urls"][0], "https://shop.example.org");

    assert_eq!(
        a.err(
            "android_save",
            json!({ "package": "com.new.app", "fields": fields("carol", "") })
        ),
        "invalid_input"
    );
    a.ok("lock", json!({}));
    assert_eq!(
        a.err(
            "android_save",
            json!({ "package": "com.new.app", "fields": fields("dave", "synthetic-3") })
        ),
        "locked"
    );
}

#[test]
fn background_sync_only_runs_unlocked() {
    let (a, remote) = android_device();
    let r = a.ok("sync_if_unlocked", json!({}));
    assert_eq!(r["synced"], true);
    a.ok("lock", json!({}));
    assert_eq!(a.ok("sync_if_unlocked", json!({}))["synced"], false);
    drop(remote);
}

#[test]
fn mobile_sign_in_needs_a_mobile_client() {
    let a = Device::new();
    // The redirect slot accepts deliveries at any time (stale ones are
    // discarded when a sign-in begins).
    a.ok(
        "oauth_redirect",
        json!({ "url": "com.googleusercontent.apps.x:/oauth2redirect?state=s" }),
    );
    assert_eq!(
        a.err("begin_sign_in", json!({ "redirect": "carrier-pigeon" })),
        "invalid_request"
    );
    if keyroam_sync::OAuthClientConfig::mobile_from_env().is_err() {
        assert_eq!(
            a.err("begin_sign_in", json!({ "redirect": "custom" })),
            "not_configured"
        );
    }
}

#[test]
fn ios_extension_engine_shares_the_app_group_folder() {
    // The app: a vault with a web login and an Android-only login.
    let (app, _remote) = android_device();
    let dir = app.ok("state", json!({}))["data_dir"].as_str().unwrap().to_string();
    let kek = app.ok("biometric_key", json!({ "password": PW }))["key"].clone();

    // The AutoFill extension: another process, so another engine, over the
    // same folder. It starts locked.
    let ext = Engine::with_kdf_cost(KdfCost::MINIMUM);
    let call = |m: &str, p: Value| ext.dispatch(m, p);
    call("open", json!({ "data_dir": dir })).unwrap();
    let services = json!([{ "identifier": "www.example.com", "type": "domain" }]);
    let r = call("ios_logins", json!({ "services": services })).unwrap();
    assert_eq!(r["state"], "locked");
    assert_eq!(r["label"], "example.com");
    assert!(r["logins"].as_array().unwrap().is_empty());
    assert_eq!(call("ios_identities", json!({})).unwrap_err().code, "locked");

    // Face ID releases the key-encryption key from the shared keychain.
    assert_eq!(
        call("unlock_with_key", json!({ "key": kek })).unwrap()["unlocked"],
        true
    );
    let r = call("ios_logins", json!({ "services": services })).unwrap();
    assert_eq!(r["state"], "unlocked");
    let logins = r["logins"].as_array().unwrap();
    assert_eq!(logins.len(), 1);
    assert_eq!(logins[0]["username"], "alice");
    assert_eq!(logins[0]["match"], "exact");
    assert!(logins[0].get("password").is_none(), "the list carries no passwords");

    let id = logins[0]["id"].clone();
    let c = call("ios_credential", json!({ "services": services, "id": id })).unwrap();
    assert_eq!(c, json!({ "username": "alice", "password": "synthetic-web" }));
    // Another site cannot have it, and identifiers must be usable.
    let other = json!([{ "identifier": "https://example.org/login", "type": "url" }]);
    assert_eq!(
        call("ios_credential", json!({ "services": other, "id": id }))
            .unwrap_err()
            .code,
        "not_found"
    );
    assert_eq!(
        call(
            "ios_logins",
            json!({ "services": [{ "identifier": "", "type": "domain" }] })
        )
        .unwrap_err()
        .code,
        "invalid_input"
    );

    // QuickType identities: web logins only, no secrets.
    let ids = call("ios_identities", json!({})).unwrap();
    assert_eq!(ids, json!([{ "id": id, "user": "alice", "domain": "www.example.com" }]));

    // A login the app adds later is seen by the extension after it reloads
    // (here: lock and unlock, as each AutoFill request does).
    app.ok(
        "add_entry",
        json!({ "data": { "title": "Shop", "username": "carol", "password": "synthetic-shop", "urls": ["https://shop.example.com"] } }),
    );
    call("lock", json!({})).unwrap();
    call("unlock", json!({ "password": PW })).unwrap();
    let r = call("ios_logins", json!({ "services": services })).unwrap();
    assert_eq!(r["logins"].as_array().unwrap().len(), 2);
}
