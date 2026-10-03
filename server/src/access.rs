//! Access rule matching.
//!
//! This module is the compiled form of the access policy: a flat allow-list of
//! `(account, share, glob)` rules produced by expanding
//! `config/lanpull.access.json` (see [`crate::policy`]). A rule grants an
//! account the whole share, or the paths matching a glob. The default is deny:
//! an account with no rule sees nothing.
//!
//! Glob syntax: `/`-separated segments, where `*` matches within one segment
//! and `**` matches zero or more segments. Patterns cannot escape the share
//! (no leading `/`, no `..`, no `_lanpull` segment).

use std::collections::BTreeMap;

use crate::config::valid_share_name;
use crate::error::{Error, Result};
use crate::relpath;

/// One part of a wildcard segment.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum WildcardPart {
    /// A literal run of characters.
    Literal(String),
    /// A `*` wildcard.
    Star,
}

/// One glob segment.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PatternSegment {
    /// `**`: zero or more path segments.
    AnyDepth,
    /// A segment containing `*` wildcards.
    Wildcard(Vec<WildcardPart>),
    /// A literal segment.
    Literal(String),
}

/// A validated path glob together with its source text.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Glob {
    raw: String,
    segments: Vec<PatternSegment>,
}

impl Glob {
    /// Parse and validate a path glob.
    ///
    /// Segments are validated without `relpath::validate`, because `**` is a
    /// valid segment here; everything else (no leading `/`, no empty, dot, or
    /// reserved segment, no NUL) is rejected.
    pub fn parse(raw: &str) -> Result<Self> {
        if raw.is_empty() {
            return Err(Error::Config("empty glob".to_string()));
        }
        if raw.starts_with('/') {
            return Err(Error::Config(format!(
                "invalid glob '{raw}': absolute path"
            )));
        }
        if raw.contains('\0') {
            return Err(Error::Config(format!("invalid glob '{raw}': contains NUL")));
        }
        let mut segments = Vec::new();
        for segment in raw.split('/') {
            if segment.is_empty() {
                return Err(Error::Config(format!(
                    "invalid glob '{raw}': empty segment"
                )));
            }
            if segment == "." || segment == ".." {
                return Err(Error::Config(format!("invalid glob '{raw}': dot segment")));
            }
            if segment.contains(relpath::RESERVED_PREFIX) {
                return Err(Error::Config(format!(
                    "invalid glob '{raw}': reserved prefix"
                )));
            }
            segments.push(parse_segment(segment));
        }
        Ok(Self {
            raw: raw.to_string(),
            segments,
        })
    }

    /// Return the source text of the glob.
    pub fn as_str(&self) -> &str {
        &self.raw
    }
}

/// Parse one glob segment.
fn parse_segment(segment: &str) -> PatternSegment {
    if segment == "**" {
        return PatternSegment::AnyDepth;
    }
    if !segment.contains('*') {
        return PatternSegment::Literal(segment.to_string());
    }
    let mut parts = Vec::new();
    let mut literal = String::new();
    for ch in segment.chars() {
        if ch == '*' {
            if !literal.is_empty() {
                parts.push(WildcardPart::Literal(std::mem::take(&mut literal)));
            }
            parts.push(WildcardPart::Star);
        } else {
            literal.push(ch);
        }
    }
    if !literal.is_empty() {
        parts.push(WildcardPart::Literal(literal));
    }
    PatternSegment::Wildcard(parts)
}

/// Match a wildcard segment against a single path segment.
fn match_wildcard(parts: &[WildcardPart], text: &str) -> bool {
    match parts.split_first() {
        None => text.is_empty(),
        Some((WildcardPart::Literal(literal), rest)) => text
            .strip_prefix(literal.as_str())
            .is_some_and(|remainder| match_wildcard(rest, remainder)),
        Some((WildcardPart::Star, rest)) => {
            if rest.is_empty() {
                return true;
            }
            text.char_indices()
                .map(|(index, _)| index)
                .chain([text.len()])
                .any(|index| {
                    text.get(index..)
                        .is_some_and(|tail| match_wildcard(rest, tail))
                })
        }
    }
}

/// Match a whole path against a compiled glob.
fn match_segments(pattern: &[PatternSegment], path: &[&str]) -> bool {
    match pattern.split_first() {
        None => path.is_empty(),
        Some((PatternSegment::AnyDepth, rest)) => (0..=path.len()).any(|skip| {
            path.get(skip..)
                .is_some_and(|tail| match_segments(rest, tail))
        }),
        Some((segment, rest)) => match path.split_first() {
            Some((first, tail)) => match_segment(segment, first) && match_segments(rest, tail),
            None => false,
        },
    }
}

/// Match one pattern segment against one path segment.
fn match_segment(segment: &PatternSegment, text: &str) -> bool {
    match segment {
        PatternSegment::Literal(literal) => literal == text,
        PatternSegment::Wildcard(parts) => match_wildcard(parts, text),
        PatternSegment::AnyDepth => false,
    }
}

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
#[derive(Debug, Default, Clone)]
pub struct Access {
    rules: BTreeMap<String, Vec<Rule>>,
}

impl Access {
    /// Return an empty mapping.
    pub fn new() -> Self {
        Self::default()
    }

    /// Return the rules for an account.
    pub fn rules(&self, account: &str) -> &[Rule] {
        self.rules.get(account).map_or(&[], Vec::as_slice)
    }

    /// Return `true` when the account has at least one rule.
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
        self.rules(account).iter().any(|rule| {
            rule.matches_share(share)
                && rule
                    .glob
                    .as_ref()
                    .is_none_or(|glob| match_segments(&glob.segments, &segments))
        })
    }

    /// Add a rule for an account.
    pub fn add_rule(&mut self, account: &str, rule: Rule) {
        self.rules
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

    use proptest::prelude::*;

    proptest! {
        #[test]
        fn double_star_matches_any_depth(segments in proptest::collection::vec("[a-z]{1,6}", 0..5)) {
            let path = segments.join("/");
            let glob = Glob::parse("**").unwrap();
            let parts: Vec<&str> = path.split('/').collect();
            let parts = if path.is_empty() { Vec::new() } else { parts };
            prop_assert!(match_segments(&glob.segments, &parts));
        }
    }
}
