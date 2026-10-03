//! Server configuration.
//!
//! The configuration is a small `KEY=VALUE` file, parsed by lanpull itself and
//! also usable as a systemd `EnvironmentFile`. Blank lines and lines starting
//! with `#` are ignored, surrounding single or double quotes are stripped, and
//! values may reference environment variables with `$NAME` or `${NAME}`.
//!
//! Shares are declared with one key per share, `SHARE_<name>=<path>`; the name
//! must match `^[a-z0-9][a-z0-9_-]*$`. There is no single-share fallback.

use std::collections::BTreeMap;
use std::net::IpAddr;
use std::path::{Path, PathBuf};

use crate::error::{Error, Result};

/// The default state directory.
pub const DEFAULT_STATE_DIR: &str = "/var/lib/lanpull";
/// The default listen address.
pub const DEFAULT_BIND: &str = "0.0.0.0";
/// The default listen port.
pub const DEFAULT_PORT: u16 = 8000;
/// The default account file location, resolved relative to the config file.
pub const DEFAULT_CLIENTS_PATH: &str = "lanpull.clients";
/// The default access policy location, resolved relative to the config file.
pub const DEFAULT_ACCESS_PATH: &str = "lanpull.access.json";
/// The prefix a share key uses.
const SHARE_PREFIX: &str = "SHARE_";

/// Return `true` when `name` is a valid share name.
pub fn valid_share_name(name: &str) -> bool {
    let mut chars = name.chars();
    let Some(first) = chars.next() else {
        return false;
    };
    if !(first.is_ascii_lowercase() || first.is_ascii_digit()) {
        return false;
    }
    chars.all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '_' || c == '-')
}

/// Server configuration loaded from a `KEY=VALUE` file.
#[derive(Debug, Clone)]
pub struct Config {
    /// Named shares: share name to the directory it distributes.
    pub shares: BTreeMap<String, PathBuf>,
    /// Directory holding manifests, cache, TLS material, arm state, and log.
    pub state_dir: PathBuf,
    /// Address the server binds to.
    pub bind: IpAddr,
    /// TCP port the server listens on.
    pub port: u16,
    /// Address embedded in the certificate SAN and used to reach the server.
    pub server_ip: IpAddr,
    /// Path to the PEM certificate.
    pub cert_path: PathBuf,
    /// Path to the PEM private key.
    pub key_path: PathBuf,
    /// Path to the per-client account file.
    pub clients_path: PathBuf,
    /// Path to the per-client access mapping.
    pub access_path: PathBuf,
    /// Path to the JSON-lines audit log.
    pub audit_log: PathBuf,
}

impl Config {
    /// Load and validate a configuration file.
    ///
    /// Relative path values are resolved against the directory containing the
    /// configuration file, so a config in `config/` can reference a sibling
    /// `lanpull.clients` file by name.
    pub fn load(path: &Path) -> Result<Self> {
        let text = std::fs::read_to_string(path)
            .map_err(|e| Error::Config(format!("cannot read {}: {e}", path.display())))?;
        let map = parse_kv(&text);
        let base = path.parent().unwrap_or_else(|| Path::new("."));
        Ok(Self::from_map(&map)?.resolve(base))
    }

    /// Resolve every relative path against `base`.
    fn resolve(mut self, base: &Path) -> Self {
        self.shares = self
            .shares
            .into_iter()
            .map(|(name, path)| (name, resolve_one(base, path)))
            .collect();
        self.state_dir = resolve_one(base, self.state_dir);
        self.cert_path = resolve_one(base, self.cert_path);
        self.key_path = resolve_one(base, self.key_path);
        self.clients_path = resolve_one(base, self.clients_path);
        self.access_path = resolve_one(base, self.access_path);
        self.audit_log = resolve_one(base, self.audit_log);
        self
    }

