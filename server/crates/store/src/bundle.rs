//! The client bundle served at `/_lanpull/client/...`.
//!
//! `make install` / `make client-bundle` stages `client/` (the `pull.py`
//! script and a `VERSION` file) into `$STATE_DIR/client`. The bundle manifest
//! lets a client compare its own version with the served one and self-update.
//!
//! The served set is closed: only [`ALLOWED_FILES`] is ever listed or served.
//! A client replaces every file named by the bundle manifest in its own
//! folder, so serving whatever happens to be staged would let a stray `auth`,
//! `lanpull.conf`, or `server.crt` overwrite the client's configuration.

use std::fs;
use std::path::Path;

use serde::{Deserialize, Serialize};

use lanpull_core::error::{Error, Result};
use lanpull_core::hash::sha256_file;

/// One file in the client bundle.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct BundleFile {
    /// File name within the bundle directory.
    pub path: String,
    /// Size in bytes.
    pub size: u64,
    /// Lowercase hex SHA-256 digest.
    pub sha256: String,
}

/// The client bundle manifest.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct BundleManifest {
    /// Contents of the staged `VERSION` file, trimmed.
    pub version: String,
    /// Bundle files, sorted by name.
    pub files: Vec<BundleFile>,
}

/// The only files the client bundle may contain and distribute.
pub const ALLOWED_FILES: &[&str] = &["pull.py", "VERSION"];

/// Return `true` when `name` is a distributable bundle file.
pub fn is_allowed(name: &str) -> bool {
    ALLOWED_FILES.contains(&name)
}

/// The error for a missing or empty client bundle.
fn missing() -> Error {
    Error::Server(
        "server has no client bundle; ask the operator to reinstall or run 'make client-bundle'"
            .to_string(),
    )
}

/// Build the bundle manifest from a staged client directory.
pub fn build(client_dir: &Path) -> Result<BundleManifest> {
    let version = fs::read_to_string(client_dir.join("VERSION"))
        .map_err(|_| missing())?
        .trim()
        .to_string();
    if version.is_empty() {
        return Err(missing());
    }

    let mut files: Vec<BundleFile> = Vec::new();
    for name in ALLOWED_FILES {
        let path = client_dir.join(name);
        let Ok(metadata) = fs::symlink_metadata(&path) else {
            continue;
        };
        if !metadata.is_file() || metadata.file_type().is_symlink() {
            continue;
        }
        files.push(BundleFile {
            path: (*name).to_string(),
            size: metadata.len(),
            sha256: sha256_file(&path)?,
        });
    }

    if files.is_empty() {
        return Err(missing());
    }

    files.sort_by(|a, b| a.path.cmp(&b.path));
    Ok(BundleManifest { version, files })
}

/// Return `true` when `name` is listed in the bundle manifest.
pub fn contains(manifest: &BundleManifest, name: &str) -> bool {
    manifest.files.iter().any(|file| file.path == name)
}
