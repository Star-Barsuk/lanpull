//! Live cache of on-disk server state.
//!
//! Accounts, the access policy, and arm state are read per request so that
//! `remove-client`, `passwd`, `arm`, `disarm`, and policy edits take effect
//! without a restart. Re-parsing the files on every request is wasteful, so
//! each value is cached and invalidated when its files change. The cache key is
//! `mtime + size + inode`; lanpull writes those files by atomic rename, which
//! always changes the inode, so an edit is detected even on filesystems with
//! coarse mtime resolution.

use std::fs;
use std::os::unix::fs::MetadataExt as _;
use std::path::Path;
use std::sync::{Arc, Mutex, MutexGuard, PoisonError};
use std::time::SystemTime;

use crate::access::Access;
use crate::arm::ArmState;
use crate::clients::Clients;
use crate::config::Config;
use crate::error::Result;
use crate::policy::Policy;

/// Identity of a file used to detect changes.
#[derive(Debug, Clone, PartialEq, Eq)]
struct Stamp {
    /// Last modification time, when the filesystem reports one.
    mtime: Option<SystemTime>,
    /// File size in bytes.
    len: u64,
    /// Inode number.
    inode: u64,
}

impl Stamp {
    /// Return the stamp of `path`, or `None` when it does not exist.
    fn of(path: &Path) -> Result<Option<Self>> {
        match fs::metadata(path) {
            Ok(meta) => Ok(Some(Self {
                mtime: meta.modified().ok(),
                len: meta.len(),
                inode: meta.ino(),
            })),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(None),
            Err(e) => Err(e.into()),
        }
    }
}

/// Cache key for an expanded access policy: both its inputs must be unchanged.
#[derive(Debug, Clone, PartialEq, Eq)]
struct AccessKey {
    /// Stamp of the account file.
    clients: Option<Stamp>,
    /// Stamp of the JSON policy file.
    policy: Option<Stamp>,
}

/// Cached accounts, access policy, and arm state.
#[derive(Debug, Default)]
pub struct LiveCache {
    clients: Mutex<Option<(Option<Stamp>, Arc<Clients>)>>,
    access: Mutex<Option<(AccessKey, Arc<Access>)>>,
    arm: Mutex<Option<(Option<Stamp>, Arc<ArmState>)>>,
}

impl LiveCache {
    /// Return an empty cache.
    pub fn new() -> Self {
        Self::default()
    }

    /// Return the accounts, reloading them when the file changed.
    pub fn clients(&self, path: &Path) -> Result<Arc<Clients>> {
        let stamp = Stamp::of(path)?;
        if let Some(value) = peek(&self.clients, |entry| {
            entry
                .as_ref()
                .filter(|(cached, _)| *cached == stamp)
                .map(|(_, value)| Arc::clone(value))
        }) {
            return Ok(value);
        }
        let value = Arc::new(Clients::load(path)?);
        *lock(&self.clients) = Some((stamp, Arc::clone(&value)));
        Ok(value)
    }

    /// Return the arm state, reloading it when the file changed.
    pub fn arm(&self, path: &Path) -> Result<Arc<ArmState>> {
        let stamp = Stamp::of(path)?;
        if let Some(value) = peek(&self.arm, |entry| {
            entry
                .as_ref()
                .filter(|(cached, _)| *cached == stamp)
                .map(|(_, value)| Arc::clone(value))
        }) {
            return Ok(value);
        }
        let value = Arc::new(ArmState::load(path)?);
        *lock(&self.arm) = Some((stamp, Arc::clone(&value)));
        Ok(value)
    }

    /// Return the expanded access policy, reloading when either input changed.
    pub fn access(&self, config: &Config) -> Result<Arc<Access>> {
        let key = AccessKey {
            clients: Stamp::of(&config.clients_path)?,
            policy: Stamp::of(&config.access_path)?,
        };
        if let Some(value) = peek(&self.access, |entry| {
            entry
                .as_ref()
                .filter(|(cached, _)| *cached == key)
                .map(|(_, value)| Arc::clone(value))
        }) {
            return Ok(value);
        }
        let clients = self.clients(&config.clients_path)?;
        let policy = Policy::load(&config.access_path)?;
        let value = Arc::new(policy.expand(&clients)?);
        *lock(&self.access) = Some((key, Arc::clone(&value)));
        Ok(value)
    }
}

/// Read a cached value under a short-lived lock.
fn peek<T, R>(mutex: &Mutex<T>, read: impl FnOnce(&T) -> Option<R>) -> Option<R> {
    read(&lock(mutex))
}

/// Lock a cache slot, recovering from a poisoned mutex.
fn lock<T>(mutex: &Mutex<T>) -> MutexGuard<'_, T> {
    mutex.lock().unwrap_or_else(PoisonError::into_inner)
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used, clippy::missing_assert_message)]

    use super::*;
    use crate::clients::Account;

    fn temp_clients(path: &Path) {
        let mut clients = Clients::new();
        clients.insert(Account {
            name: "alpha".to_string(),
            hash: String::new(),
            allowed_ip: None,
            local: false,
        });
        clients.save(path).unwrap();
    }

    #[test]
    fn clients_are_cached_until_the_file_changes() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("lanpull.clients");
        temp_clients(&path);

        let cache = LiveCache::new();
        let first = cache.clients(&path).unwrap();
        let again = cache.clients(&path).unwrap();
        assert!(Arc::ptr_eq(&first, &again));

        temp_clients(&path);
        let reloaded = cache.clients(&path).unwrap();
        assert!(!Arc::ptr_eq(&first, &reloaded));
    }

    #[test]
    fn missing_arm_file_is_cached_as_empty() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("arm.json");
        let cache = LiveCache::new();
        assert!(cache.arm(&path).unwrap().armed_entries(0).is_empty());
        assert!(Arc::ptr_eq(
            &cache.arm(&path).unwrap(),
            &cache.arm(&path).unwrap()
        ));
    }
}
