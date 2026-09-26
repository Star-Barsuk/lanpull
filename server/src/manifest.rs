//! Data manifest generation and loading.
//!
//! The manifest is written to `$STATE_DIR/manifest.json`, never inside the
//! share, and served virtually at `/_lanpull/manifest.json`.

use std::fs;
use std::os::unix::fs::MetadataExt;
use std::path::Path;

use serde::{Deserialize, Serialize};
use walkdir::WalkDir;

use crate::cache::Cache;
use crate::error::{Error, Result};
use crate::hash::sha256_file;
use crate::ignore::is_ignored;
use crate::relpath;
use crate::timeutil;

/// The only transfer scheme defined so far.
pub const SCHEME: &str = "whole-file-v1";

/// One distributed file.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Entry {
    /// Path relative to the share root, `/`-separated.
    pub path: String,
    /// Size in bytes.
    pub size: u64,
    /// Modification time, Unix seconds.
    pub mtime: i64,
    /// Lowercase hex SHA-256 digest.
    pub sha256: String,
}

/// The data manifest served to clients.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Manifest {
    /// Transfer scheme name.
    pub scheme: String,
    /// Generation time as an RFC 3339 UTC timestamp (informational).
    pub generated_at: String,
    /// Distributed files, sorted by path.
    pub files: Vec<Entry>,
}

/// Non-fatal problems found while generating a manifest.
#[derive(Debug, Default, Clone)]
pub struct Warnings {
    /// Symlink entries that were skipped.
    pub symlinks: Vec<String>,
    /// Entries under the reserved `_lanpull/` prefix that were skipped.
    pub reserved: Vec<String>,
    /// Files whose modification time changed while they were hashed.
    pub modified_during_walk: Vec<String>,
}

impl Warnings {
    /// Return `true` when there is nothing to report.
    pub const fn is_empty(&self) -> bool {
        self.symlinks.is_empty() && self.reserved.is_empty() && self.modified_during_walk.is_empty()
    }
}

impl Manifest {
    /// Load a manifest from disk.
    pub fn load(path: &Path) -> Result<Self> {
        let bytes = fs::read(path)?;
        let manifest: Self =
            serde_json::from_slice(&bytes).map_err(|e| Error::Manifest(e.to_string()))?;
        if manifest.scheme != SCHEME {
            return Err(Error::Manifest(format!(
                "unsupported scheme {}",
                manifest.scheme
            )));
        }
        Ok(manifest)
    }
}

/// Generate a manifest from the share directory.
///
/// The manifest and the hash cache are written atomically. Warnings are
/// returned rather than logged so the caller controls the output.
pub fn generate(
    share: &Path,
    manifest_path: &Path,
    cache_path: &Path,
) -> Result<(Manifest, Warnings)> {
    let cache = Cache::load(cache_path)?;
    let mut next_cache = Cache::default();
    let mut files: Vec<Entry> = Vec::new();
    let mut warnings = Warnings::default();

    for entry in WalkDir::new(share).follow_links(false) {
        let dir_entry = entry.map_err(|e| Error::Io(std::io::Error::other(e.to_string())))?;
        let file_type = dir_entry.file_type();
        if file_type.is_dir() {
            continue;
        }

        let rel = relpath::to_relative_string(share, dir_entry.path())?;

        if file_type.is_symlink() {
            warnings.symlinks.push(rel);
            continue;
        }
        if relpath::is_reserved(&rel) {
            warnings.reserved.push(rel);
            continue;
        }
        if is_ignored(&rel) {
            continue;
        }

        let metadata = fs::symlink_metadata(dir_entry.path())?;
        let size = metadata.len();
        let mtime = metadata.mtime();
        let ctime = metadata.ctime();
        let inode = metadata.ino();

        let sha256 = match cache.lookup(&rel, size, mtime, ctime, inode) {
            Some(cached) => cached.to_string(),
            None => sha256_file(dir_entry.path())?,
        };

        let after = fs::symlink_metadata(dir_entry.path())?;
        if after.mtime() != mtime {
            warnings.modified_during_walk.push(rel.clone());
        }

        next_cache.insert(rel.clone(), size, mtime, ctime, inode, sha256.clone());
        files.push(Entry {
            path: rel,
            size,
            mtime,
            sha256,
        });
    }

    files.sort_by(|a, b| a.path.cmp(&b.path));

    let manifest = Manifest {
        scheme: SCHEME.to_string(),
        generated_at: timeutil::iso8601(timeutil::now_unix()),
        files,
    };

    if let Some(parent) = manifest_path.parent() {
        fs::create_dir_all(parent)?;
    }
    let bytes = serde_json::to_vec_pretty(&manifest)?;
    crate::atomic::write(manifest_path, &bytes)?;
    next_cache.save(cache_path)?;

    Ok((manifest, warnings))
}
