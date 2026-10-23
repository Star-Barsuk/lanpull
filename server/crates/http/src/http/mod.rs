//! The HTTPS server: state, router, and shared limits.
//!
//! The request path is split by concern: `auth` verifies credentials, `audit`
//! persists records, `guard` wires the two around a request, `routes` owns the
//! domain endpoints, `files` serves bytes safely, and `serve` owns TLS and
//! shutdown. Only `routes` and the domain wrapper below know about shares and
//! manifests; the rest is transport that another service can reuse as-is.

mod audit;
mod auth;
mod files;
mod guard;
mod respond;
mod routes;
mod serve;

use std::sync::Arc;
use std::time::Duration;

use axum::http::{header, HeaderValue, StatusCode};
use axum::middleware;
use axum::routing::get;
use axum::Router;
use tower_http::catch_panic::CatchPanicLayer;
use tower_http::limit::RequestBodyLimitLayer;
use tower_http::set_header::SetResponseHeaderLayer;
use tower_http::timeout::TimeoutLayer;

use lanpull_core::access::Access;
use lanpull_core::clients::Clients;
use lanpull_core::config::Config;
use lanpull_core::error::{Error, Result};
use lanpull_core::live::LiveCache;

pub use audit::{AuditSink, FileAuditSink, NullAuditSink};
pub use auth::{AuthError, Authenticator, Credentials, Principal};
pub use serve::serve_tls;

use crate::http::auth::LanpullAuthenticator;

/// Maximum accepted request body size in bytes.
pub(crate) const MAX_BODY_BYTES: usize = 1024 * 1024;
/// Request timeout in seconds.
pub(crate) const REQUEST_TIMEOUT_SECS: u64 = 30;
/// Maximum accepted `X-Lanpull-Host` length.
pub(crate) const MAX_HOST_LEN: usize = 255;

/// Shared server state.
///
/// Accounts, the access policy, and arm state are re-read (through
/// [`LiveCache`]) on every request so that `account remove`, `account passwd`,
/// `account arm`, `account disarm`, and policy edits take effect immediately
/// without restarting the server (`docs/SPEC.md` sections 8 and 13).
#[derive(Clone)]
pub struct AppState {
    /// Server configuration.
    pub config: Arc<Config>,
    /// Cache of accounts, policy, and arm state, invalidated on file change.
    pub(crate) live: Arc<LiveCache>,
    /// Credential verifier; replaceable for tests and for other deployments.
    pub(crate) auth: Arc<dyn Authenticator>,
    /// Audit sink; replaceable for tests and for other deployments.
    pub(crate) audit: Arc<dyn AuditSink>,
}

impl std::fmt::Debug for AppState {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("AppState")
            .field("config", &self.config)
            .finish_non_exhaustive()
    }
}

impl AppState {
    /// Build state around a configuration.
    pub fn new(config: Config) -> Self {
        let config = Arc::new(config);
        let live = Arc::new(LiveCache::new());
        let auth = Arc::new(LanpullAuthenticator::new(
            Arc::clone(&config),
            Arc::clone(&live),
        ));
        let audit = Arc::new(FileAuditSink::new(config.audit_log.clone()));
        Self {
            config,
            live,
            auth,
            audit,
        }
    }

    /// Replace the credential verifier.
    #[must_use]
    pub fn with_authenticator(mut self, authenticator: Arc<dyn Authenticator>) -> Self {
        self.auth = authenticator;
        self
    }

    /// Replace the audit sink.
    #[must_use]
    pub fn with_audit(mut self, audit: Arc<dyn AuditSink>) -> Self {
        self.audit = audit;
        self
    }

    /// Return the expanded access policy, reloading it when a file changed.
    pub(crate) fn access(&self) -> Result<Arc<Access>> {
        self.live.access(&self.config)
    }
}

/// Build the application router around the given state.
pub fn router(state: AppState) -> Router {
    Router::new()
        .route(
            "/_lanpull/share/{share}/manifest.json",
            get(routes::serve_share_manifest),
        )
        .route(
            "/_lanpull/share/{share}/file/{*path}",
            get(routes::serve_share_file),
        )
        .route(
            "/_lanpull/client/manifest.json",
            get(routes::serve_bundle_manifest),
        )
        .route("/_lanpull/client/{file}", get(routes::serve_bundle_file))
        .fallback(respond::not_found)
        .layer(middleware::from_fn_with_state(state.clone(), guard::guard))
        .layer(SetResponseHeaderLayer::overriding(
            header::HeaderName::from_static("x-content-type-options"),
            HeaderValue::from_static("nosniff"),
        ))
        .layer(CatchPanicLayer::new())
        .layer(RequestBodyLimitLayer::new(MAX_BODY_BYTES))
        .layer(TimeoutLayer::with_status_code(
            StatusCode::REQUEST_TIMEOUT,
            Duration::from_secs(REQUEST_TIMEOUT_SECS),
        ))
        .with_state(state)
}

/// Run the HTTPS server until shutdown.
///
/// This wrapper owns the lanpull preconditions (accounts, certificate, startup
/// warnings); the transport itself is [`serve_tls`].
pub async fn serve(config: Config) -> Result<()> {
    let clients = Clients::load(&config.clients_path)?;
    if clients.is_empty() {
        return Err(Error::Config(
            "no client accounts; run 'lanpull account add' first".to_string(),
        ));
    }
    if !config.cert_path.is_file() {
        return Err(Error::Config(format!(
            "certificate not found at {}; run `lanpull cert`",
            config.cert_path.display()
        )));
    }
    if !config.key_path.is_file() {
        return Err(Error::Config(format!(
            "private key not found at {}; run `lanpull cert`",
            config.key_path.display()
        )));
    }

    for warning in lanpull_store::status::startup_warnings(&config) {
        tracing::warn!("{warning}");
    }

    let state = AppState::new(config.clone());
    serve_tls(
        config.server_settings(),
        router(state),
        serve::shutdown_signal(),
    )
    .await
}
