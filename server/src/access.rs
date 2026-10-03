//! Per-client access mapping.
//!
//! `config/lanpull.access` (mode `0600`, never committed) holds one rule per
//! line: `<client-name> <share>[:<glob>]`. A rule grants an account the whole
//! share, or the paths matching a glob. The default is deny: an account with no
//! rule sees nothing. A share token of `*` matches every share.
//!
//! Glob syntax: `/`-separated segments, where `*` matches within one segment
//! and `**` matches zero or more segments. Patterns cannot escape the share
//! (no leading `/`, no `..`, no `_lanpull` segment).

use std::collections::BTreeMap;
use std::io::ErrorKind;
use std::path::Path;

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

    /// Load the mapping from `path`, returning an empty mapping when absent.
    pub fn load(path: &Path) -> Result<Self> {
        let text = match std::fs::read_to_string(path) {
            Ok(text) => text,
            Err(e) if e.kind() == ErrorKind::NotFound => return Ok(Self::new()),
            Err(e) => return Err(e.into()),
        };
        parse(&text)
    }

    /// Atomically write the mapping to `path` with mode `0600`.
    pub fn save(&self, path: &Path) -> Result<()> {
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)?;
        }
        let mut text = String::new();
        for (account, rules) in &self.rules {
            for rule in rules {
                text.push_str(account);
                text.push(' ');
                text.push_str(&rule.spec());
                text.push('\n');
            }
        }
        crate::atomic::write_private(path, text.as_bytes())
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

    /// Remove matching rules for an account, returning whether any were removed.
    pub fn remove_rules(&mut self, account: &str, share: Option<&str>, glob: Option<&str>) -> bool {
        let Some(rules) = self.rules.get_mut(account) else {
            return false;
        };
        let before = rules.len();
        rules.retain(|rule| {
            let share_matches = share.is_none_or(|name| {
                rule.share.as_deref() == Some(name) || (name == "*" && rule.share.is_none())
            });
            let glob_matches =
                glob.is_none_or(|want| rule.glob.as_ref().map(Glob::as_str) == Some(want));
            !(share_matches && glob_matches)
        });
        let removed = rules.len() != before;
        if rules.is_empty() {
            self.rules.remove(account);
        }
        removed
    }

    /// Remove every rule for an account, returning whether any were present.
    pub fn remove_account(&mut self, account: &str) -> bool {
        self.rules.remove(account).is_some()
    }

    /// Iterate over accounts that have rules, in name order.
    pub fn iter(&self) -> impl Iterator<Item = (&str, &[Rule])> {
        self.rules
            .iter()
            .map(|(account, rules)| (account.as_str(), rules.as_slice()))
    }

    /// Return `true` when no account has any rule.
    pub fn is_empty(&self) -> bool {
        self.rules.is_empty()
    }
}

/// Parse the access file.
fn parse(text: &str) -> Result<Access> {
    let mut access = Access::new();
    for (index, raw) in text.lines().enumerate() {
        let line = raw.trim();
        if line.is_empty() || line.starts_with('#') {
            continue;
        }
        let line_number = index.saturating_add(1);
        let mut fields = line.split_whitespace();
        let account = fields
            .next()
            .ok_or_else(|| Error::Config(format!("access line {line_number}: missing account")))?;
        let spec = fields
            .next()
            .ok_or_else(|| Error::Config(format!("access line {line_number}: missing rule")))?;
        if fields.next().is_some() {
            return Err(Error::Config(format!(
                "access line {line_number}: expected two fields"
            )));
        }
        let rule = Rule::parse(spec)
            .map_err(|e| Error::Config(format!("access line {line_number}: {e}")))?;
        access.add_rule(account, rule);
    }
    Ok(access)
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

    fn access(text: &str) -> Access {
        parse(text).unwrap()
    }

    #[test]
    fn default_deny() {
        let a = access("laptop reports\n");
        assert!(!a.allows("other", "reports", "a.pdf"));
        assert!(!a.allows("laptop", "media", "song.mp3"));
    }

    #[test]
    fn whole_share_allows_everything() {
        let a = access("laptop reports\n");
        assert!(a.allows("laptop", "reports", "a/b/c.pdf"));
        assert!(a.allows_share("laptop", "reports"));
    }

    #[test]
    fn star_grant_covers_all_shares() {
        let a = access("desktop *\n");
        assert!(a.allows("desktop", "reports", "x"));
        assert!(a.allows("desktop", "media", "y"));
    }

    #[test]
    fn prefix_and_double_star() {
        let a = access("laptop media:music/**\nlaptop docs:a\n");
        assert!(a.allows("laptop", "media", "music/song.mp3"));
        assert!(a.allows("laptop", "media", "music/2026/album/track.flac"));
        assert!(!a.allows("laptop", "media", "video/clip.mp4"));
        assert!(a.allows("laptop", "docs", "a"));
        assert!(!a.allows("laptop", "docs", "a/b"));
    }

    #[test]
    fn wildcard_within_segment() {
        let a = access("laptop reports:*.pdf\n");
        assert!(a.allows("laptop", "reports", "final.pdf"));
        assert!(!a.allows("laptop", "reports", "sub/draft.pdf"));
        assert!(!a.allows("laptop", "reports", "final.docx"));

        let nested = access("laptop reports:**/*.pdf\n");
        assert!(nested.allows("laptop", "reports", "sub/draft.pdf"));
        assert!(nested.allows("laptop", "reports", "final.pdf"));
    }

    #[test]
    fn unknown_share_names_are_rejected() {
        assert!(parse("laptop Reports\n").is_err());
        assert!(parse("laptop reports:/abs\n").is_err());
        assert!(parse("laptop reports:../x\n").is_err());
        assert!(parse("laptop reports:_lanpull/x\n").is_err());
        assert!(parse("laptop\n").is_err());
        assert!(parse("laptop reports extra\n").is_err());
    }

    #[test]
    fn round_trip() {
        let text = "laptop reports\nlaptop media:music/**\ndesktop *\n";
        let a = access(text);
        assert_eq!(a.iter().count(), 2);
        assert_eq!(a.rules("laptop").len(), 2);
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
