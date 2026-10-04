//! Uniform command output: human-readable lines or a JSON envelope.
//!
//! A command returns an [`Outcome`]; [`emit`] renders it. In text mode the
//! lines go to stdout and the warnings to stderr. With `--json` stdout carries
//! a single envelope document `{status, command, data, warnings}`.

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

/// Render an outcome to the streams.
///
/// Text mode writes the lines to stdout and the warnings to stderr; JSON mode
/// writes one envelope document to stdout.
pub fn emit(command: &str, outcome: Outcome, json: bool) -> Result<()> {
    if json {
        let envelope = serde_json::json!({
            "status": "ok",
            "command": command,
            "data": outcome.data,
            "warnings": outcome.warnings,
        });
        let text = serde_json::to_string_pretty(&envelope).map_err(Error::Json)?;
        println!("{text}");
        return Ok(());
    }
    for warning in &outcome.warnings {
        eprintln!("warning: {warning}");
    }
    for line in &outcome.lines {
        println!("{line}");
    }
    Ok(())
}

/// Render the error envelope for `--json`, or the textual `error:`/`hint:` pair.
pub fn emit_error(command: &str, error: &Error, json: bool) {
    if json {
        let envelope = serde_json::json!({
            "status": "error",
            "command": command,
            "code": error.exit_code(),
            "message": error.to_string(),
            "hint": error.hint(),
        });
        match serde_json::to_string_pretty(&envelope) {
            Ok(text) => println!("{text}"),
            Err(_) => eprintln!("error: {error}"),
        }
        return;
    }
    eprintln!("error: {error}");
    if let Some(hint) = error.hint() {
        eprintln!("hint: {hint}");
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
