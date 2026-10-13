//! Runs the real host binary the way a browser does, against an in-process
//! bridge server that answers with the real protocol handler.

use std::io::Write;
use std::process::{Child, ChildStdin, ChildStdout, Command, Stdio};
use std::sync::{Arc, Mutex};

use keyroam_autofill::bridge::{BridgeServer, Handler};
use keyroam_autofill::framing::{read_frame, write_frame};
use keyroam_autofill::protocol::{handle, parse_request, Committer};
use keyroam_core::{EntryData, EntryId, KdfCost, LocalStore, Vault};
use serde_json::{json, Value};

struct Direct;
impl Committer for Direct {
    fn commit(
        &mut self,
        vault: &mut Vault,
        change: &mut dyn FnMut(&mut Vault) -> keyroam_core::Result<(EntryId, bool)>,
    ) -> Result<(EntryId, bool), String> {
        change(vault).map_err(|e| e.to_string())
    }
}

fn app_handler(vault: Arc<Mutex<Option<Vault>>>) -> Handler {
    Arc::new(move |req: &[u8]| match parse_request(req) {
        Ok((rid, r)) => handle(&rid, r, vault.lock().unwrap().as_mut(), &mut Direct),
        Err(e) => e,
    })
}

struct Host {
    child: Child,
    stdin: ChildStdin,
    stdout: ChildStdout,
}

impl Host {
    fn start(home: &std::path::Path) -> Host {
        let mut child = Command::new(env!("CARGO_BIN_EXE_keyroam-native-host"))
            // What Chromium passes.
            .arg("chrome-extension://ihplgegclknoghjpemmnfaoeogcahpba/")
            .env("KEYROAM_HOME", home)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .spawn()
            .unwrap();
        let stdin = child.stdin.take().unwrap();
        let stdout = child.stdout.take().unwrap();
        Host { child, stdin, stdout }
    }

    fn call(&mut self, msg: Value) -> Value {
        write_frame(&mut self.stdin, msg.to_string().as_bytes()).unwrap();
        self.stdin.flush().unwrap();
        let reply = read_frame(&mut self.stdout).unwrap().expect("host answered");
        serde_json::from_slice(&reply).unwrap()
    }

    fn finish(mut self) {
        drop(self.stdin);
        assert!(self.child.wait().unwrap().success());
    }
}

#[test]
fn relays_between_extension_and_app() {
    let dir = tempfile::tempdir().unwrap();
    let store = LocalStore::new(dir.path());
    let mut host = Host::start(dir.path());

    // No app yet.
    let r = host.call(json!({ "rid": 1, "type": "hello" }));
    assert_eq!(r["rid"], 1);
    assert_eq!(r["error"]["code"], "app_not_running");

    // The app starts with its vault locked.
    let vault = Arc::new(Mutex::new(None));
    let server = BridgeServer::start(&store, app_handler(vault.clone())).unwrap();
    let r = host.call(json!({ "rid": 2, "type": "hello" }));
    assert_eq!(r["ok"]["unlocked"], false);
    let page = json!({ "url": "https://www.example.com/login", "top_url": "https://www.example.com/" });
    let r = host.call(json!({ "rid": 3, "type": "logins", "page": page }));
    assert_eq!(r["error"]["code"], "locked");

    // Unlocked: credentials for the page's site only.
    let mut v = Vault::create("synthetic-master-pw-1", KdfCost::MINIMUM).unwrap();
    let id = v
        .add_entry(EntryData {
            title: "Example".into(),
            username: "alice".into(),
            password: "synthetic-secret".into(),
            urls: vec!["https://example.com".into()],
            ..Default::default()
        })
        .unwrap();
    *vault.lock().unwrap() = Some(v);
    let r = host.call(json!({ "rid": 4, "type": "logins", "page": page }));
    assert_eq!(r["ok"]["logins"][0]["username"], "alice");
    let r = host.call(json!({ "rid": 5, "type": "credential", "page": page, "id": id.to_string() }));
    assert_eq!(r["ok"]["password"], "synthetic-secret");

    // Malformed requests are answered by the host itself.
    let r = host.call(json!({ "rid": 6, "type": "bogus" }));
    assert_eq!(r["error"]["code"], "invalid_request");

    // The app restarts: read-only requests reconnect transparently.
    drop(server);
    let r = host.call(json!({ "rid": 7, "type": "hello" }));
    assert_eq!(r["error"]["code"], "app_not_running");
    let _server = BridgeServer::start(&store, app_handler(vault.clone())).unwrap();
    let r = host.call(json!({ "rid": 8, "type": "hello" }));
    assert_eq!(r["ok"]["unlocked"], true);

    // Saving goes through.
    let r = host.call(json!({
        "rid": 9, "type": "save", "page": page, "username": "bob", "password": "synthetic-bob"
    }));
    assert_eq!(r["ok"]["updated"], false);
    assert_eq!(vault.lock().unwrap().as_ref().unwrap().entries().count(), 2);

    host.finish();
}

#[test]
fn install_help_and_version() {
    let out = Command::new(env!("CARGO_BIN_EXE_keyroam-native-host"))
        .arg("--version")
        .output()
        .unwrap();
    assert!(out.status.success());
    assert!(String::from_utf8_lossy(&out.stdout).starts_with("keyroam-native-host "));
    let out = Command::new(env!("CARGO_BIN_EXE_keyroam-native-host"))
        .args(["install", "--bogus"])
        .output()
        .unwrap();
    assert!(!out.status.success());
}
