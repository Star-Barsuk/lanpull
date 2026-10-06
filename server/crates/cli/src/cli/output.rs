//! Uniform command output: human-readable lines or a JSON envelope.
//!
//! A command returns an [`Outcome`]; [`emit`] renders it. In text mode the
//! lines go to stdout and the warnings to stderr. With `--json` stdout carries
//! a single envelope document `{status, command, data, warnings}`.
//!
//! The rendering is split into pure [`render_ok`]/[`render_error`] functions so
//! the exact stdout/stderr text can be unit-tested; [`emit`]/[`emit_error`] are
//! thin wrappers that write them to the process streams.

use lanpull_core::error::{Error, Result};
use serde::Serialize;

/// One command's result: formatted lines plus a structured payload.
#[derive(Debug, Default)]
pub struct Outcome {
    /// Lines printed to stdout in text mode (already formatted).
    pub lines: Vec<String>,
    /// Structured payload for `--json` (defaults to JSON `null`).
    pub data: serde_json::Value,
    /// Warnings printed to stderr in text mode and embedded in the envelope.
    pub warnings: Vec<String>,
}

impl Outcome {
    /// An empty outcome.
    pub fn new() -> Self {
        Self::default()
    }

    /// An outcome whose only content is human-readable text.
    pub fn text(lines: impl IntoIterator<Item = String>) -> Self {
        Self {
            lines: lines.into_iter().collect(),
            ..Self::default()
        }
    }

    /// Attach a structured payload.
    pub fn with_data<T: Serialize>(mut self, data: &T) -> Self {
        self.data = serde_json::to_value(data).unwrap_or(serde_json::Value::Null);
        self
    }

    /// Append one text line.
    pub fn line(mut self, line: impl Into<String>) -> Self {
        self.lines.push(line.into());
        self
    }

    /// Append one warning.
    pub fn warn(mut self, warning: impl Into<String>) -> Self {
        self.warnings.push(warning.into());
        self
    }
}

/// Render a successful outcome to `(stdout, stderr)`.
///
/// Text mode puts the lines on stdout and the `warning: ` diagnostics on
/// stderr; JSON mode puts one envelope document on stdout and nothing on
/// stderr.
pub fn render_ok(command: &str, outcome: &Outcome, json: bool) -> Result<(String, String)> {
    if json {
        let envelope = serde_json::json!({
            "status": "ok",
            "command": command,
            "data": outcome.data,
            "warnings": outcome.warnings,
        });
        let text = serde_json::to_string_pretty(&envelope).map_err(Error::Json)?;
        return Ok((format!("{text}\n"), String::new()));
    }
    let mut stdout = String::new();
    for line in &outcome.lines {
        stdout.push_str(line);
        stdout.push('\n');
    }
    let mut stderr = String::new();
    for warning in &outcome.warnings {
        stderr.push_str("warning: ");
        stderr.push_str(warning);
        stderr.push('\n');
    }
    Ok((stdout, stderr))
}

/// Render a failed command to `(stdout, stderr)`.
///
/// JSON mode puts one error envelope on stdout; text mode puts `error: ` and,
/// when available, `hint: ` on stderr. Stdout is empty in text mode.
pub fn render_error(command: &str, error: &Error, json: bool) -> (String, String) {
    if json {
        let envelope = serde_json::json!({
            "status": "error",
            "command": command,
            "code": error.exit_code(),
            "message": error.to_string(),
            "hint": error.hint(),
        });
        return serde_json::to_string_pretty(&envelope).map_or_else(
            |_| (String::new(), format!("error: {error}\n")),
            |text| (format!("{text}\n"), String::new()),
        );
    }
    let mut stderr = format!("error: {error}\n");
    if let Some(hint) = error.hint() {
        stderr.push_str("hint: ");
        stderr.push_str(hint);
        stderr.push('\n');
    }
    (String::new(), stderr)
}

/// Render an outcome and write it to the process streams.
pub fn emit(command: &str, outcome: Outcome, json: bool) -> Result<()> {
    let (stdout, stderr) = render_ok(command, &outcome, json)?;
    write_streams(&stdout, &stderr);
    Ok(())
}

/// Render an error and write it to the process streams.
pub fn emit_error(command: &str, error: &Error, json: bool) {
    let (stdout, stderr) = render_error(command, error, json);
    write_streams(&stdout, &stderr);
}

/// Write the rendered streams, skipping empty ones.
fn write_streams(stdout: &str, stderr: &str) {
    if !stdout.is_empty() {
        print!("{stdout}");
    }
    if !stderr.is_empty() {
        eprint!("{stderr}");
    }
}

/// Format rows as an aligned table, padding every column but the last.
///
/// Returns one line per row. An empty input yields an empty vector.
pub fn format_rows(rows: &[Vec<String>]) -> Vec<String> {
    let columns = rows.iter().map(Vec::len).max().unwrap_or(0);
    let mut widths = vec![0_usize; columns];
    for row in rows {
        for (index, cell) in row.iter().enumerate() {
            if let Some(width) = widths.get_mut(index) {
                *width = (*width).max(cell.chars().count());
            }
        }
    }
    rows.iter()
        .map(|row| {
            let last = row.len().saturating_sub(1);
            let mut line = String::new();
            for (index, cell) in row.iter().enumerate() {
                if index == last {
                    line.push_str(cell);
                } else {
                    let width = widths.get(index).copied().unwrap_or(0);
                    let pad = width.saturating_sub(cell.chars().count()).saturating_add(2);
                    line.push_str(cell);
                    line.push_str(&" ".repeat(pad));
                }
            }
            line
        })
        .collect()
}

