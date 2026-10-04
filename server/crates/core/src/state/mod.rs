//! Internal state helpers shared across the server.
//!
//! - [`atomic`] — write files atomically via a sibling temp file.
//! - [`hash`] — SHA-256 hashing of files.
//! - [`timeutil`] — time and duration formatting/parsing.
//! - [`arm`] — the per-account arming window.
//! - [`relpath`] — safe share-relative path handling.

pub mod arm;
pub mod atomic;
pub mod hash;
pub mod relpath;
pub mod timeutil;
