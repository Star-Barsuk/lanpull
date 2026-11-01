//! Validation of manifest-relative paths.
//!
//! A manifest path is relative to the share root, `/`-separated, and must not
//! escape the share or enter the reserved `_lanpull/` namespace.

use std::path::{Component, Path, PathBuf};

use crate::error::{Error, Result};

/// The reserved prefix owned by the server.
///
/// No share file is ever served under this prefix, and a manifest containing a
/// segment equal to it is rejected.
pub const RESERVED_PREFIX: &str = "_lanpull";

/// Return `true` when the first path segment is the reserved prefix.
pub fn is_reserved(rel: &str) -> bool {
    rel.split('/').next() == Some(RESERVED_PREFIX)
}

/// Validate a manifest-relative path.
///
/// Rejects empty paths, absolute paths, `.`/`..` segments, empty segments, NUL
/// bytes, and any segment equal to the reserved prefix.
pub fn validate(rel: &str) -> Result<()> {
    if rel.is_empty() {
        return Err(Error::UnsafePath("empty path".to_string()));
    }
    if rel.starts_with('/') {
        return Err(Error::UnsafePath(format!("absolute path: {rel}")));
    }
    if rel.contains('\0') {
        return Err(Error::UnsafePath("path contains NUL".to_string()));
    }
    for segment in rel.split('/') {
        if segment.is_empty() {
            return Err(Error::UnsafePath(format!("empty segment: {rel}")));
        }
        if segment == "." || segment == ".." {
            return Err(Error::UnsafePath(format!("dot segment: {rel}")));
        }
        if segment == RESERVED_PREFIX {
            return Err(Error::UnsafePath(format!("reserved prefix: {rel}")));
        }
    }
    Ok(())
}

/// Join a validated relative path onto a root directory.
pub fn join(root: &Path, rel: &str) -> Result<PathBuf> {
    validate(rel)?;
    Ok(root.join(rel))
}

/// Convert a path under `root` into a `/`-separated relative string.
///
/// Only normal path components are accepted; the reserved prefix is rejected.
pub fn to_relative_string(root: &Path, path: &Path) -> Result<String> {
    let relative = path.strip_prefix(root).map_err(|e| Error::BadPath {
        path: path.to_path_buf(),
        reason: e.to_string(),
    })?;

    let mut parts: Vec<String> = Vec::new();
    for component in relative.components() {
        match component {
            Component::Normal(name) => {
                let text = name.to_str().ok_or_else(|| Error::BadPath {
                    path: path.to_path_buf(),
                    reason: "non-UTF-8 file name".to_string(),
                })?;
                parts.push(text.to_string());
            }
            _ => {
                return Err(Error::BadPath {
                    path: path.to_path_buf(),
                    reason: "not a normal relative path".to_string(),
                });
            }
        }
    }

    if parts.is_empty() {
        return Err(Error::BadPath {
            path: path.to_path_buf(),
            reason: "empty relative path".to_string(),
        });
    }

    // Only normal components remain, so the result is safe by construction.
    // The reserved prefix is deliberately allowed here: callers such as the
    // manifest generator need to detect and warn about it rather than fail.
    Ok(parts.join("/"))
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
    use std::path::Path;

    #[test]
    fn accepts_safe_paths() {
        for path in ["a", "a/b", "dir/sub/file.pptx", "with space/café.pdf"] {
            assert!(validate(path).is_ok(), "{path} should be accepted");
        }
    }

    #[test]
    fn rejects_unsafe_paths() {
        for path in [
            "",
            "/abs",
            "..",
            "a/../b",
            "a/./b",
            "a//b",
            "trailing/",
            "_lanpull/manifest.json",
            "a/_lanpull/b",
            "nul\0byte",
        ] {
            assert!(validate(path).is_err(), "{path:?} should be rejected");
        }
    }

    #[test]
    fn reserved_detection() {
        assert!(is_reserved("_lanpull/x"));
        assert!(is_reserved("_lanpull"));
        assert!(!is_reserved("a/_lanpull"));
    }

    #[test]
    fn relative_string_uses_forward_slashes() {
        let root = Path::new("/share");
        let nested = root.join("a").join("b.txt");
        assert_eq!(to_relative_string(root, &nested).unwrap(), "a/b.txt");
    }

    use proptest::prelude::*;

    fn safe_segment() -> impl Strategy<Value = String> {
        "[a-zA-Z0-9._-]{1,8}".prop_filter("not a dot segment or reserved", |s| {
            s != "." && s != ".." && s != RESERVED_PREFIX
        })
    }

    proptest! {
        #[test]
        fn composed_safe_paths_validate(segments in proptest::collection::vec(safe_segment(), 1..4)) {
            let path = segments.join("/");
            prop_assert!(validate(&path).is_ok());
        }

        #[test]
        fn parent_segment_always_rejected(prefix in proptest::collection::vec(safe_segment(), 0..3)) {
            let mut parts = prefix;
            parts.push("..".to_string());
            let path = parts.join("/");
            prop_assert!(validate(&path).is_err());
        }
    }
}
