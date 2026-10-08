//! Ignore rules: a mandatory built-in set, an overridable default set, and the
//! per-share `.lanpullignore` file.
//!
//! There are three tiers, evaluated in this order:
//!
//! 1. **Mandatory built-ins** (`~lock.*`, `.~lock.*`, `*.tmp`, `*.part`) — see
//!    [`is_builtin_ignored`]. They are always ignored and can never be
//!    re-included.
//! 2. **Default set** — [`DEFAULT_IGNORE_PATTERNS`], the common VCS, build,
//!    cache, editor, and OS artifacts for many languages. It is applied to
//!    every share but a `!` rule in `.lanpullignore` re-includes a path.
//! 3. **Operator `.lanpullignore`** at the share root — gitignore-like rules,
//!    where the last match wins.
//!
//! Matching follows Git closely enough to be predictable: a pattern that names
//! a directory also excludes everything below it, `*` matches within one path
//! segment, `**` spans segments, a leading `/` anchors to the share root, and a
//! pattern without a separator matches its name at any depth. Character
//! classes (`[abc]`) and escapes other than `\#`/`\!` are not supported.
//!
//! Ignored paths are never included in the manifest, never transferred, and
//! never treated as stale.

use std::borrow::Cow;
use std::path::Path;

use crate::glob::Glob;

/// The name of the per-share ignore file.
pub const IGNORE_FILE_NAME: &str = ".lanpullignore";

/// The default ignore set, applied to every share and overridable with `!`.
///
/// It targets the build, cache, and metadata artifacts of common languages and
/// tools, plus editor and OS temporaries. An operator re-includes a path by
/// adding a `!` rule to `.lanpullignore` (the built-in set above cannot be
/// re-included).
pub const DEFAULT_IGNORE_PATTERNS: &[&str] = &[
    // Version-control metadata.
    ".git/",
    ".svn/",
    ".hg/",
    ".bzr/",
    ".jj/",
    "CVS/",
    // Rust.
    "target/",
    "*.rs.bk",
    "*.pdb",
    // Python.
    "__pycache__/",
    "*.pyc",
    "*.pyo",
    "*.pyd",
    ".venv/",
    "venv/",
    ".eggs/",
    "*.egg-info/",
    ".pytest_cache/",
    ".mypy_cache/",
    ".ruff_cache/",
    ".tox/",
    ".nox/",
    ".hypothesis/",
    ".coverage",
    ".coverage.*",
    "htmlcov/",
    // JavaScript / Node.
    "node_modules/",
    ".npm/",
    ".pnpm-store/",
    ".yarn/",
    ".next/",
    ".nuxt/",
    ".turbo/",
    ".cache/",
    "coverage/",
    // JVM.
    "*.class",
    ".gradle/",
    // C / C++.
    "*.o",
    "*.obj",
    "*.a",
    "*.so",
    "*.so.*",
    "*.dylib",
    "*.dll",
    "*.exe",
    "*.out",
    "CMakeFiles/",
    "CMakeCache.txt",
    "cmake-build-*/",
    // Go, Ruby, PHP.
    "vendor/",
    ".bundle/",
    "vendor/bundle/",
    "*.gem",
    // Editors, OS, office locks.
    "*.swp",
    "*.swo",
    "*.swn",
    "*~",
    ".#*",
    "#*#",
    ".DS_Store",
    "Thumbs.db",
    "desktop.ini",
    "~$*",
    // Generic build output and backups.
    "build/",
    "dist/",
    "*.log",
    "*.bak",
    "*.orig",
    "*.rej",
];

/// Return `true` when a share-relative path matches a mandatory built-in.
///
/// Patterns: `~lock.*`, `.~lock.*`, `*.tmp`, `*.part`. These can never be
/// re-included by `.lanpullignore`.
pub fn is_builtin_ignored(rel: &str) -> bool {
    let name = rel.rsplit('/').next().unwrap_or(rel);
    if name.starts_with("~lock.") || name.starts_with(".~lock.") {
        return true;
    }
    matches!(
        Path::new(name).extension().and_then(|ext| ext.to_str()),
        Some("tmp" | "part")
    )
}

/// Return `true` when the path names the per-share ignore file itself.
///
/// The ignore file is an operator control, never share content, so it is never
/// distributed — at the share root or in any subdirectory.
fn is_ignore_file(rel: &str) -> bool {
    rel.rsplit('/').next().unwrap_or(rel) == IGNORE_FILE_NAME
}

