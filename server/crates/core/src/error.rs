//! Typed errors for the lanpull server.

use std::io;
use std::path::PathBuf;

/// Convenient result alias for lanpull operations.
pub type Result<T> = std::result::Result<T, Error>;

/// Process exit codes used by the `lanpull` binary.
///
/// The contract is documented in `docs/SPEC.md`:
/// - `0` success;
/// - `1` an operational failure;
/// - `2` a usage error (also produced by `clap`);
/// - `3` not initialized (missing configuration or state);
/// - `4` denied by access policy or account state;
/// - `5` busy (a lock or an already-running operation).
pub mod exit {
    /// Success.
    pub const OK: u8 = 0;
    /// Generic operational failure.
    pub const FAILURE: u8 = 1;
    /// Usage error (mirrors `clap`'s own exit code).
    pub const USAGE: u8 = 2;
    /// The server is not initialized (no configuration or state).
    pub const NOT_INITIALIZED: u8 = 3;
    /// Denied by the access policy or account state.
    pub const DENIED: u8 = 4;
    /// Busy: a lock is held or an operation is already running.
    pub const BUSY: u8 = 5;
}

/// Every error the server can report.
#[derive(Debug, thiserror::Error)]
pub enum Error {
    /// The configuration file is missing, unreadable, or invalid.
    #[error("configuration error: {0}")]
    Config(String),
    /// The command was used incorrectly (bad flag combination or missing input).
    #[error("{0}")]
    Usage(String),
    /// The operator declined an interactive confirmation.
    #[error("{0}")]
    Cancelled(String),
    /// The server has not been initialized: a required file is absent.
    #[error("not initialized: {0}")]
    NotInitialized(String),
    /// The access policy or account state denied the request.
    #[error("denied: {0}")]
    Denied(String),
    /// A lock is held or the operation is already running.
    #[error("busy: {0}")]
    Busy(String),
    /// A filesystem or I/O operation failed.
    #[error("I/O error: {0}")]
    Io(#[from] io::Error),
    /// JSON data could not be parsed or serialized.
    #[error("JSON error: {0}")]
    Json(#[from] serde_json::Error),
    /// A human duration could not be parsed.
    #[error("invalid duration: {0}")]
    Duration(String),
    /// A path was rejected as unsafe.
    #[error("unsafe path: {0}")]
    UnsafePath(String),
    /// A manifest could not be loaded or was structurally invalid.
    #[error("invalid manifest: {0}")]
    Manifest(String),
    /// Account management failed.
    #[error("account error: {0}")]
    Account(String),
    /// Network profile management failed.
    #[error("network error: {0}")]
    Network(String),
    /// Password hashing or verification failed.
    #[error("password error: {0}")]
    Password(String),
    /// Certificate generation failed.
    #[error("certificate error: {0}")]
    Certificate(String),
    /// The HTTPS server could not start or run.
    #[error("server error: {0}")]
    Server(String),
    /// A filesystem path could not be represented safely.
    #[error("bad path {path}: {reason}")]
    BadPath {
        /// The offending path.
        path: PathBuf,
        /// Why the path was rejected.
        reason: String,
    },
}

impl Error {
    /// The process exit code that corresponds to this error.
    pub const fn exit_code(&self) -> u8 {
        match self {
            Self::Usage(_) => exit::USAGE,
            Self::NotInitialized(_) => exit::NOT_INITIALIZED,
            Self::Denied(_) => exit::DENIED,
            Self::Busy(_) => exit::BUSY,
            Self::Config(_)
            | Self::Cancelled(_)
            | Self::Io(_)
            | Self::Json(_)
            | Self::Duration(_)
            | Self::UnsafePath(_)
            | Self::Manifest(_)
            | Self::Account(_)
            | Self::Network(_)
            | Self::Password(_)
            | Self::Certificate(_)
            | Self::Server(_)
            | Self::BadPath { .. } => exit::FAILURE,
        }
    }

    /// An actionable hint to print after the error, when one helps.
    pub const fn hint(&self) -> Option<&'static str> {
        match self {
            Self::Config(_) => Some("run 'lanpull config show' to inspect the configuration"),
            Self::Usage(_) => Some("run 'lanpull --help' for usage"),
            Self::NotInitialized(_) => {
                Some("run 'lanpull init' to create the configuration and state")
            }
            Self::Certificate(_) => Some("run 'lanpull cert' to generate the certificate"),
            Self::Account(_) => Some("run 'lanpull account list' to see accounts"),
            Self::Network(_) => Some("run 'lanpull network list' to see networks"),
            Self::Denied(_) => {
                Some("inspect 'lanpull account list' and 'lanpull access client list'")
            }
            _ => None,
        }
    }
}

#[cfg(test)]
mod tests {
    // Tests may use bare asserts for brevity; production code may not.
    #![allow(clippy::missing_assert_message)]

    use super::*;

    #[test]
    fn exit_codes_follow_the_contract() {
        assert_eq!(Error::Config("x".into()).exit_code(), exit::FAILURE);
        assert_eq!(Error::Usage("x".into()).exit_code(), exit::USAGE);
        assert_eq!(Error::Cancelled("x".into()).exit_code(), exit::FAILURE);
        assert_eq!(
            Error::NotInitialized("x".into()).exit_code(),
            exit::NOT_INITIALIZED
        );
        assert_eq!(Error::Denied("x".into()).exit_code(), exit::DENIED);
        assert_eq!(Error::Busy("x".into()).exit_code(), exit::BUSY);
    }

    #[test]
    fn hints_are_present_for_actionable_errors() {
        assert!(Error::NotInitialized("x".into()).hint().is_some());
        assert!(Error::Usage("x".into()).hint().is_some());
        assert!(Error::Cancelled("x".into()).hint().is_none());
        assert!(Error::Io(io::Error::other("x")).hint().is_none());
    }
}
