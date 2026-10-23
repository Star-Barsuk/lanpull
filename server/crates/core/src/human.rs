//! Human-readable formatting for operator-facing output.

/// Format a byte count compactly, for example `1.2 GB`.
///
/// Mirrors the client's `format_bytes`, so server and client reports agree.
#[must_use]
pub fn format_bytes(value: u64) -> String {
    #[allow(clippy::cast_precision_loss)]
    let mut size = value as f64;
    for unit in ["B", "KB", "MB", "GB"] {
        if size < 1024.0 {
            return format!("{size:.1} {unit}");
        }
        size /= 1024.0;
    }
    format!("{size:.1} TB")
}

/// Replace terminal control characters in untrusted text with `?`.
///
/// Audit fields carry attacker-controlled values (the attempted user name and
/// the request path). They are JSON-escaped in the log, but a terminal that
/// prints them verbatim must not receive escape sequences.
#[must_use]
pub fn sanitize_terminal(text: &str) -> String {
    text.chars()
        .map(|c| if c.is_control() { '?' } else { c })
        .collect()
}

#[cfg(test)]
mod tests {
    #![allow(clippy::missing_assert_message)]

    use super::*;

    #[test]
    fn terminal_control_characters_are_replaced() {
        assert_eq!(sanitize_terminal("ok\u{1b}[31mred"), "ok?[31mred");
        assert_eq!(sanitize_terminal("plain"), "plain");
    }

    #[test]
    fn formats_bytes_at_each_scale() {
        assert_eq!(format_bytes(0), "0.0 B");
        assert_eq!(format_bytes(512), "512.0 B");
        assert_eq!(format_bytes(2048), "2.0 KB");
        assert_eq!(format_bytes(1_288_490_188), "1.2 GB");
    }
}
