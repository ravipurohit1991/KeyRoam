//! Writes a small vault with synthetic entries, used by the Flutter tests.
//!
//! cargo run -p keyroam-core --example make_fixture -- app/test/fixtures/vault.krv

use keyroam_core::{EntryData, KdfCost, Vault};

fn main() {
    let path = std::env::args().nth(1).expect("usage: make_fixture <output.krv>");
    let mut v = Vault::create("synthetic-master-pw-1", KdfCost::MINIMUM).unwrap();
    for (title, user, url) in [
        ("Example Mail", "alice@example.test", "https://mail.example.test"),
        ("Example Bank", "alice", "https://bank.example.test"),
    ] {
        v.add_entry(EntryData {
            title: title.into(),
            username: user.into(),
            password: format!("synthetic-{}", title.len()),
            urls: vec![url.into()],
            ..Default::default()
        })
        .unwrap();
    }
    std::fs::write(&path, v.to_local_file().unwrap().to_bytes().unwrap()).unwrap();
    println!("wrote {path}");
}