/// One compiled ignore rule.
#[derive(Debug, Clone, PartialEq, Eq)]
struct Rule {
    /// A matching rule with `negated` re-includes the path.
    negated: bool,
    /// The pattern is relative to the share root rather than matched at any
    /// depth.
    anchored: bool,
    /// The pattern names a directory, so it matches only an ancestor of a file.
    directory: bool,
    /// The compiled glob.
    glob: Glob,
}

impl Rule {
    /// Return `true` when the rule matches a file path split into segments.
    ///
    /// A pattern that matches any ancestor directory excludes the whole
    /// subtree, so the path and every ancestor prefix (or, for an unanchored
    /// pattern, every path component) are tested.
    fn matches(&self, segments: &[&str]) -> bool {
        let len = segments.len();
        if self.anchored {
            let max = if self.directory {
                len.saturating_sub(1)
            } else {
                len
            };
            (1..=max).any(|end| {
                segments
                    .get(..end)
                    .is_some_and(|prefix| self.glob.matches_segments(prefix))
            })
        } else if self.directory {
            (0..len.saturating_sub(1)).any(|index| {
                segments
                    .get(index..=index)
                    .is_some_and(|one| self.glob.matches_segments(one))
            })
        } else {
            (0..len).any(|index| {
                segments
                    .get(index..=index)
                    .is_some_and(|one| self.glob.matches_segments(one))
            })
        }
    }
}

/// The compiled ignore rules for one share: the default set plus the
/// operator's `.lanpullignore`.
#[derive(Debug, Default, Clone)]
pub struct IgnoreRules {
    rules: Vec<Rule>,
}

impl IgnoreRules {
    /// Load the default set and `.lanpullignore` from the share root.
    ///
    /// A missing file yields the defaults. Blank lines and `#` comments are
    /// skipped. An unreadable file or an invalid pattern is reported as a
    /// warning instead of failing the scan, so a typo never blocks a rescan.
    pub fn load(share: &Path) -> (Self, Vec<String>) {
        let mut rules = Self::default();
        let mut warnings = Vec::new();

        // Defaults first, so an operator `!` rule can re-include one.
        for pattern in DEFAULT_IGNORE_PATTERNS {
            match compile_rule(pattern) {
                Ok(rule) => rules.rules.push(rule),
                Err(message) => {
                    warnings.push(format!("built-in default '{pattern}': {message}"));
                }
            }
        }

        let path = share.join(IGNORE_FILE_NAME);
        let text = match std::fs::read_to_string(&path) {
            Ok(text) => text,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => return (rules, warnings),
            Err(e) => {
                warnings.push(format!("cannot read {}: {e}", path.display()));
                return (rules, warnings);
            }
        };
        let text = text.strip_prefix('\u{feff}').unwrap_or(&text);
        for (index, line) in text.lines().enumerate() {
            match parse_line(line) {
                None => {}
                Some(Ok(rule)) => rules.rules.push(rule),
                Some(Err(message)) => warnings.push(format!(
                    "{}:{}: {message}",
                    IGNORE_FILE_NAME,
                    index.saturating_add(1)
                )),
            }
        }
        (rules, warnings)
    }

    /// Return `true` when a share-relative path must be excluded.
    ///
    /// Mandatory built-ins and the `.lanpullignore` file itself always win.
    /// Otherwise the last matching rule decides, and a negated match restores
    /// the path.
    pub fn is_ignored(&self, rel: &str) -> bool {
        if is_ignore_file(rel) || is_builtin_ignored(rel) {
            return true;
        }
        let segments: Vec<&str> = rel.split('/').collect();
        let mut ignored = false;
        for rule in &self.rules {
            if rule.matches(&segments) {
                ignored = !rule.negated;
            }
        }
        ignored
    }
}

/// Parse one `.lanpullignore` line.
///
/// Returns `None` for a blank line or comment, or a parsed rule (or a warning
/// message on an invalid pattern). A leading `\#` or `\!` is an escaped literal
/// marker rather than a comment or a negation.
fn parse_line(line: &str) -> Option<Result<Rule, String>> {
    let trimmed = line.trim();
    if trimmed.is_empty() || trimmed.starts_with('#') {
        return None;
    }
    Some(compile_rule(trimmed))
}

