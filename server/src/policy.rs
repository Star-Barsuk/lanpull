//! Access policy: the JSON source of truth and its expansion into rules.
//!
//! `config/lanpull.access.json` (`ACCESS_PATH`, mode `0600`, never committed) is
//! the single source of the access mapping. It names, per share, a `public` set
//! that applies to every account plus per-account `add` and `remove` deltas. The
//! effective set for an account is `(public - remove) union add`; the policy is
//! expanded into the flat allow-list that the request path and manifest
//! filtering use.

use std::collections::{BTreeMap, BTreeSet};
use std::io::ErrorKind;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use crate::access::{Access, Glob, Rule};
use crate::clients::Clients;
use crate::config::{valid_share_name, Config};
use crate::error::{Error, Result};

/// The only policy schema version defined so far.
pub const VERSION: u32 = 1;

/// The whole access policy.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Policy {
    /// Schema version.
    pub version: u32,
    /// Per-share policy, keyed by share name.
    #[serde(default)]
    pub shares: BTreeMap<String, SharePolicy>,
}

/// The policy for one share.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct SharePolicy {
    /// Paths visible to every account.
    #[serde(default)]
    pub public: BTreeSet<String>,
    /// Per-account deltas.
    #[serde(default)]
    pub clients: BTreeMap<String, Deltas>,
}

/// Per-account additions and removals.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct Deltas {
    /// Paths added for this account only.
    #[serde(default)]
    pub add: BTreeSet<String>,
    /// Paths removed for this account only (including from `public`).
    #[serde(default)]
    pub remove: BTreeSet<String>,
}

impl Default for Policy {
    fn default() -> Self {
        Self {
            version: VERSION,
            shares: BTreeMap::new(),
        }
    }
}

impl Policy {
    /// Return an empty policy.
    pub fn new() -> Self {
        Self::default()
    }

    /// Load the policy from `path`, returning an empty policy when absent.
    pub fn load(path: &Path) -> Result<Self> {
        let bytes = match std::fs::read(path) {
            Ok(bytes) => bytes,
            Err(e) if e.kind() == ErrorKind::NotFound => return Ok(Self::new()),
            Err(e) => return Err(e.into()),
        };
        let policy: Self = serde_json::from_slice(&bytes)?;
        policy.validate()?;
        Ok(policy)
    }

    /// Atomically write the policy to `path` with mode `0600`.
    pub fn save(&self, path: &Path) -> Result<()> {
        self.validate()?;
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)?;
        }
        let mut bytes = serde_json::to_vec_pretty(self)?;
        bytes.push(b'\n');
        crate::atomic::write_private(path, &bytes)
    }

    /// Validate the schema version, every share name, and every path.
    pub fn validate(&self) -> Result<()> {
        if self.version != VERSION {
            return Err(Error::Config(format!(
                "unsupported access policy version {}",
                self.version
            )));
        }
        for (share, policy) in &self.shares {
            if !valid_share_name(share) {
                return Err(Error::Config(format!(
                    "invalid share name '{share}' in access policy"
                )));
            }
            for path in &policy.public {
                validate_path(share, path)?;
            }
            for deltas in policy.clients.values() {
                for path in &deltas.add {
                    validate_path(share, path)?;
                }
                for path in &deltas.remove {
                    validate_path(share, path)?;
                }
            }
        }
        Ok(())
    }

    /// Expand the policy into a flat allow-list for the given accounts.
    pub fn expand(&self, clients: &Clients) -> Result<Access> {
        let mut access = Access::new();
        for (share, policy) in &self.shares {
            for account in clients.iter() {
                for path in policy.effective(&account.name) {
                    let glob = Glob::parse(&path)?;
                    access.add_rule(
                        &account.name,
                        Rule {
                            share: Some(share.clone()),
                            glob: Some(glob),
                        },
                    );
                }
            }
        }
        Ok(access)
    }

    /// Return a mutable handle to a share's policy, creating it when absent.
    pub fn share_mut(&mut self, share: &str) -> Result<&mut SharePolicy> {
        if !valid_share_name(share) {
            return Err(Error::Config(format!("invalid share name '{share}'")));
        }
        Ok(self.shares.entry(share.to_string()).or_default())
    }

    /// Drop an account's deltas across every share, returning whether any existed.
    pub fn remove_account(&mut self, account: &str) -> bool {
        let mut removed = false;
        for policy in self.shares.values_mut() {
            if policy.clients.remove(account).is_some() {
                removed = true;
            }
        }
        // Drop now-empty share policies so the serialized form stays tidy.
        self.shares
            .retain(|_, policy| !policy.public.is_empty() || !policy.clients.is_empty());
        removed
    }
}

