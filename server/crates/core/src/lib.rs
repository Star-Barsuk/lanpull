//! lanpull core library.
//!
//! Configuration, the JSON access policy, per-client accounts, TLS material,
//! and the internal state helpers. The crate has no network dependency, so all
//! of it is unit-testable without a server.
//!
//! The submodules are grouped by concern:
//! - [`config`] — server configuration and path resolution.
//! - [`policy`] — the JSON access policy and its compiled glob rules.
//! - [`network`] — named per-network server address and TLS profiles.
//! - [`glob`] — the share-relative path glob engine.
//! - [`ignore`] — built-in and per-share `.lanpullignore` patterns.
//! - [`account`] — accounts, password hashing, and client-folder staging.
//! - [`tls`] — self-signed certificate generation.
//! - [`state`] — atomic writes, hashing, time, and the arm window.
//! - [`audit`], [`live`], [`throttle`] — audit log, live config cache, and
//!   failed-authentication throttling.

pub mod account;
pub mod audit;
pub mod config;
pub mod error;
pub mod glob;
pub mod human;
pub mod ignore;
pub mod live;
pub mod network;
pub mod policy;
pub mod state;
pub mod throttle;
pub mod tls;

// Flat re-exports keep call sites short without hiding the grouping.
pub use account::clients;
pub use policy::access;
pub use state::{arm, atomic, hash, relpath, timeutil};
pub use tls as cert;

pub use error::{Error, Result};
