use thiserror::Error;

#[derive(Debug, Error)]
pub enum SyncError {
    #[error(transparent)]
    Core(#[from] keyroam_core::Error),
    /// Could not reach Google (offline, DNS, TLS, timeout). Local use continues.
    #[error("network unavailable: {0}")]
    Network(String),
    /// Google sign-in is required again (refresh token revoked or expired).
    #[error("Google sign-in required: {0}")]
    AuthRequired(String),
    #[error("Google authorization failed: {0}")]
    Auth(String),
    #[error("Google API error {status}: {message}")]
    Http { status: u16, message: String },
    #[error("unexpected data from Google Drive: {0}")]
    Protocol(String),
    #[error("no KeyRoam vault was found in this Google account")]
    NoRemoteVault,
    #[error("this Google account already contains a KeyRoam vault; sign in to it from this device instead of creating a new one")]
    RemoteVaultExists,
    #[error("Google OAuth client is not configured: {0}")]
    NotConfigured(String),
    #[error("cancelled")]
    Cancelled,
}

impl SyncError {
    /// True when the failure is due to connectivity and should be retried later.
    pub fn is_offline(&self) -> bool {
        matches!(self, SyncError::Network(_))
    }
}

impl From<std::io::Error> for SyncError {
    fn from(e: std::io::Error) -> Self {
        SyncError::Core(e.into())
    }
}

pub type Result<T, E = SyncError> = std::result::Result<T, E>;
