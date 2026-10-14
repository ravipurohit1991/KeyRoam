//! KeyRoam desktop browser autofill.
//!
//! ```text
//! browser extension ──native messaging──► keyroam-native-host ──loopback + token──► desktop app
//!                    (stdio, JSON frames)                       ([`ipc`])            (unlocked vault)
//! ```
//!
//! * [`origin`]: public-suffix-aware matching of pages to saved logins, and
//!   the rules for insecure pages and cross-origin frames.
//! * [`protocol`]: the message format and framing shared by all three parts.
//! * [`service`]: answers requests against the app's vault.
//! * [`ipc`]: the authenticated local channel from the host to the app.
//! * [`install`]: registers the host with Chrome, Chromium, Edge, Brave and Firefox.

#![forbid(unsafe_code)]

pub mod install;
pub mod ipc;
pub mod origin;
pub mod protocol;
pub mod service;
