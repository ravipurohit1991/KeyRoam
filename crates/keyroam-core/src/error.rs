use thiserror::Error;

/// Errors produced by the vault engine.
///
/// Messages never contain secret material.
#[derive(Debug, Error)]
pub enum Error {
    #[error("incorrect master password, or the vault key data is corrupted")]
    WrongPassword,
    #[error("encrypted data failed authentication (tampered, corrupted, or encrypted with a different key)")]
    Authentication,
    #[error("unsupported KDF parameters: {0}")]
    InvalidKdfParams(String),
    #[error("invalid vault data: {0}")]
    Format(String),
    #[error("unsupported format version {found} for {what} (this build supports up to {supported}); update KeyRoam")]
    UnsupportedVersion {
        what: &'static str,
        found: u32,
        supported: u32,
    },
    #[error("this data belongs to a different vault")]
    VaultMismatch,
    #[error("entry not found")]
    EntryNotFound,
    #[error("invalid input: {0}")]
    InvalidInput(String),
    #[error("no vault exists in this location")]
    NoVault,
    #[error("a vault already exists in this location")]
    VaultExists,
    #[error("the vault key was replaced on another device; enter your master password to continue")]
    KeyRotated,
    #[error("I/O error: {0}")]
    Io(#[from] std::io::Error),
}

impl From<serde_json::Error> for Error {
    fn from(e: serde_json::Error) -> Self {
        Error::Format(e.to_string())
    }
}

pub type Result<T, E = Error> = std::result::Result<T, E>;
