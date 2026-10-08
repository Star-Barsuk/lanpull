//! The client bundle served at `/_lanpull/client/...`.
//!
//! `make install` / `make client-bundle` stages `client/` (the `pull.py`
//! script and a `VERSION` file) into `$STATE_DIR/client`. The bundle manifest
//! lets a client compare its own version with the served one and self-update.

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
    for entry in fs::read_dir(client_dir)? {
        let entry = entry?;
        let metadata = entry.metadata()?;
        if !metadata.is_file() {
            continue;
        }
        let name = entry.file_name().to_string_lossy().to_string();
        let sha256 = sha256_file(&entry.path())?;
        files.push(BundleFile {
            path: name,
            size: metadata.len(),
            sha256,
        });
    }

    if files.is_empty() {
        return Err(missing());
    }

    files.sort_by(|a, b| a.path.cmp(&b.path));
    Ok(BundleManifest { version, files })
}

/// Return the sorted names of the regular files in a staged client bundle.
///
/// Cheaper than [`build`] for the per-file route: it lists names without
/// hashing file contents, while enforcing the same "bundle present"
/// precondition (a non-empty `VERSION` and at least one file). The name set is
/// identical to [`build`]'s, so membership checks stay equivalent.
pub fn names(client_dir: &Path) -> Result<Vec<String>> {
    if fs::read_to_string(client_dir.join("VERSION"))
        .map_err(|_| missing())?
        .trim()
        .is_empty()
    {
        return Err(missing());
    }

    let mut names: Vec<String> = Vec::new();
    for entry in fs::read_dir(client_dir)? {
        let entry = entry?;
        if entry.metadata()?.is_file() {
            names.push(entry.file_name().to_string_lossy().to_string());
        }
    }
    if names.is_empty() {
        return Err(missing());
    }
    names.sort();
    Ok(names)
}

/// Return `true` when `name` is listed in the bundle manifest.
pub fn contains(manifest: &BundleManifest, name: &str) -> bool {
    manifest.files.iter().any(|file| file.path == name)
}
