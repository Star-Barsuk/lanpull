//! Network profiles.
//!
//! A profile records the server address and TLS material for one LAN, so an
//! operator who moves the server between networks can switch with
//! `lanpull network use <name>` instead of editing the configuration by hand.
//! Profiles live in `lanpull.networks.json` (`NETWORKS_PATH`, mode `0600`,
//! never committed) next to the configuration file.
//!
//! A switch never regenerates an existing certificate: each network keeps a
//! stable pinned certificate, so clients of the other networks are untouched.

use std::collections::BTreeMap;
use std::io::ErrorKind;
use std::net::IpAddr;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use crate::config::valid_network_name;
use crate::error::{Error, Result};

/// The only network-profiles schema version defined so far.
pub const VERSION: u32 = 1;

/// One network profile.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct Network {
    /// Profile name.
    pub name: String,
    /// Server address clients use; embedded in the certificate SAN.
    pub ip: IpAddr,
    /// PEM certificate for this network.
    pub cert_path: PathBuf,
    /// PEM private key for this network.
    pub key_path: PathBuf,
}

/// The on-disk form of one profile.
#[derive(Debug, Clone, Serialize, Deserialize)]
struct Entry {
    /// Server address for this network.
    ip: IpAddr,
    /// PEM certificate path.
    cert_path: PathBuf,
    /// PEM private key path.
    key_path: PathBuf,
}

/// The on-disk network-profile file.
#[derive(Debug, Serialize, Deserialize)]
struct File {
    /// Schema version.
    version: u32,
    /// Profiles, keyed by name.
    #[serde(default)]
    networks: BTreeMap<String, Entry>,
}

/// The set of network profiles, keyed by name.
#[derive(Debug, Default, Clone)]
pub struct Networks {
    entries: BTreeMap<String, Network>,
}

impl Networks {
    /// Return an empty profile set.
    pub fn new() -> Self {
        Self::default()
    }

    /// Load profiles from `path`, returning an empty set when it is absent.
    pub fn load(path: &Path) -> Result<Self> {
        let bytes = match std::fs::read(path) {
            Ok(bytes) => bytes,
            Err(e) if e.kind() == ErrorKind::NotFound => return Ok(Self::new()),
            Err(e) => return Err(e.into()),
        };
        let file: File = serde_json::from_slice(&bytes)?;
        if file.version != VERSION {
            return Err(Error::Config(format!(
                "unsupported network profiles version {}",
                file.version
            )));
        }
        let mut networks = Self::new();
        for (name, entry) in file.networks {
            if !valid_network_name(&name) {
                return Err(Error::Config(format!(
                    "invalid network name '{name}' in {}",
                    path.display()
                )));
            }
            networks.insert(Network {
                name,
                ip: entry.ip,
                cert_path: entry.cert_path,
                key_path: entry.key_path,
            });
        }
        Ok(networks)
    }

    /// Atomically write the profiles to `path` with mode `0600`.
    pub fn save(&self, path: &Path) -> Result<()> {
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)?;
        }
        let mut networks = BTreeMap::new();
        for network in self.entries.values() {
            networks.insert(
                network.name.clone(),
                Entry {
                    ip: network.ip,
                    cert_path: network.cert_path.clone(),
                    key_path: network.key_path.clone(),
                },
            );
        }
        let file = File {
            version: VERSION,
            networks,
        };
        let mut bytes = serde_json::to_vec_pretty(&file)?;
        bytes.push(b'\n');
        crate::atomic::write_private(path, &bytes)
    }

    /// Return `true` when there are no profiles.
    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }

    /// Return the number of profiles.
    pub fn len(&self) -> usize {
        self.entries.len()
    }

    /// Look up a profile by name.
    pub fn get(&self, name: &str) -> Option<&Network> {
        self.entries.get(name)
    }

    /// Insert or replace a profile.
    pub fn insert(&mut self, network: Network) {
        self.entries.insert(network.name.clone(), network);
    }

    /// Remove a profile, returning `true` when one was present.
    pub fn remove(&mut self, name: &str) -> bool {
        self.entries.remove(name).is_some()
    }

    /// Iterate over profiles in name order.
    pub fn iter(&self) -> impl Iterator<Item = &Network> {
        self.entries.values()
    }
}

#[cfg(test)]
mod tests {
    // Tests may unwrap and use bare asserts for brevity; production code may not.
    #![allow(
        clippy::unwrap_used,
        clippy::expect_used,
        clippy::panic,
        clippy::indexing_slicing,
        clippy::arithmetic_side_effects,
        clippy::missing_assert_message
    )]

    use super::*;

    fn sample() -> Network {
        Network {
            name: "home".to_string(),
            ip: "192.168.0.13".parse().unwrap(),
            cert_path: PathBuf::from("/var/lib/lanpull/net-home.crt"),
            key_path: PathBuf::from("/var/lib/lanpull/net-home.key"),
        }
    }

    #[test]
    fn missing_file_is_an_empty_set() {
        let dir = tempfile::tempdir().unwrap();
        let networks = Networks::load(&dir.path().join("absent.json")).unwrap();
        assert!(networks.is_empty());
        assert_eq!(networks.len(), 0);
    }

    #[test]
    fn round_trips_through_disk() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("lanpull.networks.json");
        let mut networks = Networks::new();
        networks.insert(sample());
        networks.save(&path).unwrap();

        let loaded = Networks::load(&path).unwrap();
        assert_eq!(loaded.get("home"), Some(&sample()));
    }

    #[test]
    fn remove_reports_presence() {
        let mut networks = Networks::new();
        networks.insert(sample());
        assert!(networks.remove("home"));
        assert!(!networks.remove("home"));
        assert!(networks.is_empty());
    }

    #[test]
    fn rejects_an_unknown_version() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("lanpull.networks.json");
        std::fs::write(&path, r#"{"version": 2, "networks": {}}"#).unwrap();
        assert!(Networks::load(&path).is_err());
    }

    #[test]
    fn rejects_an_invalid_name() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("lanpull.networks.json");
        std::fs::write(
            &path,
            r#"{"version": 1, "networks": {"Bad": {"ip": "127.0.0.1", "cert_path": "/c", "key_path": "/k"}}}"#,
        )
        .unwrap();
        assert!(Networks::load(&path).is_err());
    }
}
