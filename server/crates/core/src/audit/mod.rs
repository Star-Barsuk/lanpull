//! The audit log and its report.
//!
//! Every request, including rejected ones, is appended to
//! `$STATE_DIR/access.log` as one JSON line. `lanpull report` summarizes it.

use std::collections::BTreeMap;
use std::io::{BufRead, BufReader, Write};
use std::path::Path;

use serde::{Deserialize, Serialize};

use crate::error::Result;
use crate::timeutil;

/// The path prefix shared by every share-scoped route.
const SHARE_ROUTE_PREFIX: &str = "/_lanpull/share/";
/// The marker that separates a share from its file path.
const FILE_MARKER: &str = "/file/";

/// Return `true` for a share data-file download (`/_lanpull/share/<share>/file/<path>`).
fn is_data_file(path: &str) -> bool {
    path.starts_with(SHARE_ROUTE_PREFIX) && path.contains(FILE_MARKER)
}

/// One audit log record.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Record {
    /// RFC 3339 UTC timestamp.
    pub ts: String,
    /// Authenticated account name, or the attempted name on rejection.
    pub user: String,
    /// Source IP address.
    pub ip: String,
    /// Sanitized `X-Lanpull-Host` value, or empty when absent.
    pub host: String,
    /// HTTP method.
    pub method: String,
    /// Request path.
    pub path: String,
    /// HTTP status code.
    pub status: u16,
    /// Response body size in bytes when known.
    pub bytes: u64,
    /// Rejection reason, when the request was refused.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub reason: Option<String>,
}

/// Append one record as a JSON line.
pub fn append(path: &Path, record: &Record) -> Result<()> {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    let mut line = serde_json::to_string(record)?;
    line.push('\n');
    let mut file = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(path)?;
    file.write_all(line.as_bytes())?;
    Ok(())
}

/// Per-account aggregate of the audit log.
#[derive(Debug, Default, Clone, Serialize, Deserialize)]
pub struct AccountSummary {
    /// Account name.
    pub user: String,
    /// Timestamp of the most recent request.
    pub last_seen: String,
    /// Hostname reported by the most recent request.
    pub last_host: String,
    /// Number of data files transferred.
    pub files: u64,
    /// Total bytes transferred.
    pub bytes: u64,
    /// Number of rejected requests.
    pub rejected: u64,
    /// Rejected requests by reason, sorted by reason.
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub by_reason: BTreeMap<String, u64>,
}

/// The whole audit report.
#[derive(Debug, Default, Clone, Serialize, Deserialize)]
pub struct Report {
    /// Aggregates, one per account, sorted by name.
    pub accounts: Vec<AccountSummary>,
}

/// Summarize the audit log, optionally filtered by account and start time.
pub fn summarize(
    path: &Path,
    user_filter: Option<&str>,
    since_epoch: Option<i64>,
) -> Result<Report> {
    let file = match std::fs::File::open(path) {
        Ok(file) => file,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(Report::default()),
        Err(e) => return Err(e.into()),
    };

    let mut accounts: BTreeMap<String, AccountSummary> = BTreeMap::new();
    let mut latest: BTreeMap<String, i64> = BTreeMap::new();

    for line in BufReader::new(file).lines() {
        let line = line?;
        if line.trim().is_empty() {
            continue;
        }
        let record: Record = match serde_json::from_str(&line) {
            Ok(record) => record,
            Err(e) => {
                tracing::warn!(error = %e, "skipping malformed audit line");
                continue;
            }
        };
        if let Some(filter) = user_filter {
            if record.user != filter {
                continue;
            }
        }
        let epoch = timeutil::parse_iso8601(&record.ts).unwrap_or(0);
        if let Some(cutoff) = since_epoch {
            if epoch < cutoff {
                continue;
            }
        }

        let summary = accounts
            .entry(record.user.clone())
            .or_insert_with(|| AccountSummary {
                user: record.user.clone(),
                last_seen: record.ts.clone(),
                last_host: record.host.clone(),
                ..AccountSummary::default()
            });

        if latest.get(&record.user).is_none_or(|seen| epoch >= *seen) {
            latest.insert(record.user.clone(), epoch);
            summary.last_seen.clone_from(&record.ts);
            summary.last_host.clone_from(&record.host);
        }

        if let Some(reason) = record.reason.as_deref() {
            summary.rejected = summary.rejected.saturating_add(1);
            let counter = summary.by_reason.entry(reason.to_string()).or_insert(0);
            *counter = counter.saturating_add(1);
        } else if record.status == 401 {
            summary.rejected = summary.rejected.saturating_add(1);
            let counter = summary
                .by_reason
                .entry(format!("http_{}", record.status))
                .or_insert(0);
            *counter = counter.saturating_add(1);
        } else if (record.status == 200 || record.status == 206) && is_data_file(&record.path) {
            summary.files = summary.files.saturating_add(1);
            summary.bytes = summary.bytes.saturating_add(record.bytes);
        }
    }

    Ok(Report {
        accounts: accounts.into_values().collect(),
    })
}

/// Read every audit record matching the filters, in file order.
///
/// Used by `report --tail`; unlike [`summarize`], the records are returned
/// verbatim so the caller can show the most recent ones.
pub fn read_records(
    path: &Path,
    user_filter: Option<&str>,
    since_epoch: Option<i64>,
) -> Result<Vec<Record>> {
    let file = match std::fs::File::open(path) {
        Ok(file) => file,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(Vec::new()),
        Err(e) => return Err(e.into()),
    };

    let mut records = Vec::new();
    for line in BufReader::new(file).lines() {
        let line = line?;
        if line.trim().is_empty() {
            continue;
        }
        let record: Record = match serde_json::from_str(&line) {
            Ok(record) => record,
            Err(e) => {
                tracing::warn!(error = %e, "skipping malformed audit line");
                continue;
            }
        };
        if let Some(filter) = user_filter {
            if record.user != filter {
                continue;
            }
        }
        if let Some(cutoff) = since_epoch {
            let epoch = timeutil::parse_iso8601(&record.ts).unwrap_or(0);
            if epoch < cutoff {
                continue;
            }
        }
        records.push(record);
    }
    Ok(records)
}
