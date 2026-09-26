//! The `size+mtime+ctime+inode` to SHA-256 cache.
//!
//! Re-hashing a 400 MB file on every rescan is expensive, so the generator
//! reuses a cached digest while the file's metadata is unchanged. Including
//! `ctime` and `inode` defends against a same-size, same-mtime replacement that
//! `cp -p` or a coarse-mtime filesystem could otherwise hide.

use std::collections::BTreeMap;
use std::io::ErrorKind;
use std::path::Path;

use serde::{Deserialize, Serialize};

use crate::error::Result;

/// One cached digest together with the metadata that produced it.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CacheEntry {
    /// File size in bytes.
    pub size: u64,
    /// Modification time, Unix seconds.
    pub mtime: i64,
    /// Inode change time, Unix seconds.
    pub ctime: i64,
    /// Inode number.
    pub inode: u64,
    /// Cached lowercase hex SHA-256 digest.
    pub sha256: String,
}

/// The persistent hash cache.
#[derive(Debug, Default, Serialize, Deserialize)]
pub struct Cache {
    /// Map from relative path to cached entry.
    pub entries: BTreeMap<String, CacheEntry>,
}

impl Cache {
    /// Load the cache from `path`, returning an empty cache when it is absent.
    pub fn load(path: &Path) -> Result<Self> {
        match std::fs::read(path) {
            Ok(bytes) => Ok(serde_json::from_slice(&bytes)?),
            Err(e) if e.kind() == ErrorKind::NotFound => Ok(Self::default()),
            Err(e) => Err(e.into()),
        }
    }

    /// Atomically write the cache to `path`.
    pub fn save(&self, path: &Path) -> Result<()> {
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)?;
        }
        let bytes = serde_json::to_vec(self)?;
        crate::atomic::write(path, &bytes)
    }

    /// Return a cached digest when every metadata field still matches.
    pub fn lookup(&self, rel: &str, size: u64, mtime: i64, ctime: i64, inode: u64) -> Option<&str> {
        let entry = self.entries.get(rel)?;
        let matches = entry.size == size
            && entry.mtime == mtime
            && entry.ctime == ctime
            && entry.inode == inode;
        if matches {
            Some(entry.sha256.as_str())
        } else {
            None
        }
    }

    /// Store a digest and its metadata.
    pub fn insert(
        &mut self,
        rel: String,
        size: u64,
        mtime: i64,
        ctime: i64,
        inode: u64,
        sha256: String,
    ) {
        self.entries.insert(
            rel,
            CacheEntry {
                size,
                mtime,
                ctime,
                inode,
                sha256,
            },
        );
    }
}
