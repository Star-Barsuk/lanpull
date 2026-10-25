//! Access policy: the JSON source of truth and its expansion into rules.
//!
//! `config/lanpull.access.json` (`ACCESS_PATH`, mode `0600`, never committed) is
//! the single source of the access mapping. It names, per share, a `public` set
//! that applies to every account plus per-account `add` and `remove` deltas. The
//! effective set for an account is `(public - remove) union add`; the policy is
//! expanded into the flat allow-list that the request path and manifest
//! filtering use.

pub mod access;

use std::collections::{BTreeMap, BTreeSet};
use std::io::ErrorKind;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use crate::access::{Access, Rule};
use crate::clients::Clients;
use crate::config::{valid_share_name, Config};
use crate::error::{Error, Result};
use crate::glob::Glob;

/// The current policy schema version.
pub const VERSION: u32 = 2;

/// The oldest policy schema version still accepted on load.
pub const MIN_VERSION: u32 = 1;

/// A named group of accounts (possibly containing other groups).
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct Group {
    /// Account or group names that belong to this group.
    #[serde(default)]
    pub members: BTreeSet<String>,
}

/// The whole access policy.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Policy {
    /// Schema version.
    pub version: u32,
    /// Named groups, keyed by group name.
    #[serde(default)]
    pub groups: BTreeMap<String, Group>,
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
    /// Per-group deltas.
    #[serde(default)]
    pub groups: BTreeMap<String, Deltas>,
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
            groups: BTreeMap::new(),
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

    /// Validate the schema version, every share name, every group, and every path.
    pub fn validate(&self) -> Result<()> {
        if self.version != VERSION && self.version != MIN_VERSION {
            return Err(Error::Config(format!(
                "unsupported access policy version {}",
                self.version
            )));
        }
        for name in self.groups.keys() {
            if !valid_share_name(name) {
                return Err(Error::Config(format!(
                    "invalid group name '{name}' in access policy"
                )));
            }
        }
        // Reject cyclic group membership before it can break expansion.
        let mut visiting = BTreeSet::new();
        let mut visited = BTreeSet::new();
        for name in self.groups.keys() {
            Self::detect_group_cycle(name, &self.groups, &mut visiting, &mut visited)?;
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
            for deltas in policy.groups.values() {
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

    /// Depth-first cycle detection over group membership.
    fn detect_group_cycle(
        name: &str,
        groups: &BTreeMap<String, Group>,
        visiting: &mut BTreeSet<String>,
        visited: &mut BTreeSet<String>,
    ) -> Result<()> {
        if visited.contains(name) || !groups.contains_key(name) {
            return Ok(());
        }
        if !visiting.insert(name.to_string()) {
            return Err(Error::Config(format!(
                "cyclic group membership at '{name}'"
            )));
        }
        if let Some(group) = groups.get(name) {
            for member in &group.members {
                if groups.contains_key(member) {
                    Self::detect_group_cycle(member, groups, visiting, visited)?;
                }
            }
        }
        visiting.remove(name);
        visited.insert(name.to_string());
        Ok(())
    }

    /// Expand the policy into a flat allow-list for the given accounts.
    ///
    /// Group membership is resolved here, so [`Access`] stays keyed by
    /// account and the request path needs no group awareness.
    pub fn expand(&self, clients: &Clients) -> Result<Access> {
        let mut access = Access::new();
        for (share, policy) in &self.shares {
            for account in clients.iter() {
                let (allowed, denied) = policy.effective(&account.name, &self.groups);
                for path in allowed {
                    let glob = Glob::parse(&path)?;
                    access.add_rule(
                        &account.name,
                        Rule {
                            share: Some(share.clone()),
                            glob: Some(glob),
                        },
                    );
                }
                for path in denied {
                    let glob = Glob::parse(&path)?;
                    access.deny_rule(
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

    /// Return `true` when `account` would see at least one path once created.
    ///
    /// Unlike [`Policy::expand`], this does not require the account to exist in
    /// the account file: it answers the question `account add` asks before the
    /// account is written, so a `public` rule grants a brand-new account access.
    /// Group membership is taken into account when the account already belongs
    /// to a group.
    pub fn has_effective_rules(&self, account: &str) -> bool {
        self.shares
            .values()
            .any(|share| !share.effective(account, &self.groups).0.is_empty())
    }

    /// Drop an account's deltas across every share, returning whether any existed.
    ///
    /// The account is also removed from every group's membership list.
    pub fn remove_account(&mut self, account: &str) -> bool {
        let mut removed = false;
        for policy in self.shares.values_mut() {
            if policy.clients.remove(account).is_some() {
                removed = true;
            }
        }
        for group in self.groups.values_mut() {
            group.members.remove(account);
        }
        // Drop now-empty share policies so the serialized form stays tidy.
        self.shares.retain(|_, policy| {
            !policy.public.is_empty() || !policy.clients.is_empty() || !policy.groups.is_empty()
        });
        removed
    }

    /// Drop a group everywhere, returning whether it existed.
    pub fn remove_group(&mut self, name: &str) -> bool {
        let removed = self.groups.remove(name).is_some();
        for policy in self.shares.values_mut() {
            policy.groups.remove(name);
        }
        for group in self.groups.values_mut() {
            group.members.remove(name);
        }
        // Drop now-empty share policies so the serialized form stays tidy.
        self.shares.retain(|_, policy| {
            !policy.public.is_empty() || !policy.clients.is_empty() || !policy.groups.is_empty()
        });
        removed
    }

    /// Return the group names that contain `account` (directly or transitively).
    pub fn groups_of(&self, account: &str) -> Vec<String> {
        let mut result = BTreeSet::new();
        for name in self.groups.keys().cloned() {
            if self.group_contains(&name, account, &mut BTreeSet::new()) {
                result.insert(name);
            }
        }
        result.into_iter().collect()
    }

    /// Return `true` when `group` contains `target` directly or transitively.
    fn group_contains(&self, group: &str, target: &str, visited: &mut BTreeSet<String>) -> bool {
        if !visited.insert(group.to_string()) {
            return false;
        }
        if let Some(def) = self.groups.get(group) {
            if def.members.contains(target) {
                return true;
            }
            for member in &def.members {
                if self.groups.contains_key(member) && self.group_contains(member, target, visited)
                {
                    return true;
                }
            }
        }
        false
    }

    /// Return all (transitively) member account names of a group.
    pub fn members_of(&self, group: &str) -> BTreeSet<String> {
        let mut result = BTreeSet::new();
        let mut visited = BTreeSet::new();
        self.collect_members(group, &mut result, &mut visited);
        result
    }

    /// Recursive member collection.
    fn collect_members(
        &self,
        group: &str,
        result: &mut BTreeSet<String>,
        visited: &mut BTreeSet<String>,
    ) {
        if !visited.insert(group.to_string()) {
            return;
        }
        if let Some(def) = self.groups.get(group) {
            for member in &def.members {
                if self.groups.contains_key(member) {
                    self.collect_members(member, result, visited);
                } else {
                    result.insert(member.clone());
                }
            }
        }
    }

    /// Read-only access to all groups.
    pub const fn groups(&self) -> &BTreeMap<String, Group> {
        &self.groups
    }

    /// Revoke `path` for `principal` on `share`, creating a deny override when
    /// the path is still reachable through another source (public or a group).
    pub fn revoke(&mut self, share: &str, principal: &str, path: &str, group: bool) -> Result<()> {
        let groups = self.groups.clone();
        let share_policy = self.share_mut(share)?;
        // Always drop an earlier explicit grant.
        if group {
            if let Some(deltas) = share_policy.groups.get_mut(principal) {
                deltas.add.remove(path);
            }
        } else if let Some(deltas) = share_policy.clients.get_mut(principal) {
            deltas.add.remove(path);
        }
        let currently_allowed = share_policy
            .effective(principal, &groups)
            .0
            .iter()
            .any(|rule| Glob::parse(rule).is_ok_and(|glob| glob.covers(path)));
        if currently_allowed {
            let deltas = if group {
                share_policy
                    .groups
                    .entry(principal.to_string())
                    .or_default()
            } else {
                share_policy
                    .clients
                    .entry(principal.to_string())
                    .or_default()
            };
            deltas.remove.insert(path.to_string());
            // Prune empty entries.
            let empty = deltas.add.is_empty() && deltas.remove.is_empty();
            // Note: cannot remove here easily after borrow; handle below.
            let _ = empty;
        }
        // Prune empty entries for this principal.
        if group {
            let empty = share_policy
                .groups
                .get(principal)
                .is_some_and(|d| d.add.is_empty() && d.remove.is_empty());
            if empty {
                share_policy.groups.remove(principal);
            }
        } else {
            let empty = share_policy
                .clients
                .get(principal)
                .is_some_and(|d| d.add.is_empty() && d.remove.is_empty());
            if empty {
                share_policy.clients.remove(principal);
            }
        }
        Ok(())
    }

    /// Mutable access to a group, creating it when absent.
    pub fn group_mut(&mut self, name: &str) -> Result<&mut Group> {
        if !valid_share_name(name) {
            return Err(Error::Config(format!("invalid group name '{name}'")));
        }
        Ok(self.groups.entry(name.to_string()).or_default())
    }
}

impl SharePolicy {
    /// The allow globs for an account: `public` minus exact-path removals, plus
    /// the account's `add` entries.
    ///
    /// A `remove` that targets a path covered only by a glob in `public` cannot
    /// be represented as a static set difference; it is returned by [`SharePolicy::denied`]
    /// and enforced at match time by [`crate::access::Access`]. Exact removals
    /// of an exact `public` entry are subtracted here as before.
    pub fn allowed(&self, account: &str) -> BTreeSet<String> {
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

    /// The deny globs for an account (its `remove` entries).
    pub fn denied(&self, account: &str) -> BTreeSet<String> {
        self.clients
            .get(account)
            .map_or_else(BTreeSet::new, |deltas| deltas.remove.clone())
    }

    /// Record an addition for an account.
    pub fn add_for(&mut self, account: &str, path: String) {
        self.add_for_principal(account, path, false);
    }

    /// Record an addition for a group.
    pub fn add_for_group(&mut self, group: &str, path: String) {
        self.add_for_principal(group, path, true);
    }

    /// Shared implementation for account and group additions.
    fn add_for_principal(&mut self, principal: &str, path: String, group: bool) {
        let covered = self.public.contains(&path) || self.public_covers(&path);
        let deltas = self.deltas_mut(principal, group);
        // A grant clears any earlier exclusion of the same path.
        deltas.remove.remove(&path);
        if !covered {
            deltas.add.insert(path);
        }
        self.prune_principal(principal, group);
    }

    /// Record a removal for an account.
    pub fn remove_for(&mut self, account: &str, path: &str) {
        self.remove_for_principal(account, path, false);
    }

    /// Record a removal for a group.
    pub fn remove_for_group(&mut self, group: &str, path: &str) {
        self.remove_for_principal(group, path, true);
    }

    /// Shared implementation for account and group removals.
    ///
    /// A removal always drops an earlier explicit `add`. It creates a deny
    /// entry only when the path is still reachable through `public` (or a
    /// covering glob); paths reachable only through another principal's grant
    /// are handled by the caller, which can pass the broader context.
    fn remove_for_principal(&mut self, principal: &str, path: &str, group: bool) {
        self.deltas_mut(principal, group).add.remove(path);
        if self.public.contains(path) || self.public_covers(path) {
            self.deltas_mut(principal, group)
                .remove
                .insert(path.to_string());
        }
        self.prune_principal(principal, group);
    }

    /// Mutable access to a principal's deltas.
    fn deltas_mut(&mut self, principal: &str, group: bool) -> &mut Deltas {
        if group {
            self.groups.entry(principal.to_string()).or_default()
        } else {
            self.clients.entry(principal.to_string()).or_default()
        }
    }

    /// Prune a principal's empty delta entry.
    fn prune_principal(&mut self, principal: &str, group: bool) {
        let empty = if group {
            self.groups
                .get(principal)
                .is_some_and(|deltas| deltas.add.is_empty() && deltas.remove.is_empty())
        } else {
            self.clients
                .get(principal)
                .is_some_and(|deltas| deltas.add.is_empty() && deltas.remove.is_empty())
        };
        if empty {
            if group {
                self.groups.remove(principal);
            } else {
                self.clients.remove(principal);
            }
        }
    }

    /// Effective allow and deny sets for a principal, including inherited groups.
    ///
    /// `groups` is the top-level group map used to resolve membership. Group
    /// membership is expanded transitively; a cycle is already rejected by
    /// [`Policy::validate`].
    pub fn effective(
        &self,
        principal: &str,
        groups: &BTreeMap<String, Group>,
    ) -> (BTreeSet<String>, BTreeSet<String>) {
        let mut allow = self.public.clone();
        let mut deny = BTreeSet::new();

        // The principal's own deltas.
        for deltas in [self.clients.get(principal), self.groups.get(principal)]
            .into_iter()
            .flatten()
        {
            for path in &deltas.remove {
                allow.remove(path);
                deny.insert(path.clone());
            }
            for path in &deltas.add {
                allow.insert(path.clone());
            }
        }

        // Resolve every group that contains the principal, directly or transitively.
        let mut current: BTreeSet<String> = groups
            .iter()
            .filter_map(|(name, def)| {
                if def.members.contains(principal) {
                    Some(name.clone())
                } else {
                    None
                }
            })
            .collect();
        let mut changed = true;
        while changed {
            changed = false;
            for (name, def) in groups {
                if current.contains(name) {
                    continue;
                }
                if def.members.iter().any(|member| current.contains(member))
                    && current.insert(name.clone())
                {
                    changed = true;
                }
            }
        }

        for group in &current {
            if let Some(deltas) = self.groups.get(group) {
                for path in &deltas.remove {
                    allow.remove(path);
                    deny.insert(path.clone());
                }
                for path in &deltas.add {
                    allow.insert(path.clone());
                }
            }
        }

        (allow, deny)
    }

    /// Return `true` when any `public` glob covers the concrete `path`.
    fn public_covers(&self, path: &str) -> bool {
        self.public
            .iter()
            .any(|rule| Glob::parse(rule).is_ok_and(|glob| glob.covers(path)))
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
    fn remove_below_a_public_glob_records_a_delta() {
        // A literal path covered by a public glob (for example `**`) must get a
        // `remove` delta, so it is hidden for that one account and stays served
        // as 403 rather than 200.
        let mut policy = Policy::new();
        let share = policy.share_mut("cube").unwrap();
        share.public.insert("**".to_string());
        share.remove_for("alpha", "sub/secret.pdf");
        assert!(
            share.clients["alpha"].remove.contains("sub/secret.pdf"),
            "remove delta must be recorded under a public glob"
        );
        let access = policy.expand(&clients(&["alpha", "beta"])).unwrap();
        assert!(!access.allows("alpha", "cube", "sub/secret.pdf"));
        assert!(access.allows("alpha", "cube", "sub/other.pdf"));
        assert!(access.allows("beta", "cube", "sub/secret.pdf"));
    }

    #[test]
    fn re_add_below_a_public_glob_clears_the_delta() {
        let mut policy = Policy::new();
        let share = policy.share_mut("cube").unwrap();
        share.public.insert("**".to_string());
        share.remove_for("alpha", "sub/secret.pdf");
        share.add_for("alpha", "sub/secret.pdf".to_string());
        assert!(!share.clients.contains_key("alpha"));
        let access = policy.expand(&clients(&["alpha"])).unwrap();
        assert!(access.allows("alpha", "cube", "sub/secret.pdf"));
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
        assert!(share.allowed("alpha").is_empty());
    }

    #[test]
    fn add_and_remove_cancel_each_other() {
        let mut policy = Policy::new();
        let share = policy.share_mut("cube").unwrap();
        share.add_for("alpha", "sub/f.pdf".to_string());
        share.remove_for("alpha", "sub/f.pdf");
        assert!(share.allowed("alpha").is_empty());
        share.add_for("alpha", "sub/f.pdf".to_string());
        assert!(share.allowed("alpha").contains("sub/f.pdf"));
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
                groups: BTreeMap::new(),
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

    use proptest::prelude::*;

    proptest! {
        #[test]
        fn split_rule_roundtrips(
            share in "[a-z0-9_-]{1,8}",
            path in "[a-z0-9._/-]{1,16}",
            extra_colon in 0usize..2,
        ) {
            let spec = format!("{share}:{path}{}", ":".repeat(extra_colon));
            let (left, right) = split_rule(&spec).unwrap();
            prop_assert_eq!(left, share);
            if extra_colon == 0 {
                prop_assert_eq!(right, path);
            } else {
                // The first ':' separates; a later ':' is part of the path.
                prop_assert_eq!(right, format!("{path}:"));
            }
        }
    }
}
