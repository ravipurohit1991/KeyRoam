//! Drives the real `keyroam` binary. Google is not reachable from tests, so
//! the vault is created through the library and commands run with --no-sync.

use std::io::Write;
use std::path::Path;
use std::process::{Command, Output, Stdio};

use keyroam_core::{KdfCost, LocalStore, Vault};

const PW: &str = "synthetic-master-pw-1";

fn keyroam(dir: &Path, args: &[&str], stdin: &str) -> Output {
    let mut child = Command::new(env!("CARGO_BIN_EXE_keyroam"))
        .arg("--data-dir")
        .arg(dir)
        .args(["--password-stdin", "--no-sync"])
        .args(args)
        .env_remove("KEYROAM_GOOGLE_CLIENT_ID")
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap();
    child.stdin.take().unwrap().write_all(stdin.as_bytes()).unwrap();
    child.wait_with_output().unwrap()
}

fn ok(out: Output) -> String {
    assert!(
        out.status.success(),
        "stdout: {}\nstderr: {}",
        String::from_utf8_lossy(&out.stdout),
        String::from_utf8_lossy(&out.stderr)
    );
    String::from_utf8(out.stdout).unwrap()
}

fn setup() -> tempfile::TempDir {
    let dir = tempfile::tempdir().unwrap();
    let v = Vault::create(PW, KdfCost::MINIMUM).unwrap();
    LocalStore::new(dir.path())
        .save_new_vault(&v.to_local_file().unwrap())
        .unwrap();
    dir
}

fn first_id(list: &str) -> String {
    list.split_whitespace().next().unwrap().to_string()
}

#[test]
fn generate_works_without_vault() {
    let dir = tempfile::tempdir().unwrap();
    let out = ok(keyroam(dir.path(), &["generate", "--length", "32"], ""));
    assert_eq!(out.trim().chars().count(), 32);
    let out = ok(keyroam(dir.path(), &["status"], ""));
    assert!(out.contains("No vault"));
}

#[test]
fn entry_lifecycle() {
    let dir = setup();
    let d = dir.path();
    ok(keyroam(
        d,
        &[
            "add",
            "--title",
            "Example",
            "--username",
            "alice@example.test",
            "--url",
            "https://example.test",
        ],
        &format!("{PW}\nsynthetic-entry-pw\n"),
    ));
    let list = ok(keyroam(d, &["list"], &format!("{PW}\n")));
    assert!(list.contains("Example") && list.contains("alice@example.test"));
    let id = first_id(&list);

    let hidden = ok(keyroam(d, &["show", &id], &format!("{PW}\n")));
    assert!(!hidden.contains("synthetic-entry-pw"));
    let shown = ok(keyroam(d, &["show", &id, "--reveal"], &format!("{PW}\n")));
    assert!(shown.contains("synthetic-entry-pw"));

    ok(keyroam(
        d,
        &["edit", &id, "--password"],
        &format!("{PW}\nsynthetic-entry-pw-2\n"),
    ));
    let shown = ok(keyroam(d, &["show", &id, "--reveal", "--history"], &format!("{PW}\n")));
    assert!(shown.contains("synthetic-entry-pw-2") && shown.contains("history:"));

    ok(keyroam(d, &["rm", &id], &format!("{PW}\n")));
    assert!(ok(keyroam(d, &["list"], &format!("{PW}\n"))).contains("(no entries)"));
    assert!(ok(keyroam(d, &["list", "--deleted"], &format!("{PW}\n"))).contains(&id));
    ok(keyroam(d, &["restore", &id], &format!("{PW}\n")));
    assert!(ok(keyroam(d, &["list", "--search", "EXAMPLE"], &format!("{PW}\n"))).contains("Example"));

    // Nothing readable on disk.
    let raw = std::fs::read_to_string(d.join("vault.krv")).unwrap();
    assert!(!raw.contains("synthetic-entry-pw") && !raw.contains("alice@example.test"));

    // Permanent purge: deleted entries only, and nothing is left behind.
    assert!(!keyroam(d, &["purge", &id], &format!("{PW}\n")).status.success());
    ok(keyroam(d, &["rm", &id], &format!("{PW}\n")));
    assert!(ok(keyroam(d, &["purge", "--all"], &format!("{PW}\n"))).contains("erased 1 deleted entry"));
    assert!(!ok(keyroam(d, &["list", "--deleted"], &format!("{PW}\n"))).contains(&id));
    assert!(!keyroam(d, &["restore", &id], &format!("{PW}\n")).status.success());

    let status = ok(keyroam(d, &["status"], ""));
    assert!(status.contains("waiting to upload"), "{status}");
}

#[test]
fn wrong_password_fails() {
    let dir = setup();
    let out = keyroam(dir.path(), &["list"], "wrong-password-xyz\n");
    assert!(!out.status.success());
    assert!(String::from_utf8_lossy(&out.stderr).contains("incorrect master password"));
}

#[test]
fn export_and_import_backup() {
    let dir = setup();
    let d = dir.path();
    ok(keyroam(
        d,
        &["add", "--title", "Backed up", "--generate"],
        &format!("{PW}\n"),
    ));
    let backup = d.join("backup.krbackup");
    let bpw = "synthetic-backup-pw";
    ok(keyroam(
        d,
        &["export", backup.to_str().unwrap()],
        &format!("{PW}\n{bpw}\n{bpw}\n"),
    ));
    assert!(!keyroam(d, &["export", backup.to_str().unwrap()], "").status.success());

    // Restore into a different, empty vault.
    let other = setup();
    let out = ok(keyroam(
        other.path(),
        &["import", backup.to_str().unwrap()],
        &format!("{PW}\n{bpw}\n"),
    ));
    assert!(out.contains("0 entries before, 1 after"), "{out}");
    assert!(ok(keyroam(other.path(), &["list"], &format!("{PW}\n"))).contains("Backed up"));

    // Wrong backup password.
    let out = keyroam(
        other.path(),
        &["import", backup.to_str().unwrap()],
        &format!("{PW}\nnope-nope-nope\n"),
    );
    assert!(!out.status.success());
}

#[test]
fn change_master_password() {
    let dir = setup();
    let d = dir.path();
    let new = "brand-new-synthetic-pw";
    ok(keyroam(d, &["passwd"], &format!("{PW}\n{new}\n{new}\n")));
    assert!(!keyroam(d, &["list"], &format!("{PW}\n")).status.success());
    ok(keyroam(d, &["list"], &format!("{new}\n")));
    // A local backup of the pre-change vault was kept.
    assert!(!LocalStore::new(d).local_backups().unwrap().is_empty());
}

#[test]
fn sync_without_google_reports_clearly() {
    let dir = setup();
    let out = keyroam(dir.path(), &["sync"], &format!("{PW}\n"));
    assert!(!out.status.success());
    assert!(String::from_utf8_lossy(&out.stderr).contains("not signed in"));
}
