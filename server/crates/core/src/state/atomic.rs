//! Atomic file writes.
//!
//! Every file lanpull generates is first written to a sibling `.tmp` file on
//! the same filesystem and then `rename()`d over the destination, so a reader
//! never observes a partially written file.

use std::fs;
use std::io::Write;
use std::os::unix::fs::OpenOptionsExt;
use std::path::{Path, PathBuf};

use crate::error::Result;

/// Build the sibling temporary path used for an atomic write.
fn tmp_path(path: &Path) -> PathBuf {
    let mut name = path.as_os_str().to_os_string();
    name.push(".tmp");
    PathBuf::from(name)
}

/// Write `bytes` to `path` atomically.
///
/// The parent directory must exist.
pub fn write(path: &Path, bytes: &[u8]) -> Result<()> {
    let tmp = tmp_path(path);
    {
        let mut file = fs::File::create(&tmp)?;
        file.write_all(bytes)?;
        file.sync_all()?;
    }
    fs::rename(tmp, path)?;
    Ok(())
}

/// Write `bytes` to `path` atomically with mode `0600`.
///
/// Used for files that hold secrets or private state.
pub fn write_private(path: &Path, bytes: &[u8]) -> Result<()> {
    let tmp = tmp_path(path);
    {
        let mut file = fs::OpenOptions::new()
            .write(true)
            .create(true)
            .truncate(true)
            .mode(0o600)
            .open(&tmp)?;
        file.write_all(bytes)?;
        file.sync_all()?;
    }
    fs::rename(tmp, path)?;
    Ok(())
}