#[cfg(test)]
mod tests {
    // Tests may unwrap and use bare asserts for brevity; production code may not.
    #![allow(
        clippy::unwrap_used,
        clippy::expect_used,
        clippy::panic,
        clippy::indexing_slicing,
        clippy::missing_assert_message
    )]

    use super::*;
    use serde_json::Value;

    fn outcome() -> Outcome {
        Outcome::text(vec!["first".to_string(), "second".to_string()])
            .warn("careful")
            .with_data(&serde_json::json!({ "key": "value" }))
    }

    #[test]
    fn text_success_splits_stdout_and_warnings() {
        let (stdout, stderr) = render_ok("share list", &outcome(), false).unwrap();
        assert_eq!(stdout, "first\nsecond\n");
        assert_eq!(stderr, "warning: careful\n");
    }

    #[test]
    fn json_success_is_one_envelope_on_stdout() {
        let (stdout, stderr) = render_ok("share list", &outcome(), true).unwrap();
        assert_eq!(stderr, "");
        assert!(stdout.ends_with('\n'));
        let value: Value = serde_json::from_str(&stdout).unwrap();
        assert_eq!(value.get("status").and_then(Value::as_str), Some("ok"));
        assert_eq!(
            value.get("command").and_then(Value::as_str),
            Some("share list")
        );
        assert_eq!(
            value
                .get("data")
                .and_then(|d| d.get("key"))
                .and_then(Value::as_str),
            Some("value")
        );
        assert_eq!(
            value
                .get("warnings")
                .and_then(Value::as_array)
                .map(Vec::len),
            Some(1)
        );
    }

    #[test]
    fn json_success_without_data_is_null() {
        let (stdout, _) = render_ok("status", &Outcome::new(), true).unwrap();
        let value: Value = serde_json::from_str(&stdout).unwrap();
        assert!(value.get("data").unwrap().is_null());
        assert_eq!(
            value
                .get("warnings")
                .and_then(Value::as_array)
                .map(Vec::len),
            Some(0)
        );
    }

    #[test]
    fn text_error_has_error_and_hint_lines() {
        let (stdout, stderr) = render_error("share add", &Error::Usage("bad use".into()), false);
        assert_eq!(stdout, "");
        assert_eq!(
            stderr,
            "error: bad use\nhint: run 'lanpull --help' for usage\n"
        );
    }

    #[test]
    fn text_error_without_hint_has_only_the_error_line() {
        let error = Error::Io(std::io::Error::other("boom"));
        let (stdout, stderr) = render_error("serve", &error, false);
        assert_eq!(stdout, "");
        assert_eq!(stderr, "error: I/O error: boom\n");
    }

    #[test]
    fn json_error_is_one_envelope_on_stdout() {
        let (stdout, stderr) = render_error("config get", &Error::Config("nope".into()), true);
        assert_eq!(stderr, "");
        let value: Value = serde_json::from_str(&stdout).unwrap();
        assert_eq!(value.get("status").and_then(Value::as_str), Some("error"));
        assert_eq!(
            value.get("command").and_then(Value::as_str),
            Some("config get")
        );
        assert_eq!(value.get("code").and_then(Value::as_u64), Some(1));
        assert_eq!(
            value.get("message").and_then(Value::as_str),
            Some("configuration error: nope")
        );
        assert_eq!(
            value.get("hint").and_then(Value::as_str),
            Some("run 'lanpull config show' to inspect the configuration")
        );
    }

    #[test]
    fn json_error_hint_is_null_when_absent() {
        let error = Error::Cancelled("stopped".into());
        let (stdout, _) = render_error("account remove", &error, true);
        let value: Value = serde_json::from_str(&stdout).unwrap();
        assert_eq!(value.get("code").and_then(Value::as_u64), Some(1));
        assert!(value.get("hint").unwrap().is_null());
    }

    #[test]
    fn format_rows_empty_input_is_empty() {
        assert!(format_rows(&[]).is_empty());
    }

    #[test]
    fn format_rows_pads_every_column_but_the_last() {
        let rows = vec![
            vec!["a".to_string(), "bb".to_string(), "c".to_string()],
            vec!["aaa".to_string(), "b".to_string(), "ccc".to_string()],
        ];
        let lines = format_rows(&rows);
        // Column 0 width 3, column 1 width 2; two spaces of separation.
        assert_eq!(lines[0], "a    bb  c");
        assert_eq!(lines[1], "aaa  b   ccc");
    }

    #[test]
    fn format_rows_handles_a_single_column() {
        assert_eq!(format_rows(&[vec!["only".to_string()]]), vec!["only"]);
    }

    #[test]
    fn format_rows_handles_ragged_rows() {
        let rows = vec![
            vec!["a".to_string(), "b".to_string()],
            vec!["c".to_string()],
        ];
        assert_eq!(format_rows(&rows), vec!["a  b", "c"]);
    }

    #[test]
    fn format_rows_measures_unicode_by_char() {
        let rows = vec![
            vec!["файл".to_string(), "x".to_string()],
            vec!["a".to_string(), "y".to_string()],
        ];
        // Width 4 for the first column, so the short cell is padded to it.
        assert_eq!(format_rows(&rows)[1], "a     y");
    }
}
