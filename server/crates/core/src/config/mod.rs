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
use std::path::{Component, Path, PathBuf};

use crate::error::{Error, Result};

/// The default state directory.
pub const DEFAULT_STATE_DIR: &str = "/var/lib/lanpull";
/// The canonical configuration directory, outside the repository.
pub const DEFAULT_CONFIG_DIR: &str = "/etc/lanpull";
/// The canonical configuration file path.
pub const DEFAULT_CONFIG_PATH: &str = "/etc/lanpull/lanpull.conf";
/// The environment variable that overrides the configuration path.
pub const CONFIG_ENV: &str = "LANPULL_CONFIG";
/// The default listen address.
pub const DEFAULT_BIND: &str = "0.0.0.0";
/// The default listen port.
pub const DEFAULT_PORT: u16 = 8000;
/// The default account file location, resolved relative to the config file.
pub const DEFAULT_CLIENTS_PATH: &str = "lanpull.clients";
/// The default access policy location, resolved relative to the config file.
pub const DEFAULT_ACCESS_PATH: &str = "lanpull.access.json";
/// The default network-profiles location, resolved relative to the config file.
pub const DEFAULT_NETWORKS_PATH: &str = "lanpull.networks.json";
/// The default certificate file name inside the state directory.
pub const CERT_FILE_NAME: &str = "server.crt";
/// The default private-key file name inside the state directory.
pub const KEY_FILE_NAME: &str = "server.key";
/// The pattern a valid share name must match.
pub const SHARE_NAME_PATTERN: &str = "^[a-z0-9][a-z0-9_-]*$";
/// The prefix a share key uses.
const SHARE_KEY_PREFIX: &str = "SHARE_";

/// Return `true` when `name` is a valid share name.
pub fn valid_share_name(name: &str) -> bool {
    valid_name(name)
}

/// Return `true` when `name` is a valid network profile name.
pub fn valid_network_name(name: &str) -> bool {
    valid_name(name)
}

/// Shared identifier grammar for shares and network profiles.
fn valid_name(name: &str) -> bool {
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
    /// Path to the JSON network-profile file.
    pub networks_path: PathBuf,
    /// Path to the JSON-lines audit log.
    pub audit_log: PathBuf,
}

/// Resolve the configuration path from an explicit flag and the environment.
///
/// A path passed explicitly (or in `$LANPULL_CONFIG`) must exist: a typo in an
/// explicit path is an error, not a fallback. Only when neither is set does the
/// function probe the canonical location and then the in-repo development path.
pub fn resolve_path(explicit: Option<&Path>) -> Result<PathBuf> {
    if let Some(path) = explicit {
        if path.is_file() {
            return Ok(path.to_path_buf());
        }
        return Err(Error::NotInitialized(format!(
            "configuration not found at {}",
            path.display()
        )));
    }
    if let Some(path) = std::env::var_os(CONFIG_ENV) {
        let path = PathBuf::from(path);
        if path.is_file() {
            return Ok(path);
        }
        return Err(Error::NotInitialized(format!(
            "configuration not found at {} (from ${CONFIG_ENV})",
            path.display()
        )));
    }
    let canonical = PathBuf::from(DEFAULT_CONFIG_PATH);
    if canonical.is_file() {
        return Ok(canonical);
    }
    let development = PathBuf::from("config/lanpull.conf");
    if development.is_file() {
        return Ok(development);
    }
    Err(Error::NotInitialized(format!(
        "no configuration found; checked {} and {}",
        canonical.display(),
        development.display()
    )))
}

impl Config {
    /// Load and validate a configuration file.
    ///
    /// Relative path values are resolved against the directory containing the
    /// configuration file, so a config in `config/` can reference a sibling
    /// `lanpull.clients` file by name.
    pub fn load(path: &Path) -> Result<Self> {
        let text = std::fs::read_to_string(path).map_err(|e| {
            if e.kind() == std::io::ErrorKind::NotFound {
                Error::NotInitialized(format!("configuration not found at {}", path.display()))
            } else {
                Error::Config(format!("cannot read {}: {e}", path.display()))
            }
        })?;
        let base = path.parent().unwrap_or_else(|| Path::new("."));
        Self::parse(&text, base)
    }

    /// Parse and validate configuration `text` as if it were at `base`.
    ///
    /// This is the validation half of [`Config::load`], without touching the
    /// filesystem, so an editor can reject an invalid prospective file before
    /// it is written.
    pub fn parse(text: &str, base: &Path) -> Result<Self> {
        let map = parse_kv(text);
        let config = Self::from_map(&map)?.resolve(base);
        config.validate_isolation()?;
        Ok(config)
    }

