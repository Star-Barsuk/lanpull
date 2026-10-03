//! lanpull server library.
//!
//! The crate implements every server-side concern of lanpull: configuration,
//! the share walk and manifest cache, per-client accounts, the arm window, the
//! audit log, certificate generation, the client bundle, and the HTTPS server.
//! The `lanpull` binary in `src/bin/lanpull.rs` is a thin CLI over this API.

pub mod access;
pub mod arm;
pub mod atomic;
pub mod audit;
pub mod bundle;
pub mod cache;
pub mod cert;
pub mod clients;
pub mod config;
pub mod error;
pub mod hash;
pub mod http;
pub mod ignore;
pub mod manifest;
pub mod policy;
pub mod relpath;
pub mod status;
pub mod timeutil;

pub use error::{Error, Result};
