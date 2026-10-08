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

#[cfg(test)]
mod tests {
    #![allow(clippy::missing_assert_message)]

    use super::*;

    #[test]
    fn formats_bytes_at_each_scale() {
        assert_eq!(format_bytes(0), "0.0 B");
        assert_eq!(format_bytes(512), "512.0 B");
        assert_eq!(format_bytes(2048), "2.0 KB");
        assert_eq!(format_bytes(1_288_490_188), "1.2 GB");
    }
}
