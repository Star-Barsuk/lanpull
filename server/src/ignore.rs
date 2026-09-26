//! Built-in ignore patterns.
//!
//! Matches the set documented in `docs/SPEC.md` section 6. Server-side ignore
//! is authoritative for the manifest; the same rules protect the client's own
//! internal state.

use std::path::Path;

/// Return `true` when a relative path matches a built-in ignore pattern.
///
/// Patterns: `~lock.*`, `.~lock.*`, `*.tmp`, `*.part`.
pub fn is_ignored(rel: &str) -> bool {
    let name = rel.rsplit('/').next().unwrap_or(rel);
    if name.starts_with("~lock.") || name.starts_with(".~lock.") {
        return true;
    }
    matches!(
        Path::new(name).extension().and_then(|ext| ext.to_str()),
        Some("tmp" | "part")
    )
}
