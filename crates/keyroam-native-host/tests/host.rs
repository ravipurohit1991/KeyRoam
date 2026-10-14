//! Drives the real `keyroam-native-host` executable the way a browser does:
//! length-prefixed JSON on stdin and stdout, relayed to an app-side server
//! that answers from a real (synthetic) vault.

use std::io::{Read, Write};
use std::path::Path;
use std::process::{Command, Stdio};
use std::sync::{Arc, Mutex};

use keyroam_autofill::ipc::{Handler, Server};
use keyroam_autofill::protocol::{read_frame, write_frame};
use keyroam_autofill::service::{handle, VaultAccess};
use keyroam_core::{EntryData, KdfCost, Vault};
use serde_json::{json, Value};

const HOST: &str = env!("CARGO_BIN_EXE_keyroam-native-host");
const CALLER: &str = "chrome-extension://ghgndmpoakmlmcagombpmikjdalnmiec/";

/// Run the host with `messages` on stdin; return its responses and exit status.
fn run_host(home: &Path, input: &[u8]) -> (Vec<Value>, bool) {
    let mut child = Command::new(HOST)
        .arg(CALLER)
        .env("KEYROAM_HOME", home)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()
        .unwrap();
    child.stdin.take().unwrap().write_all(input).unwrap();
    let mut out = Vec::new();
    child.stdout.take().unwrap().read_to_end(&mut out).unwrap();
    let ok = child.wait().unwrap().success();
    let mut cursor = std::io::Cursor::new(out);
    let mut responses = Vec::new();
    while let Some(frame) = read_frame(&mut cursor, 1 << 20).unwrap() {
        responses.push(serde_json::from_slice(&frame).unwrap());
    }
    (responses, ok)
}

fn frames(messages: &[Value]) -> Vec<u8> {
    let mut buf = Vec::new();
    for m in messages {
        write_frame(&mut buf, &serde_json::to_vec(m).unwrap()).unwrap();
    }
    buf
}

/// An app-side server over a vault; `None` means the vault is locked.
fn app(home: &Path, vault: Arc<Mutex<Option<Vault>>>) -> Server {
    let handler: Handler = Arc::new(move |req, caller| {
        assert_eq!(caller, CALLER);
        let mut guard = vault.lock().unwrap();
        let access = match guard.as_mut() {
            Some(v) => VaultAccess::Unlocked(v),
            None => VaultAccess::Locked,
        };
        handle(&req.command, access).map(|o| o.body)
    });
    Server::start(home, handler).unwrap()
}

fn synthetic_vault() -> (Vault, String) {
    let mut v = Vault::create("synthetic-master-pw-1", KdfCost::MINIMUM).unwrap();
    let id = v
        .add_entry(EntryData {
            title: "Example".into(),
            username: "alice@example.test".into(),
            password: "synthetic-site-pw".into(),
            urls: vec!["https://example.com".into()],
            ..Default::default()
        })
        .unwrap();
    (v, id.to_string())
}

#[test]
fn reports_app_not_running() {
    let home = tempfile::tempdir().unwrap();
    let (out, ok) = run_host(home.path(), &frames(&[json!({"v": 1, "id": "a", "type": "status"})]));
    assert!(ok);
    assert_eq!(out.len(), 1);
    assert_eq!(out[0]["id"], "a");
    assert_eq!(out[0]["error"]["code"], "app_not_running");
}

#[test]
fn relays_list_fill_and_save() {
    let home = tempfile::tempdir().unwrap();
    let (vault, id) = synthetic_vault();
    let vault = Arc::new(Mutex::new(Some(vault)));
    let _server = app(home.path(), vault.clone());

    let page = "https://example.com/login";
    let (out, ok) = run_host(
        home.path(),
        &frames(&[
            json!({"v": 1, "id": 1, "type": "status"}),
            json!({"v": 1, "id": 2, "type": "list", "url": page, "top_url": page}),
            json!({"v": 1, "id": 3, "type": "fill", "url": page, "top_url": page, "entry_id": id}),
            json!({"v": 1, "id": 4, "type": "fill", "url": "https://evil.test/", "top_url": "https://evil.test/",
                   "entry_id": id, "consent": true}),
            json!({"v": 1, "id": 5, "type": "check_save", "url": page, "top_url": page,
                   "username": "bob", "password": "synthetic-new"}),
            json!({"v": 1, "id": 6, "type": "save", "url": page, "top_url": page,
                   "username": "bob", "password": "synthetic-new"}),
        ]),
    );
    assert!(ok);
    let ids: Vec<i64> = out.iter().map(|r| r["id"].as_i64().unwrap()).collect();
    assert_eq!(ids, [1, 2, 3, 4, 5, 6]);
    assert_eq!(out[0]["ok"]["state"], "unlocked");
    assert_eq!(out[1]["ok"]["logins"][0]["username"], "alice@example.test");
    assert!(!out[1].to_string().contains("synthetic-site-pw"));
    assert_eq!(out[2]["ok"]["password"], "synthetic-site-pw");
    assert_eq!(out[3]["error"]["code"], "no_match");
    assert_eq!(out[4]["ok"]["action"], "save");
    assert_eq!(out[5]["ok"]["result"], "created");
    assert_eq!(vault.lock().unwrap().as_ref().unwrap().entries().count(), 2);

    // Locking the app stops everything but status.
    *vault.lock().unwrap() = None;
    let (out, _) = run_host(
        home.path(),
        &frames(&[
            json!({"v": 1, "id": 7, "type": "status"}),
            json!({"v": 1, "id": 8, "type": "list", "url": page, "top_url": page}),
        ]),
    );
    assert_eq!(out[0]["ok"]["state"], "locked");
    assert_eq!(out[1]["error"]["code"], "locked");
}

#[test]
fn rejects_malformed_and_oversized_messages() {
    let home = tempfile::tempdir().unwrap();
    let _server = app(home.path(), Arc::new(Mutex::new(None)));

    let mut input = Vec::new();
    write_frame(&mut input, b"not json").unwrap();
    write_frame(&mut input, b"[1,2]").unwrap();
    write_frame(&mut input, br#"{"v": 1, "id": "x", "type": "dump_everything"}"#).unwrap();
    let (out, ok) = run_host(home.path(), &input);
    assert!(ok);
    assert_eq!(out.len(), 3);
    assert!(out.iter().all(|r| r["error"]["code"] == "invalid_request"));
    assert_eq!(out[2]["id"], "x");

    // An oversized frame ends the session with an error.
    let mut input = (10u32 * 1024 * 1024).to_ne_bytes().to_vec();
    input.extend_from_slice(&[b' '; 16]);
    let (out, ok) = run_host(home.path(), &input);
    assert!(!ok);
    assert_eq!(out[0]["error"]["code"], "invalid_request");
}

#[test]
fn cli_help_and_bad_arguments() {
    let out = Command::new(HOST).arg("--help").output().unwrap();
    assert!(out.status.success());
    assert!(String::from_utf8_lossy(&out.stdout).contains("install"));

    let out = Command::new(HOST).args(["install", "--extension-id"]).output().unwrap();
    assert!(!out.status.success());
}
