//! The arm window.
//!
//! An account is accepted only while it is armed. Arm state is runtime-only and
//! stored in `$STATE_DIR/arm.json`; `serve` reads it per request.

use std::collections::BTreeMap;
use std::io::ErrorKind;
use std::path::Path;

use serde::{Deserialize, Serialize};

use crate::error::Result;

/// Arm windows keyed by account name, valued by expiry time in Unix seconds.
#[derive(Debug, Default, Clone, Serialize, Deserialize)]
pub struct ArmState {
    /// Map from account name to expiry time.
    pub armed: BTreeMap<String, i64>,
}

impl ArmState {
    /// Load arm state from `path`, returning empty state when it is absent.
    pub fn load(path: &Path) -> Result<Self> {
        match std::fs::read(path) {
            Ok(bytes) => Ok(serde_json::from_slice(&bytes)?),
            Err(e) if e.kind() == ErrorKind::NotFound => Ok(Self::default()),
            Err(e) => Err(e.into()),
        }
    }

    /// Atomically write arm state to `path`.
    pub fn save(&self, path: &Path) -> Result<()> {
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)?;
        }
        let bytes = serde_json::to_vec(self)?;
        crate::atomic::write_private(path, &bytes)
    }

    /// Arm `name` until `expires_at`.
    pub fn arm(&mut self, name: &str, expires_at: i64) {
        self.armed.insert(name.to_string(), expires_at);
    }

    /// Clear the arm window for `name`, returning `true` when one existed.
    pub fn disarm(&mut self, name: &str) -> bool {
        self.armed.remove(name).is_some()
    }

    /// Return `true` when `name` is armed and not expired at `now`.
    pub fn is_armed(&self, name: &str, now: i64) -> bool {
        self.armed.get(name).is_some_and(|expiry| *expiry > now)
    }

    /// Remaining seconds for `name` at `now`, if any.
    pub fn remaining(&self, name: &str, now: i64) -> Option<i64> {
        self.armed
            .get(name)
            .filter(|expiry| **expiry > now)
            .map(|expiry| expiry.saturating_sub(now))
    }

    /// List armed accounts with their remaining seconds at `now`.
    pub fn armed_entries(&self, now: i64) -> Vec<(String, i64)> {
        self.armed
            .iter()
            .filter(|(_, expiry)| **expiry > now)
            .map(|(name, expiry)| (name.clone(), expiry.saturating_sub(now)))
            .collect()
    }

    /// Remove expired entries at `now`.
    pub fn prune(&mut self, now: i64) {
        self.armed.retain(|_, expiry| *expiry > now);
    }
}
