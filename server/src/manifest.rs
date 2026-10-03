//! Data manifest generation and loading.
//!
//! The manifest is written to `$STATE_DIR/manifest.json`, never inside the
//! share, and served virtually at `/_lanpull/manifest.json`.

use std::collections::{BTreeMap, BTreeSet};
use std::fs;
use std::os::unix::fs::MetadataExt;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};
use walkdir::WalkDir;

use crate::access::Access;
use crate::cache::Cache;
use crate::clients::Clients;
use crate::config::Config;
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

/// Filter a share manifest down to the entries an account may read.
pub fn filter(manifest: &Manifest, access: &Access, account: &str, share: &str) -> Manifest {
    let files = manifest
        .files
        .iter()
        .filter(|entry| access.allows(account, share, &entry.path))
        .cloned()
        .collect();
    Manifest {
        scheme: manifest.scheme.clone(),
        generated_at: manifest.generated_at.clone(),
        files,
    }
}

/// Summary of a manifest regeneration.
#[derive(Debug, Default)]
pub struct Regenerated {
    /// Number of files in each share's full manifest.
    pub share_files: BTreeMap<String, usize>,
    /// Number of files visible to each account, summed over its shares.
    pub account_files: BTreeMap<String, usize>,
    /// Non-fatal warnings.
    pub warnings: Vec<String>,
}

/// Regenerate every share manifest and every per-account filtered manifest.
///
/// This is the single implementation behind `make rescan` and every CLI
/// mutation that changes accounts or the access mapping.
pub fn regenerate(config: &Config) -> Result<Regenerated> {
    let access = Access::load(&config.access_path)?;
    let clients = Clients::load(&config.clients_path)?;
    let mut report = Regenerated::default();

    let mut full: BTreeMap<String, Manifest> = BTreeMap::new();
    for (share, path) in &config.shares {
        let (manifest, warnings) = generate(
            path,
            &config.manifest_path(share),
            &config.cache_path(share),
        )?;
        for path in &warnings.symlinks {
            report
                .warnings
                .push(format!("{share}: symlink skipped: {path}"));
        }
        for path in &warnings.reserved {
            report
                .warnings
                .push(format!("{share}: reserved-prefix entry ignored: {path}"));
        }
        for path in &warnings.modified_during_walk {
            report
                .warnings
                .push(format!("{share}: file modified during walk: {path}"));
        }
        report
            .share_files
            .insert(share.clone(), manifest.files.len());
        full.insert(share.clone(), manifest);
    }

    let mut expected: BTreeSet<PathBuf> = BTreeSet::new();
    for account in clients.iter() {
        if !access.has_rules(&account.name) {
            report.warnings.push(format!(
                "account {} has no access rules and can pull nothing",
                account.name
            ));
            continue;
        }
        let mut visible = 0_usize;
        for share in config.shares.keys() {
            if !access.allows_share(&account.name, share) {
                continue;
            }
            let Some(share_manifest) = full.get(share) else {
                continue;
            };
            let filtered = filter(share_manifest, &access, &account.name, share);
            visible = visible.saturating_add(filtered.files.len());
            let path = config.access_manifest_path(&account.name, share);
            if let Some(parent) = path.parent() {
                fs::create_dir_all(parent)?;
            }
            let bytes = serde_json::to_vec_pretty(&filtered)?;
            crate::atomic::write(&path, &bytes)?;
            expected.insert(path);
        }
        report.account_files.insert(account.name.clone(), visible);
    }

    clean_access_dir(config, &expected)?;
    Ok(report)
}

/// Remove per-account manifests that are no longer expected.
fn clean_access_dir(config: &Config, expected: &BTreeSet<PathBuf>) -> Result<()> {
    let dir = config.access_dir();
    if !dir.is_dir() {
        return Ok(());
    }
    let mut directories: Vec<PathBuf> = Vec::new();
    for entry in WalkDir::new(&dir).follow_links(false) {
        let entry = entry.map_err(|e| Error::Io(std::io::Error::other(e.to_string())))?;
        if entry.file_type().is_dir() {
            directories.push(entry.path().to_path_buf());
        } else if entry.file_type().is_file() && !expected.contains(entry.path()) {
            fs::remove_file(entry.path())?;
        }
    }
    directories.sort();
    for directory in directories.into_iter().rev() {
        if directory != dir && fs::read_dir(&directory)?.next().is_none() {
            fs::remove_dir(&directory)?;
        }
    }
    Ok(())
}
