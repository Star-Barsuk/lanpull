//! Server configuration.
//!
//! The configuration is a small `KEY=VALUE` file, parsed by lanpull itself and
//! also usable as a systemd `EnvironmentFile`. Blank lines and lines starting
//! with `#` are ignored, surrounding single or double quotes are stripped, and
//! values may reference environment variables with `$NAME` or `${NAME}`.

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

/// Server configuration loaded from a `KEY=VALUE` file.
#[derive(Debug, Clone)]
pub struct Config {
    /// Directory whose contents are distributed to clients.
    pub share_dir: PathBuf,
    /// Directory holding manifest, cache, TLS material, arm state, and log.
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
        self.share_dir = resolve_one(base, self.share_dir);
        self.state_dir = resolve_one(base, self.state_dir);
        self.cert_path = resolve_one(base, self.cert_path);
        self.key_path = resolve_one(base, self.key_path);
        self.clients_path = resolve_one(base, self.clients_path);
        self.audit_log = resolve_one(base, self.audit_log);
        self
    }

    /// Build a configuration from parsed key/value pairs.
    pub fn from_map(map: &BTreeMap<String, String>) -> Result<Self> {
        let share_dir = required_path(map, "SHARE_DIR")?;
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
        let audit_log =
            optional_path(map, "AUDIT_LOG").unwrap_or_else(|| state_dir.join("access.log"));

        Ok(Self {
            share_dir,
            state_dir,
            bind,
            port,
            server_ip,
            cert_path,
            key_path,
            clients_path,
            audit_log,
        })
    }

    /// Path of the data manifest.
    pub fn manifest_path(&self) -> PathBuf {
        self.state_dir.join("manifest.json")
    }

    /// Path of the hash cache.
    pub fn cache_path(&self) -> PathBuf {
        self.state_dir.join("manifest.cache.json")
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

fn required_path(map: &BTreeMap<String, String>, key: &str) -> Result<PathBuf> {
    map.get(key)
        .filter(|value| !value.is_empty())
        .map(PathBuf::from)
        .ok_or_else(|| Error::Config(format!("{key} is required")))
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