    /// Build a configuration from parsed key/value pairs.
    pub fn from_map(map: &BTreeMap<String, String>) -> Result<Self> {
        let shares = parse_shares(map)?;
        let state_dir =
            optional_path(map, "STATE_DIR").unwrap_or_else(|| PathBuf::from(DEFAULT_STATE_DIR));

        let server_ip = required_ip(map, "SERVER_IP")?;
        let bind = match map.get("BIND") {
            Some(value) => parse_ip("BIND", value)?,
            None => parse_ip("BIND", DEFAULT_BIND)?,
        };
        let port = match map.get("PORT") {
            Some(value) => value
                .parse::<u16>()
                .map_err(|e| Error::Config(format!("PORT: {e}")))?,
            None => DEFAULT_PORT,
        };

        let cert_path =
            optional_path(map, "CERT_PATH").unwrap_or_else(|| state_dir.join("server.crt"));
        let key_path =
            optional_path(map, "KEY_PATH").unwrap_or_else(|| state_dir.join("server.key"));
        let clients_path = optional_path(map, "CLIENTS_PATH")
            .unwrap_or_else(|| PathBuf::from(DEFAULT_CLIENTS_PATH));
        let access_path =
            optional_path(map, "ACCESS_PATH").unwrap_or_else(|| PathBuf::from(DEFAULT_ACCESS_PATH));
        let audit_log =
            optional_path(map, "AUDIT_LOG").unwrap_or_else(|| state_dir.join("access.log"));

        Ok(Self {
            shares,
            state_dir,
            bind,
            port,
            server_ip,
            cert_path,
            key_path,
            clients_path,
            access_path,
            audit_log,
        })
    }

    /// Directory holding generated manifests (full and per-account).
    pub fn manifest_dir(&self) -> PathBuf {
        self.state_dir.join("manifest")
    }

    /// Path of a share's full manifest.
    pub fn manifest_path(&self, share: &str) -> PathBuf {
        self.manifest_dir().join(format!("{share}.json"))
    }

    /// Path of a share's hash cache.
    pub fn cache_path(&self, share: &str) -> PathBuf {
        self.manifest_dir().join(format!("{share}.cache.json"))
    }

    /// Directory holding per-account filtered manifests.
    pub fn access_dir(&self) -> PathBuf {
        self.manifest_dir().join("access")
    }

    /// Path of an account's filtered manifest for one share.
    pub fn access_manifest_path(&self, account: &str, share: &str) -> PathBuf {
        self.access_dir()
            .join(account)
            .join(format!("{share}.json"))
    }

    /// Path of the arm state file.
    pub fn arm_path(&self) -> PathBuf {
        self.state_dir.join("arm.json")
    }

    /// Directory holding the staged client bundle.
    pub fn bundle_dir(&self) -> PathBuf {
        self.state_dir.join("client")
    }
}

/// Collect the `SHARE_<name>=<path>` keys into a name-to-path map.
fn parse_shares(map: &BTreeMap<String, String>) -> Result<BTreeMap<String, PathBuf>> {
    let mut shares = BTreeMap::new();
    for (key, value) in map {
        let Some(name) = key.strip_prefix(SHARE_PREFIX) else {
            continue;
        };
        if !valid_share_name(name) {
            return Err(Error::Config(format!(
                "invalid share name in key {key}: must match ^[a-z0-9][a-z0-9_-]*$"
            )));
        }
        if value.is_empty() {
            return Err(Error::Config(format!("{key} is empty")));
        }
        shares.insert(name.to_string(), PathBuf::from(value));
    }
    if shares.is_empty() {
        return Err(Error::Config(
            "at least one SHARE_<name>=<path> is required".to_string(),
        ));
    }
    Ok(shares)
}

/// Parse a `KEY=VALUE` file into a map.
///
/// Environment references of the form `$NAME` and `${NAME}` are expanded.
pub fn parse_kv(text: &str) -> BTreeMap<String, String> {
    let mut map = BTreeMap::new();
    for raw in text.lines() {
        let line = raw.trim();
        if line.is_empty() || line.starts_with('#') {
            continue;
        }
        if let Some((key, value)) = line.split_once('=') {
            let key = key.trim();
            if key.is_empty() {
                continue;
            }
            let value = strip_quotes(value.trim());
            map.insert(key.to_string(), expand_env(&value));
        }
    }
    map
}