/// Compile one non-blank, non-comment `.lanpullignore` line.
fn compile_rule(raw: &str) -> Result<Rule, String> {
    let unescaped = raw
        .strip_prefix("\\#")
        .map(|rest| format!("#{rest}"))
        .or_else(|| raw.strip_prefix("\\!").map(|rest| format!("!{rest}")));
    let (line, escaped): (Cow<'_, str>, bool) = unescaped.map_or_else(
        || (Cow::Borrowed(raw), false),
        |line| (Cow::Owned(line), true),
    );
    let line = line.as_ref();
    let (negated, rest) = if escaped {
        (false, line)
    } else {
        line.strip_prefix('!')
            .map_or((false, line), |rest| (true, rest))
    };
    let had_leading_slash = rest.starts_with('/');
    let rest = rest.strip_prefix('/').unwrap_or(rest);
    let directory = rest.ends_with('/');
    let rest = rest.strip_suffix('/').unwrap_or(rest);
    if rest.is_empty() {
        return Err(format!("invalid pattern '{raw}'"));
    }
    let anchored = had_leading_slash || rest.contains('/');
    let glob = Glob::parse(rest).map_err(|e| format!("invalid pattern '{raw}': {e}"))?;
    Ok(Rule {
        negated,
        anchored,
        directory,
        glob,
    })
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
        clippy::missing_assert_message,
        clippy::assert_is_empty
    )]

    use super::*;

    /// Compile operator-only rules (no default set) for isolated matching tests.
    fn rules(lines: &[&str]) -> IgnoreRules {
        let mut rules = IgnoreRules::default();
        for line in lines {
            match parse_line(line) {
                Some(Ok(rule)) => rules.rules.push(rule),
                other => panic!("unexpected parse of {line:?}: {other:?}"),
            }
        }
        rules
    }

    /// Load only the default set (a share with no `.lanpullignore`).
    fn defaults() -> IgnoreRules {
        let dir = tempfile::tempdir().unwrap();
        let (rules, warnings) = IgnoreRules::load(dir.path());
        assert!(
            warnings.is_empty(),
            "defaults produced warnings: {warnings:?}"
        );
        rules
    }

    #[test]
    fn mandatory_patterns_are_ignored() {
        for ignored in ["~lock.a", ".~lock.b", "a.tmp", "dir/a.part"] {
            assert!(is_builtin_ignored(ignored), "{ignored} should be ignored");
        }
        for kept in ["a.txt", "dir/b.pdf", "tmp", "part", "a.tmpx"] {
            assert!(!is_builtin_ignored(kept), "{kept} should be kept");
        }
    }

    #[test]
    fn all_defaults_compile() {
        for pattern in DEFAULT_IGNORE_PATTERNS {
            assert!(
                compile_rule(pattern).is_ok(),
                "default '{pattern}' is invalid"
            );
        }
    }

    #[test]
    fn default_set_excludes_language_artifacts() {
        let r = defaults();
        for ignored in [
            ".git/HEAD",
            ".svn/entries",
            "server/target/debug/lanpull",
            "src/main.rs.bk",
            "app.pdb",
            "__pycache__/module.cpython-311.pyc",
            "mod.pyc",
            ".venv/bin/python",
            "venv/lib/x",
            "pkg.egg-info/PKG-INFO",
            ".pytest_cache/v/cache",
            ".mypy_cache/3.11/x.json",
            ".coverage",
            ".coverage.html",
            "htmlcov/index.html",
            "node_modules/pkg/index.js",
            ".next/cache/x",
            "coverage/lcov.info",
            "com/example/Foo.class",
            ".gradle/8.0/x",
            "obj/foo.o",
            "libfoo.a",
            "libfoo.so",
            "libfoo.so.1.2",
            "libfoo.dylib",
            "foo.dll",
            "foo.exe",
            "a.out",
            "CMakeFiles/x.dir/a.o",
            "CMakeCache.txt",
            "cmake-build-debug/x",
            "vendor/lib/x.go",
            ".bundle/config",
            "vendor/bundle/gems/x",
            "pkg.gem",
            "notes.swp",
            "notes.swo",
            "notes.swn",
            "backup~",
            ".#lock",
            "#autosave#",
            ".DS_Store",
            "sub/Thumbs.db",
            "desktop.ini",
            "~$deck.docx",
            "build/output.bin",
            "dist/app.js",
            "server.log",
            "old.bak",
            "patch.orig",
            "patch.rej",
        ] {
            assert!(
                r.is_ignored(ignored),
                "{ignored} should be ignored by defaults"
            );
        }
    }

    #[test]
    fn default_set_keeps_content() {
        let r = defaults();
        for kept in [
            "README.md",
            "report.pdf",
            "slides/deck.pptx",
            "src/main.rs",
            "Cargo.toml",
            "Cargo.lock",
            "requirements.txt",
            "package.json",
            "package-lock.json",
            "go.mod",
            "Makefile",
            "index.html",
            "Dockerfile",
            "logo.png",
            "data.csv",
            "notes.txt",
            "release.out.txt",
        ] {
            assert!(!r.is_ignored(kept), "{kept} should be kept by defaults");
        }
    }

    #[test]
    fn operator_negation_overrides_a_default() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join(IGNORE_FILE_NAME), "!*.log\n!build/**\n").unwrap();
        let (r, warnings) = IgnoreRules::load(dir.path());
        assert!(warnings.is_empty(), "unexpected warnings: {warnings:?}");
        assert!(!r.is_ignored("server.log"));
        assert!(!r.is_ignored("build/output.bin"));
        assert!(r.is_ignored("server.log.bak"));
    }

    #[test]
    fn mandatory_cannot_be_negated() {
        let r = rules(&["!*.tmp", "!~lock.x"]);
        assert!(r.is_ignored("a.tmp"));
        assert!(r.is_ignored("~lock.a"));
    }

    #[test]
    fn comments_and_blanks_are_skipped() {
        assert!(parse_line("").is_none());
        assert!(parse_line("   ").is_none());
        assert!(parse_line("\t").is_none());
        assert!(parse_line("# comment").is_none());
        assert!(parse_line("   # indented comment").is_none());
    }

    #[test]
    fn escaped_leading_markers_are_literal() {
        let hash = rules(&["\\#file"]);
        assert!(hash.is_ignored("#file"));
        assert!(hash.is_ignored("dir/#file"));
        let bang = rules(&["\\!file"]);
        assert!(bang.is_ignored("!file"));
        // The next line is still a real negation.
        let both = rules(&["*.txt", "\\!keep.txt", "!other.txt"]);
        assert!(both.is_ignored("a.txt"));
        assert!(both.is_ignored("!keep.txt"));
        assert!(!both.is_ignored("other.txt"));
    }

    #[test]
    fn bom_is_stripped() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join(IGNORE_FILE_NAME), "\u{feff}*.iso\n").unwrap();
        let (rules, warnings) = IgnoreRules::load(dir.path());
        assert!(warnings.is_empty(), "unexpected warnings: {warnings:?}");
        assert!(rules.is_ignored("a.iso"));
    }

    #[test]
    fn crlf_lines_are_parsed() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(
            dir.path().join(IGNORE_FILE_NAME),
            "*.iso\r\n# c\r\n*.raw\r\n",
        )
        .unwrap();
        let (rules, warnings) = IgnoreRules::load(dir.path());
        assert!(warnings.is_empty(), "unexpected warnings: {warnings:?}");
        assert!(rules.is_ignored("a.iso"));
        assert!(rules.is_ignored("a.raw"));
    }

    #[test]
    fn name_pattern_matches_at_any_depth() {
        let r = rules(&["*.iso"]);
        assert!(r.is_ignored("a.iso"));
        assert!(r.is_ignored("dir/sub/a.iso"));
        assert!(!r.is_ignored("a.txt"));
    }

    #[test]
    fn pattern_matching_a_directory_excludes_its_contents() {
        let r = rules(&["drafts"]);
        assert!(r.is_ignored("drafts/a.txt"));
        assert!(r.is_ignored("x/drafts/a.txt"));
        assert!(r.is_ignored("drafts"));
        assert!(!r.is_ignored("drafts.txt"));
    }

    #[test]
    fn trailing_slash_matches_directories_only() {
        let r = rules(&["drafts/"]);
        assert!(r.is_ignored("drafts/a.txt"));
        assert!(r.is_ignored("x/drafts/a.txt"));
        assert!(!r.is_ignored("drafts.txt"));
        // A file named `drafts` is not a directory and is kept.
        assert!(!r.is_ignored("drafts"));
    }

    #[test]
    fn wildcard_directory_excludes_contents() {
        let r = rules(&["*.iso"]);
        assert!(r.is_ignored("archive.iso/data.bin"));
        assert!(!r.is_ignored("archive.iso.txt"));
    }

    #[test]
    fn anchored_pattern_matches_only_at_root() {
        let r = rules(&["/draft.txt", "media/**"]);
        assert!(r.is_ignored("draft.txt"));
        assert!(!r.is_ignored("sub/draft.txt"));
        assert!(r.is_ignored("media/clip.mp4"));
        assert!(!r.is_ignored("sub/media/clip.mp4"));
    }

    #[test]
    fn anchored_directory_matches_only_at_root() {
        let unanchored = rules(&["media/"]);
        assert!(unanchored.is_ignored("x/media/a"));
        let anchored = rules(&["/media/"]);
        assert!(anchored.is_ignored("media/a"));
        assert!(!anchored.is_ignored("x/media/a"));
    }

    #[test]
    fn negation_restores_an_ignored_path() {
        let r = rules(&["*", "!keep.txt"]);
        assert!(r.is_ignored("a.txt"));
        assert!(!r.is_ignored("keep.txt"));
    }

    #[test]
    fn later_rule_wins() {
        let r = rules(&["!keep.txt", "keep.txt"]);
        assert!(r.is_ignored("keep.txt"));
    }

    #[test]
    fn wildcard_star_matches_dotfiles() {
        let r = rules(&["*"]);
        assert!(r.is_ignored(".hidden"));
        assert!(r.is_ignored("dir/.hidden"));
    }

    #[test]
    fn ignore_file_is_never_distributed() {
        let r = rules(&["!.lanpullignore"]);
        assert!(r.is_ignored(".lanpullignore"));
        assert!(r.is_ignored("sub/.lanpullignore"));
    }

    #[test]
    fn nested_ignore_file_is_not_parsed() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::create_dir_all(dir.path().join("sub")).unwrap();
        std::fs::write(dir.path().join("sub").join(IGNORE_FILE_NAME), "*.iso\n").unwrap();
        let (rules, warnings) = IgnoreRules::load(dir.path());
        assert!(warnings.is_empty(), "unexpected warnings: {warnings:?}");
        // Only the root file is parsed, so the nested rule has no effect ...
        assert!(!rules.is_ignored("sub/a.iso"));
        // ... but the nested file itself is never distributed.
        assert!(rules.is_ignored("sub/.lanpullignore"));
    }

    #[test]
    fn invalid_patterns_are_warnings() {
        for line in ["!", "/", "a//b", "..", "_lanpull/x", "a\0b"] {
            let entry = parse_line(line);
            assert!(matches!(entry, Some(Err(_))), "{line:?} should be an error");
        }
        // A single slash with a valid body is valid (root anchor).
        assert!(matches!(parse_line("/draft.txt"), Some(Ok(_))));
    }

    #[test]
    fn ignore_file_as_directory_is_a_warning() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::create_dir_all(dir.path().join(IGNORE_FILE_NAME)).unwrap();
        let (rules, warnings) = IgnoreRules::load(dir.path());
        assert_eq!(warnings.len(), 1, "expected one warning: {warnings:?}");
        // The built-in default set still applies.
        assert!(rules.is_ignored("server/target/x"));
    }

    #[test]
    fn load_reports_warnings_and_applies_rules() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(
            dir.path().join(IGNORE_FILE_NAME),
            "# comment\n*.iso\n\nbad//pattern\n",
        )
        .unwrap();
        let (rules, warnings) = IgnoreRules::load(dir.path());
        assert_eq!(warnings.len(), 1);
        assert!(warnings[0].starts_with(".lanpullignore:4:"), "{warnings:?}");
        assert!(rules.is_ignored("dir/a.iso"));
        assert!(!rules.is_ignored("a.txt"));
    }

    #[test]
    fn missing_file_still_applies_defaults() {
        let dir = tempfile::tempdir().unwrap();
        let (rules, warnings) = IgnoreRules::load(dir.path());
        assert!(warnings.is_empty());
        assert!(rules.is_ignored("node_modules/x.js"));
        assert!(!rules.is_ignored("a.txt"));
    }
}
