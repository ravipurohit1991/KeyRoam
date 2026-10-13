//! KeyRoam synchronisation: Google OAuth 2.0 (PKCE, `drive.appdata` scope)
//! and the conflict-safe Google Drive `appDataFolder` sync protocol.
//!
//! Google Drive is the only cloud provider. There is no developer-operated
//! server: devices talk to Google directly and every byte uploaded is already
//! encrypted by `keyroam-core`.

#![forbid(unsafe_code)]
// `ureq::Error` is large by design; boxing it everywhere adds noise without benefit.
#![allow(clippy::result_large_err)]

pub mod drive;
pub mod engine;
pub mod error;
mod http;
pub mod oauth;
pub mod remote;
#[cfg(test)]
mod test_server;

pub use drive::DriveClient;
pub use engine::{
    commit_local_change, create_vault, fetch_remote_header, join_vault, sync, SyncReport, SyncState, SyncStatus,
};
pub use error::{Result, SyncError};
pub use oauth::{authorize_installed_app, AccessTokenSource, GoogleSession, OAuthClientConfig};
pub use remote::{RemoteFile, RemoteStore};