/// Remove one layer of matching surrounding quotes.
fn strip_quotes(value: &str) -> String {
    let bytes = value.as_bytes();
    let first = bytes.first().copied();
    let last = bytes.last().copied();
    if value.len() >= 2 && first == last && (first == Some(b'"') || first == Some(b'\'')) {
        value
            .get(1..value.len().saturating_sub(1))
            .unwrap_or("")
            .to_string()
    } else {
        value.to_string()
    }
}

/// Expand `$NAME` and `${NAME}` environment references.
fn expand_env(value: &str) -> String {
    let mut result = String::with_capacity(value.len());
    let mut chars = value.chars().peekable();
    while let Some(ch) = chars.next() {
        if ch != '$' {
            result.push(ch);
            continue;
        }
        let (name, braced) = if chars.peek() == Some(&'{') {
            chars.next();
            let mut name = String::new();
            for inner in chars.by_ref() {
                if inner == '}' {
                    break;
                }
                name.push(inner);
            }
            (name, true)
        } else {
            let mut name = String::new();
            while let Some(inner) = chars.peek().copied() {
                if inner.is_ascii_alphanumeric() || inner == '_' {
                    name.push(inner);
                    chars.next();
                } else {
                    break;
                }
            }
            (name, false)
        };
        if name.is_empty() {
            result.push('$');
        } else if let Ok(expanded) = std::env::var(&name) {
            result.push_str(&expanded);
        } else {
            result.push('$');
            if braced {
                result.push('{');
            }
            result.push_str(&name);
            if braced {
                result.push('}');
            }
        }
    }
    result
}

fn optional_path(map: &BTreeMap<String, String>, key: &str) -> Option<PathBuf> {
    map.get(key)
        .filter(|value| !value.is_empty())
        .map(PathBuf::from)
}

fn resolve_one(base: &Path, path: PathBuf) -> PathBuf {
    if path.is_absolute() {
        path
    } else {
        base.join(path)
    }
}

fn required_ip(map: &BTreeMap<String, String>, key: &str) -> Result<IpAddr> {
    map.get(key).map_or_else(
        || Err(Error::Config(format!("{key} is required"))),
        |value| parse_ip(key, value),
    )
}

fn parse_ip(key: &str, value: &str) -> Result<IpAddr> {
    value
        .parse::<IpAddr>()
        .map_err(|e| Error::Config(format!("{key}: {e}")))
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

    fn map(text: &str) -> BTreeMap<String, String> {
        parse_kv(text)
    }

    #[test]
    fn share_names_validated() {
        assert!(valid_share_name("reports"));
        assert!(valid_share_name("media-2026"));
        assert!(valid_share_name("a_b"));
        assert!(!valid_share_name("Reports"));
        assert!(!valid_share_name("-x"));
        assert!(!valid_share_name("a b"));
        assert!(!valid_share_name(""));
    }

    #[test]
    fn parses_multiple_shares() {
        let m = map("SHARE_reports=/srv/r\nSHARE_media=/srv/m\nSERVER_IP=127.0.0.1\n");
        let config = Config::from_map(&m).unwrap();
        assert_eq!(config.shares.len(), 2);
        assert!(config.shares.contains_key("reports"));
        assert!(config.shares.contains_key("media"));
    }

    #[test]
    fn requires_at_least_one_share() {
        let m = map("SERVER_IP=127.0.0.1\n");
        assert!(Config::from_map(&m).is_err());
    }

    #[test]
    fn rejects_bad_share_name() {
        let m = map("SHARE_Bad=/srv/x\nSERVER_IP=127.0.0.1\n");
        assert!(Config::from_map(&m).is_err());
    }

    #[test]
    fn manifest_paths_are_per_share() {
        let m = map("SHARE_reports=/srv/r\nSTATE_DIR=/state\nSERVER_IP=127.0.0.1\n");
        let config = Config::from_map(&m).unwrap();
        assert!(config
            .manifest_path("reports")
            .ends_with("manifest/reports.json"));
        assert!(config
            .cache_path("reports")
            .ends_with("manifest/reports.cache.json"));
        assert!(config
            .access_manifest_path("laptop", "reports")
            .ends_with("manifest/access/laptop/reports.json"));
    }
}
