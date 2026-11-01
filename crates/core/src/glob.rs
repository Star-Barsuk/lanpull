//! Share-relative path globs.
//!
//! A small wildcard engine shared by the access policy and the per-share
//! `.lanpullignore` rules. Syntax: `/`-separated segments, where `*` matches
//! within one segment and `**` matches zero or more segments. Patterns cannot
//! escape the share (no leading `/`, no `..`, no `_lanpull` segment).

use crate::error::{Error, Result};
use crate::relpath;

/// One part of a wildcard segment.
#[derive(Debug, Clone, PartialEq, Eq)]
enum WildcardPart {
    /// A literal run of characters.
    Literal(String),
    /// A `*` wildcard.
    Star,
}

/// One glob segment.
#[derive(Debug, Clone, PartialEq, Eq)]
enum PatternSegment {
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

    /// Return `true` when this glob matches the concrete share-relative `path`.
    ///
    /// Used by the policy editor to decide whether removing a literal path needs
    /// a `remove` delta: it does both for an exact `public` entry and for a
    /// `public` glob (for example `**`) that covers the path.
    pub fn covers(&self, path: &str) -> bool {
        if relpath::validate(path).is_err() {
            return false;
        }
        let segments: Vec<&str> = path.split('/').collect();
        self.matches_segments(&segments)
    }

    /// Return `true` when this glob matches a path already split into segments.
    ///
    /// The caller has already validated the path, so this skips re-validation.
    pub(crate) fn matches_segments(&self, path: &[&str]) -> bool {
        match_segments(&self.segments, path)
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

    fn covers(pattern: &str, path: &str) -> bool {
        Glob::parse(pattern).unwrap().covers(path)
    }

    #[test]
    fn double_star_matches_any_depth() {
        assert!(covers("**", "a"));
        assert!(covers("**", "a/b/c"));
        assert!(covers("music/**", "music/song.mp3"));
        assert!(covers("music/**", "music/2026/album/track.flac"));
        assert!(!covers("music/**", "video/clip.mp4"));
    }

    #[test]
    fn star_matches_within_one_segment() {
        assert!(covers("*.pdf", "final.pdf"));
        assert!(!covers("*.pdf", "sub/draft.pdf"));
        assert!(!covers("*.pdf", "final.docx"));
        assert!(covers("**/*.pdf", "sub/draft.pdf"));
        assert!(covers("**/*.pdf", "final.pdf"));
    }

    #[test]
    fn invalid_globs_are_rejected() {
        assert!(Glob::parse("").is_err());
        assert!(Glob::parse("/abs").is_err());
        assert!(Glob::parse("../x").is_err());
        assert!(Glob::parse("a//b").is_err());
        assert!(Glob::parse("_lanpull/x").is_err());
        assert!(Glob::parse("a\0b").is_err());
        assert!(Glob::parse(".").is_err());
        assert!(Glob::parse("a/./b").is_err());
        assert!(Glob::parse("a/").is_err());
    }

    #[test]
    fn covers_rejects_unsafe_paths() {
        let glob = Glob::parse("**").unwrap();
        assert!(!glob.covers("/abs"));
        assert!(!glob.covers("a/../b"));
        assert!(!glob.covers("_lanpull/x"));
    }

    #[test]
    fn star_matches_any_run_within_a_segment() {
        assert!(covers("a*", "a"));
        assert!(covers("a*", "abc"));
        assert!(covers("*", ".hidden"));
        assert!(covers("a*b", "ab"));
        assert!(covers("a*b", "aXXb"));
        assert!(covers("*.txt", "file.txt"));
        assert!(covers("file.*", "file."));
    }

    #[test]
    fn single_star_never_crosses_a_segment() {
        assert!(!covers("a*b", "a/b"));
        assert!(!covers("*", "a/b"));
        assert!(!covers("*.txt", "dir/file.txt"));
    }

    #[test]
    fn double_star_only_as_a_whole_segment() {
        // `a**b` is a normal wildcard segment (two stars), not an any-depth token.
        assert!(covers("a**b", "aXXb"));
        assert!(!covers("a**b", "a/X/b"));
    }

    #[test]
    fn double_star_in_the_middle() {
        assert!(covers("a/**/b", "a/b"));
        assert!(covers("a/**/b", "a/x/b"));
        assert!(covers("a/**/b", "a/x/y/b"));
        assert!(!covers("a/**/b", "a/x"));
        assert!(!covers("a/**/b", "x/a/b"));
    }

    #[test]
    fn literal_special_characters_are_matched() {
        assert!(covers("#*#", "#autosave#"));
        assert!(covers("~$*", "~$deck.docx"));
        assert!(covers(".#*", ".#lock"));
        assert!(covers(".DS_Store", ".DS_Store"));
        assert!(covers("a+b", "a+b"));
    }

    #[test]
    fn unicode_segments_are_matched() {
        assert!(covers("файл*", "файл.txt"));
        assert!(covers("*док*", "мой-док.pdf"));
    }

    #[test]
    fn matches_segments_uses_the_same_rules() {
        let glob = Glob::parse("a/**/b").unwrap();
        assert!(glob.matches_segments(&["a", "b"]));
        assert!(glob.matches_segments(&["a", "x", "b"]));
        assert!(!glob.matches_segments(&["a", "x"]));
    }

    use proptest::prelude::*;

    proptest! {
        #[test]
        fn any_depth_matches_every_path(segments in proptest::collection::vec("[a-z]{1,6}", 0..5)) {
            let path = segments.join("/");
            let glob = Glob::parse("**").unwrap();
            let parts: Vec<&str> = path.split('/').collect();
            let parts = if path.is_empty() { Vec::new() } else { parts };
            prop_assert!(glob.matches_segments(&parts));
        }
    }
}