impl SharePolicy {
    /// The effective, sorted path set for an account.
    pub fn effective(&self, account: &str) -> BTreeSet<String> {
        let mut set = self.public.clone();
        if let Some(deltas) = self.clients.get(account) {
            for path in &deltas.remove {
                set.remove(path);
            }
            for path in &deltas.add {
                set.insert(path.clone());
            }
        }
        set
    }

    /// Record an addition for an account.
    ///
    /// A path already in `public` needs no `add` entry: clearing any earlier
    /// removal is enough. Otherwise the path joins the account's `add` set.
    pub fn add_for(&mut self, account: &str, path: String) {
        if self.public.contains(&path) {
            if let Some(deltas) = self.clients.get_mut(account) {
                deltas.remove.remove(&path);
            }
        } else {
            self.clients
                .entry(account.to_string())
                .or_default()
                .add
                .insert(path);
        }
        self.prune(account);
    }

    /// Record a removal for an account.
    ///
    /// Only a `public` path needs a `remove` entry; a personal `add` path is
    /// simply dropped. Empty delta entries are pruned.
    pub fn remove_for(&mut self, account: &str, path: &str) {
        if let Some(deltas) = self.clients.get_mut(account) {
            deltas.add.remove(path);
        }
        if self.public.contains(path) {
            self.clients
                .entry(account.to_string())
                .or_default()
                .remove
                .insert(path.to_string());
        }
        self.prune(account);
    }

    /// Drop an account's delta entry when it holds nothing.
    fn prune(&mut self, account: &str) {
        let empty = self
            .clients
            .get(account)
            .is_some_and(|deltas| deltas.add.is_empty() && deltas.remove.is_empty());
        if empty {
            self.clients.remove(account);
        }
    }
}

/// Load the policy and expand it for the configured accounts.
pub fn load_access(config: &Config) -> Result<Access> {
    let clients = Clients::load(&config.clients_path)?;
    let policy = Policy::load(&config.access_path)?;
    policy.expand(&clients)
}

/// Validate one share-relative path using the glob grammar.
fn validate_path(share: &str, path: &str) -> Result<()> {
    Glob::parse(path)
        .map_err(|e| Error::Config(format!("invalid path '{path}' in share '{share}': {e}")))?;
    Ok(())
}

/// Split a `<share>:<path>` rule spec into its parts.
pub fn split_rule(spec: &str) -> Result<(String, String)> {
    match spec.split_once(':') {
        Some((share, path)) if !share.is_empty() && !path.is_empty() => {
            Ok((share.to_string(), path.to_string()))
        }
        _ => Err(Error::Config(format!(
            "invalid rule '{spec}': expected <share>:<path>"
        ))),
    }
}

