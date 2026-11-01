//! Audit records and their sink.
//!
//! The guard writes one record per request through an [`AuditSink`]. The
//! default sink appends to the JSON-lines audit log; tests and embedders can
//! replace it without touching the request path.

use std::path::PathBuf;
use std::sync::atomic::{AtomicU64, Ordering};

use axum::http::{header, HeaderMap};
use axum::response::Response;

use lanpull_core::audit::{self, Record};

use crate::http::MAX_HOST_LEN;

/// Per-process request counter used to build a request identifier.
static REQUEST_SEQUENCE: AtomicU64 = AtomicU64::new(0);

/// Persists audit records.
pub trait AuditSink: Send + Sync {
    /// Record one finished request.
    fn record(&self, record: &Record);
}

/// Append records to the JSON-lines audit log.
#[derive(Debug, Clone)]
pub struct FileAuditSink {
    /// Path of the audit log.
    path: PathBuf,
}

impl FileAuditSink {
    /// Build a sink writing to `path`.
    pub const fn new(path: PathBuf) -> Self {
        Self { path }
    }
}

impl AuditSink for FileAuditSink {
    fn record(&self, record: &Record) {
        if let Err(e) = audit::append(&self.path, record) {
            tracing::warn!(error = %e, "cannot write audit log");
        }
    }
}

/// A sink that discards records.
#[derive(Debug, Default, Clone, Copy)]
pub struct NullAuditSink;

impl AuditSink for NullAuditSink {
    fn record(&self, _record: &Record) {}
}

/// Return the next per-process request identifier.
///
/// The value only has to be unique within one process lifetime; it is used to
/// correlate an audit line with a tracing span, not as a security token.
pub(super) fn next_request_id() -> String {
    let sequence = REQUEST_SEQUENCE.fetch_add(1, Ordering::Relaxed);
    format!("{:x}-{sequence:x}", std::process::id())
}

/// Sanitize the advisory client hostname header.
pub(super) fn sanitize_host(headers: &HeaderMap) -> String {
    let raw = headers
        .get("x-lanpull-host")
        .and_then(|value| value.to_str().ok())
        .unwrap_or("");
    raw.chars()
        .filter(|c| *c != '\r' && *c != '\n')
        .take(MAX_HOST_LEN)
        .collect()
}

/// Read the response size from `Content-Length` when present.
pub(super) fn response_bytes(response: &Response) -> u64 {
    response
        .headers()
        .get(header::CONTENT_LENGTH)
        .and_then(|value| value.to_str().ok())
        .and_then(|value| value.parse::<u64>().ok())
        .unwrap_or(0)
}
