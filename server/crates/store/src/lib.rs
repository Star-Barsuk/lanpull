//! lanpull store library.
//!
//! Builds the per-share data manifests and their hash cache from the share
//! directories, serves the staged client bundle metadata, and reports operator
//! status. It depends on [`lanpull_core`] but never on the network layer, so
//! the manifest logic is testable in isolation.

pub mod bundle;
pub mod cache;
pub mod manifest;
pub mod status;
