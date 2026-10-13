//! A stand-in for the desktop app, for the browser end-to-end test
//! (`browser-extension/e2e/`): it serves an unlocked synthetic vault over the
//! real bridge with the real protocol handler.
//!
//! cargo run -p keyroam-native-host --example e2e_app -- <data-dir> <page-origin>
//!
//! Prints `READY`, then `SAVED <title> <username>` for every login the
//! extension saves. Exits when stdin closes.

use std::io::{BufRead, Write};
use std::sync::{Arc, Mutex};

use keyroam_autofill::bridge::BridgeServer;
use keyroam_autofill::protocol::{handle, parse_request, Committer};
use keyroam_core::{EntryData, EntryId, KdfCost, LocalStore, Vault};

struct Announce;

impl Committer for Announce {
    fn commit(
        &mut self,
        vault: &mut Vault,
        change: &mut dyn FnMut(&mut Vault) -> keyroam_core::Result<(EntryId, bool)>,
    ) -> Result<(EntryId, bool), String> {
        let (id, updated) = change(vault).map_err(|e| e.to_string())?;
        let data = vault.get(id).and_then(|r| r.current().data.clone()).unwrap_or_default();
        println!("SAVED {} {}", data.title, data.username);
        std::io::stdout().flush().ok();
        Ok((id, updated))
    }
}

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let [dir, origin] = args.as_slice() else {
        eprintln!("usage: e2e_app <data-dir> <page-origin>");
        std::process::exit(2);
    };
    let mut vault = Vault::create("synthetic-master-pw-1", KdfCost::MINIMUM).unwrap();
    vault
        .add_entry(EntryData {
            title: "Local test".into(),
            username: "alice@example.test".into(),
            password: "synthetic-e2e-secret".into(),
            urls: vec![origin.clone()],
            ..Default::default()
        })
        .unwrap();
    vault
        .add_entry(EntryData {
            title: "Elsewhere".into(),
            username: "mallory@example.test".into(),
            password: "synthetic-other-site".into(),
            urls: vec!["https://example.org".into()],
            ..Default::default()
        })
        .unwrap();
    let vault = Arc::new(Mutex::new(vault));
    let handler = Arc::new(move |req: &[u8]| match parse_request(req) {
        Ok((rid, r)) => handle(&rid, r, Some(&mut vault.lock().unwrap()), &mut Announce),
        Err(e) => e,
    });
    let _server = BridgeServer::start(&LocalStore::new(dir), handler).unwrap();
    println!("READY");
    std::io::stdout().flush().ok();
    for _ in std::io::stdin().lock().lines() {}
}
