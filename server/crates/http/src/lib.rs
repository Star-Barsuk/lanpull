//! lanpull HTTPS server.
//!
//! The axum router, request handlers, authentication middleware, and server
//! startup. Built on [`lanpull_core`] for configuration and policy and on
//! [`lanpull_store`] for manifests and share paths.

pub mod http;

pub use http::*;
