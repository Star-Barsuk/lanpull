//! Typed errors for the lanpull server.

use std::io;
use std::path::PathBuf;

/// Convenient result alias for lanpull operations.
pub type Result<T> = std::result::Result<T, Error>;

/// Every error the server can report.
#[derive(Debug, thiserror::Error)]
pub enum Error {
    /// The configuration file is missing, unreadable, or invalid.
    #[error("configuration error: {0}")]
    Config(String),
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
