//! Shared HTTP plumbing.

use std::time::Duration;

use serde::Deserialize;
use url::Url;

use crate::error::{Result, SyncError};

pub(crate) fn agent() -> ureq::Agent {
    ureq::AgentBuilder::new()
        .timeout_connect(Duration::from_secs(15))
        .timeout(Duration::from_secs(120))
        .user_agent(concat!("KeyRoam/", env!("CARGO_PKG_VERSION")))
        .build()
}

/// Endpoints must use HTTPS. Plain HTTP is allowed only for loopback hosts,
/// which the test-suite uses for local protocol fakes.
pub(crate) fn check_endpoint(url: &str) -> Result<()> {
    let u = Url::parse(url).map_err(|e| SyncError::NotConfigured(format!("invalid endpoint {url}: {e}")))?;
    let loopback = matches!(u.host_str(), Some("127.0.0.1") | Some("localhost") | Some("[::1]"));
    if u.scheme() == "https" || (u.scheme() == "http" && loopback) {
        Ok(())
    } else {
        Err(SyncError::NotConfigured(format!("endpoint must use https: {url}")))
    }
}

#[derive(Deserialize)]
struct GoogleApiError {
    error: GoogleApiErrorBody,
}

#[derive(Deserialize)]
struct GoogleApiErrorBody {
    #[serde(default)]
    message: String,
}

/// Convert a ureq error into a [`SyncError`], extracting Google's error message.
pub(crate) fn map_error(e: ureq::Error) -> SyncError {
    match e {
        ureq::Error::Status(status, resp) => {
            let body = resp.into_string().unwrap_or_default();
            let message = serde_json::from_str::<GoogleApiError>(&body)
                .map(|g| g.error.message)
                .unwrap_or_else(|_| body.chars().take(300).collect());
            SyncError::Http { status, message }
        }
        ureq::Error::Transport(t) => SyncError::Network(t.to_string()),
    }
}
