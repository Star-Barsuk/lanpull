//! Atomic file writes.
//!
//! Every file lanpull generates is first written to a unique sibling temporary
//! file on the same filesystem and then `rename()`d over the destination, so a
//! reader never observes a partially written file. The temporary name carries
//! the process id and a per-process counter, and is opened with `O_EXCL`, so
//! two concurrent writers can never clobber each other's temporary file.

use std::fs;
use std::io::Write;
use std::os::unix::fs::OpenOptionsExt;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};

use crate::error::Result;

/// Per-process counter that makes sibling temporary names unique.
static TMP_COUNTER: AtomicU64 = AtomicU64::new(0);

/// Build a unique sibling temporary path used for an atomic write.
fn tmp_path(path: &Path) -> PathBuf {
    let sequence = TMP_COUNTER.fetch_add(1, Ordering::Relaxed);
    let mut name = path.as_os_str().to_os_string();
    name.push(format!(".tmp.{}.{sequence}", std::process::id()));
    PathBuf::from(name)
}

/// Write `bytes` to `path` atomically with the given creation `mode`.
///
/// The parent directory must exist. The temporary file is removed if writing
/// or renaming fails, so a failed write leaves no residue behind.
fn write_with_mode(path: &Path, bytes: &[u8], mode: u32) -> Result<()> {
    let tmp = tmp_path(path);
    let write = || -> Result<()> {
        let mut file = fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .mode(mode)
            .open(&tmp)?;
        file.write_all(bytes)?;
        file.sync_all()?;
        Ok(())
    };
    if let Err(e) = write() {
        let _ = fs::remove_file(&tmp);
        return Err(e);
    }
    if let Err(e) = fs::rename(&tmp, path) {
        let _ = fs::remove_file(&tmp);
        return Err(e.into());
    }
    Ok(())
}

/// Write `bytes` to `path` atomically.
///
/// The parent directory must exist.
pub fn write(path: &Path, bytes: &[u8]) -> Result<()> {
    write_with_mode(path, bytes, 0o666)
}

/// Write `bytes` to `path` atomically with mode `0600`.
///
/// Used for files that hold secrets or private state.
pub fn write_private(path: &Path, bytes: &[u8]) -> Result<()> {
    write_with_mode(path, bytes, 0o600)
}