/// Resolve a policy path under a share root for existence checks.
pub fn resolve(share_root: &Path, path: &str) -> Option<PathBuf> {
    crate::relpath::join(share_root, path).ok()
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

    use std::os::unix::fs::PermissionsExt;

    use super::*;
    use crate::clients::Account;

    fn clients(names: &[&str]) -> Clients {
        let mut clients = Clients::new();
        for name in names {
            clients.insert(Account {
                name: name.to_string(),
                hash: String::new(),
                allowed_ip: None,
                local: false,
            });
        }
        clients
    }

    #[test]
    fn public_applies_to_every_account() {
        let mut policy = Policy::new();
        policy
            .share_mut("cube")
            .unwrap()
            .public
            .insert("sub/common.pdf".to_string());
        let access = policy.expand(&clients(&["alpha", "beta"])).unwrap();
        assert!(access.allows("alpha", "cube", "sub/common.pdf"));
        assert!(access.allows("beta", "cube", "sub/common.pdf"));
        assert!(!access.allows("alpha", "cube", "sub/other.pdf"));
    }

    #[test]
    fn remove_subtracts_for_one_account_only() {
        let mut policy = Policy::new();
        let share = policy.share_mut("cube").unwrap();
        share.public.insert("sub/f.pdf".to_string());
        share.remove_for("beta", "sub/f.pdf");
        let access = policy.expand(&clients(&["alpha", "beta"])).unwrap();
        assert!(access.allows("alpha", "cube", "sub/f.pdf"));
        assert!(!access.allows("beta", "cube", "sub/f.pdf"));
    }

    #[test]
    fn add_union_applies_to_one_account() {
        let mut policy = Policy::new();
        policy
            .share_mut("cube")
            .unwrap()
            .add_for("alpha", "sub/target.pdf".to_string());
        let access = policy.expand(&clients(&["alpha", "beta"])).unwrap();
        assert!(access.allows("alpha", "cube", "sub/target.pdf"));
        assert!(!access.allows("beta", "cube", "sub/target.pdf"));
    }

    #[test]
    fn removing_a_personal_add_leaves_no_delta() {
        let mut policy = Policy::new();
        let share = policy.share_mut("cube").unwrap();
        share.add_for("alpha", "sub/personal.pdf".to_string());
        assert!(share.clients.contains_key("alpha"));
        share.remove_for("alpha", "sub/personal.pdf");
        assert!(!share.clients.contains_key("alpha"));
        assert!(share.effective("alpha").is_empty());
    }

    #[test]
    fn add_and_remove_cancel_each_other() {
        let mut policy = Policy::new();
        let share = policy.share_mut("cube").unwrap();
        share.add_for("alpha", "sub/f.pdf".to_string());
        share.remove_for("alpha", "sub/f.pdf");
        assert!(share.effective("alpha").is_empty());
        share.add_for("alpha", "sub/f.pdf".to_string());
        assert!(share.effective("alpha").contains("sub/f.pdf"));
    }

    #[test]
    fn serialize_is_sorted_and_round_trips() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("lanpull.access.json");
        let mut policy = Policy::new();
        let share = policy.share_mut("cube").unwrap();
        share.public.insert("b.pdf".to_string());
        share.public.insert("a.pdf".to_string());
        share.add_for("beta", "z.pdf".to_string());
        policy.save(&path).unwrap();

        let mode = std::fs::metadata(&path).unwrap().permissions().mode() & 0o777;
        assert_eq!(mode, 0o600);
        let text = std::fs::read_to_string(&path).unwrap();
        let a = text.find("a.pdf").unwrap();
        let b = text.find("b.pdf").unwrap();
        assert!(a < b, "paths must be sorted");

        let loaded = Policy::load(&path).unwrap();
        assert_eq!(loaded.version, VERSION);
        assert!(loaded.shares["cube"].public.contains("a.pdf"));
    }

    #[test]
    fn rejects_unsafe_path() {
        let mut policy = Policy::new();
        policy
            .share_mut("cube")
            .unwrap()
            .public
            .insert("../escape".to_string());
        assert!(policy.validate().is_err());
    }

    #[test]
    fn rejects_bad_share_name() {
        let mut policy = Policy::new();
        policy.shares.insert(
            "Bad".to_string(),
            SharePolicy {
                public: BTreeSet::new(),
                clients: BTreeMap::new(),
            },
        );
        assert!(policy.validate().is_err());
    }

    #[test]
    fn missing_file_is_empty_policy() {
        let dir = tempfile::tempdir().unwrap();
        let policy = Policy::load(&dir.path().join("nope.json")).unwrap();
        assert!(policy.shares.is_empty());
    }

    #[test]
    fn remove_account_drops_deltas_and_prunes_empty_shares() {
        let mut policy = Policy::new();
        let share = policy.share_mut("cube").unwrap();
        share.public.insert("keep.pdf".to_string());
        share.add_for("alpha", "only.pdf".to_string());

        assert!(policy.remove_account("alpha"));
        assert!(!policy.shares["cube"].clients.contains_key("alpha"));
        assert!(policy.shares["cube"].public.contains("keep.pdf"));
        assert!(!policy.remove_account("alpha"));

        let mut empty = Policy::new();
        empty
            .share_mut("cube")
            .unwrap()
            .add_for("alpha", "x.pdf".to_string());
        assert!(empty.remove_account("alpha"));
        assert!(empty.shares.is_empty());
    }

    #[test]
    fn whole_share_glob_is_expressible() {
        let mut policy = Policy::new();
        policy
            .share_mut("cube")
            .unwrap()
            .public
            .insert("**".to_string());
        let access = policy.expand(&clients(&["alpha"])).unwrap();
        assert!(access.allows("alpha", "cube", "any/deep/path.pdf"));
    }
}
