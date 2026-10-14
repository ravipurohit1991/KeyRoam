//! KeyRoam browser autofill.
//!
//! The browser extension never sees the vault. It talks to the native-messaging
//! host (`keyroam-native-host`), which relays each request over an
//! authenticated loopback [`bridge`] to the running desktop app. The app
//! answers with [`protocol::handle`], which releases a credential only to a
//! page of the entry's own site ([`origin`]), only while the vault is unlocked.

#![forbid(unsafe_code)]

pub mod android;
pub mod bridge;
pub mod framing;
pub mod install;
pub mod matching;
pub mod origin;
pub mod protocol;