    /// Reject internal state that would be published as share content.
    ///
    /// Nothing lanpull generates (manifests, TLS key, policy, accounts, audit
    /// log) may live under a share root: the manifest walk would otherwise
    /// include it and serve it to clients. Paths are compared lexically after
    /// normalization, so the check does not require the files to exist.
    fn validate_isolation(&self) -> Result<()> {
        let shares: Vec<(&str, PathBuf)> = self
            .shares
            .iter()
            .map(|(name, root)| (name.as_str(), normalize_path(root)))
            .collect();
        let guarded = [
            ("STATE_DIR", &self.state_dir),
            ("CERT_PATH", &self.cert_path),
            ("KEY_PATH", &self.key_path),
            ("CLIENTS_PATH", &self.clients_path),
            ("ACCESS_PATH", &self.access_path),
            ("NETWORKS_PATH", &self.networks_path),
            ("AUDIT_LOG", &self.audit_log),
        ];
        for (label, path) in guarded {
            let sensitive = normalize_path(path);
            for (name, root) in &shares {
                if sensitive.starts_with(root) {
                    return Err(Error::Config(format!(
                        "{label} {} is inside share '{name}' ({}); keep internal state outside every share root",
                        path.display(),
                        root.display()
                    )));
                }
            }
        }
        Ok(())
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
        self.networks_path = resolve_one(base, self.networks_path);
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
            optional_path(map, "CERT_PATH").unwrap_or_else(|| state_dir.join(CERT_FILE_NAME));
        let key_path =
            optional_path(map, "KEY_PATH").unwrap_or_else(|| state_dir.join(KEY_FILE_NAME));
        let clients_path = optional_path(map, "CLIENTS_PATH")
            .unwrap_or_else(|| PathBuf::from(DEFAULT_CLIENTS_PATH));
        let access_path =
            optional_path(map, "ACCESS_PATH").unwrap_or_else(|| PathBuf::from(DEFAULT_ACCESS_PATH));
        let networks_path = optional_path(map, "NETWORKS_PATH")
            .unwrap_or_else(|| PathBuf::from(DEFAULT_NETWORKS_PATH));
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
            networks_path,
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
        let Some(name) = key.strip_prefix(SHARE_KEY_PREFIX) else {
            continue;
        };
        if !valid_share_name(name) {
            return Err(Error::Config(format!(
                "invalid share name in key {key}: must match {SHARE_NAME_PATTERN}"
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

/// Absolutely resolve `.`/`..` components without touching the filesystem.
///
/// Used only for the containment check, so a symlinked intermediate directory
/// is compared on its lexical path (a false negative there is not a safety
/// issue: the share walk itself follows no symlinks).
fn normalize_path(path: &Path) -> PathBuf {
    let absolute = std::path::absolute(path).unwrap_or_else(|_| path.to_path_buf());
    let mut normalized = PathBuf::new();
    for component in absolute.components() {
        match component {
            Component::CurDir => {}
            Component::ParentDir => {
                normalized.pop();
            }
            other => normalized.push(other.as_os_str()),
        }
    }
    normalized
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
    fn network_names_share_the_identifier_grammar() {
        assert!(valid_network_name("home"));
        assert!(valid_network_name("office-2"));
        assert!(!valid_network_name("Home"));
        assert!(!valid_network_name(""));
    }

    #[test]
    fn networks_path_defaults_next_to_the_config() {
        let base = Path::new("/etc/lanpull");
        let config = Config::parse("SHARE_reports=/srv/r\nSERVER_IP=127.0.0.1\n", base).unwrap();
        assert_eq!(
            config.networks_path,
            PathBuf::from("/etc/lanpull/lanpull.networks.json")
        );
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
    fn parse_validates_without_a_file() {
        let base = Path::new("/etc/lanpull");
        let config = Config::parse("SHARE_reports=/srv/r\nSERVER_IP=127.0.0.1\n", base).unwrap();
        assert_eq!(config.shares["reports"], PathBuf::from("/srv/r"));

        let err = Config::parse("SERVER_IP=127.0.0.1\n", base).unwrap_err();
        assert!(matches!(err, Error::Config(_)), "got {err}");
    }

    #[test]
    fn rejects_bad_share_name() {
        let m = map("SHARE_Bad=/srv/x\nSERVER_IP=127.0.0.1\n");
        assert!(Config::from_map(&m).is_err());
    }

    #[test]
    fn rejects_state_dir_inside_a_share() {
        let dir = tempfile::tempdir().unwrap();
        let share = dir.path().join("share");
        let conf = dir.path().join("lanpull.conf");
        std::fs::write(
            &conf,
            format!(
                "SHARE_reports={}\nSTATE_DIR={}\nSERVER_IP=127.0.0.1\n",
                share.display(),
                share.join("state").display()
            ),
        )
        .unwrap();
        assert!(Config::load(&conf).is_err());
    }

    #[test]
    fn accepts_state_dir_outside_every_share() {
        let dir = tempfile::tempdir().unwrap();
        let conf = dir.path().join("lanpull.conf");
        std::fs::write(
            &conf,
            format!(
                "SHARE_reports={}\nSTATE_DIR={}\nSERVER_IP=127.0.0.1\n",
                dir.path().join("share").display(),
                dir.path().join("state").display()
            ),
        )
        .unwrap();
        assert!(Config::load(&conf).is_ok());
    }

    #[test]
    fn resolve_path_prefers_explicit_and_errors_when_missing() {
        let dir = tempfile::tempdir().unwrap();
        let conf = dir.path().join("lanpull.conf");
        std::fs::write(&conf, "SHARE_reports=/srv/r\nSERVER_IP=127.0.0.1\n").unwrap();

        assert_eq!(resolve_path(Some(&conf)).unwrap(), conf);

        let missing = dir.path().join("absent.conf");
        let err = resolve_path(Some(&missing)).unwrap_err();
        assert!(matches!(err, Error::NotInitialized(_)), "got {err}");
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
