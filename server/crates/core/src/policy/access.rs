//! Access rule matching.
//!
//! This module is the compiled form of the access policy: a flat allow-list of
//! `(account, share, glob)` rules produced by expanding
//! `config/lanpull.access.json` (see [`crate::policy`]). A rule grants an
//! account the whole share, or the paths matching a glob. The default is deny:
//! an account with no rule sees nothing.
//!
//! The glob engine itself lives in [`crate::glob`].

use std::collections::BTreeMap;

use crate::config::valid_share_name;
use crate::error::{Error, Result};
use crate::glob::Glob;

/// One access rule.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Rule {
    /// Share name, or `None` for every share.
    pub share: Option<String>,
    /// Optional path glob; `None` grants the whole share.
    pub glob: Option<Glob>,
}

impl Rule {
    /// Parse a `<share>[:<glob>]` rule specification.
    pub fn parse(spec: &str) -> Result<Self> {
        let (share_token, glob_token) = match spec.split_once(':') {
            Some((share, glob)) => (share, Some(glob)),
            None => (spec, None),
        };
        let share = if share_token == "*" {
            None
        } else {
            if !valid_share_name(share_token) {
                return Err(Error::Config(format!("invalid share name '{share_token}'")));
            }
            Some(share_token.to_string())
        };
        let glob = match glob_token {
            None => None,
            Some(glob) => Some(Glob::parse(glob)?),
        };
        Ok(Self { share, glob })
    }

    /// Return `true` when the rule applies to `share`.
    fn matches_share(&self, share: &str) -> bool {
        self.share.as_deref().is_none_or(|name| name == share)
    }

    /// Render the rule spec (without the account name).
    pub fn spec(&self) -> String {
        let mut text = self.share.clone().unwrap_or_else(|| "*".to_string());
        if let Some(glob) = &self.glob {
            text.push(':');
            text.push_str(glob.as_str());
        }
        text
    }
}

/// The parsed access mapping.
///
/// An account's effective access is its allow rules minus its deny rules. A
/// deny rule is produced when a per-account `remove` subtracts a path from the
/// `public` set; because `public` may hold a glob (for example `**`), the
/// subtraction is applied per requested path at match time rather than as a
/// static set difference.
#[derive(Debug, Default, Clone)]
pub struct Access {
    rules: BTreeMap<String, Vec<Rule>>,
    denied: BTreeMap<String, Vec<Rule>>,
}

impl Access {
    /// Return an empty mapping.
    pub fn new() -> Self {
        Self::default()
    }

    /// Return the allow rules for an account.
    pub fn rules(&self, account: &str) -> &[Rule] {
        self.rules.get(account).map_or(&[], Vec::as_slice)
    }

    /// Return the deny rules for an account.
    pub fn denied_rules(&self, account: &str) -> &[Rule] {
        self.denied.get(account).map_or(&[], Vec::as_slice)
    }

    /// Return `true` when the account has at least one effective allow rule.
    ///
    /// An account whose only rules are denied everywhere is not considered to
    /// have access; the caller cannot know the concrete paths here, so any
    /// allow rule counts.
    pub fn has_rules(&self, account: &str) -> bool {
        self.rules
            .get(account)
            .is_some_and(|rules| !rules.is_empty())
    }

    /// Return `true` when the account may see `share` at all.
    pub fn allows_share(&self, account: &str, share: &str) -> bool {
        self.rules(account)
            .iter()
            .any(|rule| rule.matches_share(share))
    }

    /// Return `true` when the account may read `path` inside `share`.
    pub fn allows(&self, account: &str, share: &str, path: &str) -> bool {
        let segments: Vec<&str> = path.split('/').collect();
        let matched = |rule: &Rule| {
            rule.matches_share(share)
                && rule
                    .glob
                    .as_ref()
                    .is_none_or(|glob| glob.matches_segments(&segments))
        };
        if self.denied_rules(account).iter().any(matched) {
            return false;
        }
        self.rules(account).iter().any(matched)
    }

    /// Add an allow rule for an account.
    pub fn add_rule(&mut self, account: &str, rule: Rule) {
        self.rules
            .entry(account.to_string())
            .or_default()
            .push(rule);
    }

    /// Add a deny rule for an account.
    pub fn deny_rule(&mut self, account: &str, rule: Rule) {
        self.denied
            .entry(account.to_string())
            .or_default()
            .push(rule);
    }

    /// Return `true` when no account has any rule.
    pub fn is_empty(&self) -> bool {
        self.rules.is_empty()
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

    /// Build an access set from `<account> <spec>` pairs.
    fn access(entries: &[(&str, &str)]) -> Access {
        let mut access = Access::new();
        for (account, spec) in entries {
            access.add_rule(account, Rule::parse(spec).unwrap());
        }
        access
    }

    #[test]
    fn default_deny() {
        let a = access(&[("laptop", "reports")]);
        assert!(!a.allows("other", "reports", "a.pdf"));
        assert!(!a.allows("laptop", "media", "song.mp3"));
    }

    #[test]
    fn whole_share_allows_everything() {
        let a = access(&[("laptop", "reports")]);
        assert!(a.allows("laptop", "reports", "a/b/c.pdf"));
        assert!(a.allows_share("laptop", "reports"));
    }

    #[test]
    fn star_grant_covers_all_shares() {
        let a = access(&[("desktop", "*")]);
        assert!(a.allows("desktop", "reports", "x"));
        assert!(a.allows("desktop", "media", "y"));
    }

    #[test]
    fn prefix_and_double_star() {
        let a = access(&[("laptop", "media:music/**"), ("laptop", "docs:a")]);
        assert!(a.allows("laptop", "media", "music/song.mp3"));
        assert!(a.allows("laptop", "media", "music/2026/album/track.flac"));
        assert!(!a.allows("laptop", "media", "video/clip.mp4"));
        assert!(a.allows("laptop", "docs", "a"));
        assert!(!a.allows("laptop", "docs", "a/b"));
    }

    #[test]
    fn wildcard_within_segment() {
        let a = access(&[("laptop", "reports:*.pdf")]);
        assert!(a.allows("laptop", "reports", "final.pdf"));
        assert!(!a.allows("laptop", "reports", "sub/draft.pdf"));
        assert!(!a.allows("laptop", "reports", "final.docx"));

        let nested = access(&[("laptop", "reports:**/*.pdf")]);
        assert!(nested.allows("laptop", "reports", "sub/draft.pdf"));
        assert!(nested.allows("laptop", "reports", "final.pdf"));
    }

    #[test]
    fn bad_rule_specs_are_rejected() {
        assert!(Rule::parse("Reports").is_err());
        assert!(Rule::parse("reports:/abs").is_err());
        assert!(Rule::parse("reports:../x").is_err());
        assert!(Rule::parse("reports:_lanpull/x").is_err());
        assert!(Rule::parse("").is_err());
    }

    #[test]
    fn rules_accumulate_per_account() {
        let a = access(&[
            ("laptop", "reports"),
            ("laptop", "media:music/**"),
            ("desktop", "*"),
        ]);
        assert_eq!(a.rules("laptop").len(), 2);
        assert_eq!(a.rules("desktop").len(), 1);
        assert!(a.allows("laptop", "media", "music/x"));
    }
}
