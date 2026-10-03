//! The HTTPS server.
//!
//! The server exposes exactly four authenticated routes under the reserved
//! `_lanpull/` prefix. File bytes are served by `tower-http`'s `ServeFile`,
//! which provides `Range` and traversal-safe handling; lanpull only routes,
//! authenticates, and enforces the reserved-prefix policy.

use std::net::{IpAddr, SocketAddr};
use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;

use axum::body::Body;
use axum::extract::{ConnectInfo, Extension, Path as AxumPath, State};
use axum::http::{header, HeaderMap, Method, Request, StatusCode};
use axum::middleware::{self, Next};
use axum::response::Response;
use axum::routing::get;
use axum::Router;
use base64::Engine as _;
use tower::ServiceExt as _;
use tower_http::catch_panic::CatchPanicLayer;
use tower_http::limit::RequestBodyLimitLayer;
use tower_http::services::ServeFile;
use tower_http::set_header::SetResponseHeaderLayer;
use tower_http::timeout::TimeoutLayer;

use crate::arm::ArmState;
use crate::audit::{self, Record};
use crate::clients::{Account, Clients, Verify};
use crate::config::Config;
use crate::error::{Error, Result};
use crate::timeutil;
use crate::{bundle, policy, relpath, status};

/// Maximum accepted request body size in bytes.
const MAX_BODY_BYTES: usize = 1024 * 1024;
/// Request timeout in seconds.
const REQUEST_TIMEOUT_SECS: u64 = 30;
/// Maximum accepted `X-Lanpull-Host` length.
const MAX_HOST_LEN: usize = 255;
/// Audit header carrying the rejection reason.
const REASON_HEADER: &str = "x-lanpull-reason";
/// Advisory client hostname header.
const HOST_HEADER: &str = "x-lanpull-host";

/// Shared server state.
///
/// Accounts and arm state are read from disk on every request so that
/// `remove-client`, `passwd`, `arm`, and `disarm` take effect immediately
/// without restarting the server (`docs/SPEC.md` sections 8 and 13).
#[derive(Debug, Clone)]
pub struct AppState {
    /// Server configuration.
    pub config: Arc<Config>,
}

/// The authenticated account name attached to a request by the `access` layer.
#[derive(Debug, Clone)]
struct User(String);

/// A rejected authentication attempt.
#[derive(Debug)]
struct Failure {
    /// Attempted account name, or empty when no credentials were sent.
    user: String,
    /// Machine-readable reason recorded in the audit log.
    reason: String,
}

/// Build the application router around the given state.
pub fn router(state: AppState) -> Router {
    Router::new()
        .route(
            "/_lanpull/share/{share}/manifest.json",
            get(serve_share_manifest),
        )
        .route(
            "/_lanpull/share/{share}/file/{*path}",
            get(serve_share_file),
        )
        .route("/_lanpull/client/manifest.json", get(serve_bundle_manifest))
        .route("/_lanpull/client/{file}", get(serve_bundle_file))
        .fallback(not_found)
        .layer(middleware::from_fn_with_state(state.clone(), access))
        .layer(SetResponseHeaderLayer::overriding(
            header::HeaderName::from_static("x-content-type-options"),
            header::HeaderValue::from_static("nosniff"),
        ))
        .layer(CatchPanicLayer::new())
        .layer(RequestBodyLimitLayer::new(MAX_BODY_BYTES))
        .layer(TimeoutLayer::with_status_code(
            StatusCode::REQUEST_TIMEOUT,
            Duration::from_secs(REQUEST_TIMEOUT_SECS),
        ))
        .with_state(state)
}

/// Authenticate a request once and apply the audit log around it.
async fn access(
    State(state): State<AppState>,
    ConnectInfo(peer): ConnectInfo<SocketAddr>,
    mut request: Request<Body>,
    next: Next,
) -> Response {
    let method = request.method().clone();
    let path = request.uri().path().to_string();
    let host = sanitize_host(request.headers());
    let ip = peer.ip();

    match authenticate(&state, request.headers(), ip) {
        Ok(account) => {
            let user = account.name;
            request.extensions_mut().insert(User(user.clone()));
            let response = next.run(request).await;
            let bytes = response_bytes(&response);
            let reason = (response.status() == StatusCode::FORBIDDEN)
                .then_some("forbidden")
                .map(str::to_string);
            write_audit(
                &state,
                &Record {
                    ts: timeutil::iso8601(timeutil::now_unix()),
                    user,
                    ip: ip.to_string(),
                    host,
                    method: method.to_string(),
                    path,
                    status: response.status().as_u16(),
                    bytes,
                    reason,
                },
            );
            response
        }
        Err(failure) => {
            let response = unauthorized(&failure.reason);
            write_audit(
                &state,
                &Record {
                    ts: timeutil::iso8601(timeutil::now_unix()),
                    user: failure.user,
                    ip: ip.to_string(),
                    host,
                    method: method.to_string(),
                    path,
                    status: 401,
                    bytes: 0,
                    reason: Some(failure.reason),
                },
            );
            response
        }
    }
}

/// Check credentials, IP binding, and the arm window, in that order.
fn authenticate(
    state: &AppState,
    headers: &HeaderMap,
    ip: IpAddr,
) -> std::result::Result<Account, Failure> {
    let Some((user, password)) = basic_credentials(headers) else {
        return Err(Failure {
            user: String::new(),
            reason: "unknown_user".to_string(),
        });
    };

    let clients = match Clients::load(&state.config.clients_path) {
        Ok(clients) => clients,
        Err(e) => {
            tracing::warn!("cannot read account file: {e}");
            return Err(Failure {
                user,
                reason: "unknown_user".to_string(),
            });
        }
    };

    let (account, password_ok) = match clients.verify(&user, &password) {
        Verify::Unknown => {
            return Err(Failure {
                user,
                reason: "unknown_user".to_string(),
            });
        }
        Verify::BadPassword(account) => (account, false),
        Verify::Ok(account) => (account, true),
    };
    if !password_ok {
        return Err(Failure {
            user,
            reason: "bad_password".to_string(),
        });
    }

    if let Some(allowed) = account.allowed_ip {
        if allowed != ip {
            return Err(Failure {
                user,
                reason: "foreign_ip".to_string(),
            });
        }
    }

    if !account.local {
        let armed = match ArmState::load(&state.config.arm_path()) {
            Ok(arm) => arm.is_armed(&account.name, timeutil::now_unix()),
            Err(e) => {
                tracing::warn!("cannot read arm state: {e}");
                false
            }
        };
        if !armed {
            return Err(Failure {
                user: account.name,
                reason: "not_armed".to_string(),
            });
        }
    }

    Ok(account)
}

/// Append an audit record, logging (but not failing) on error.
fn write_audit(state: &AppState, record: &Record) {
    if let Err(e) = audit::append(&state.config.audit_log, record) {
        tracing::warn!("cannot write audit log: {e}");
    }
}

/// Handler for `/_lanpull/share/<share>/manifest.json`.
async fn serve_share_manifest(
    State(state): State<AppState>,
    Extension(user): Extension<User>,
    AxumPath(share): AxumPath<String>,
    method: Method,
    headers: HeaderMap,
) -> Response {
    if !state.config.shares.contains_key(&share) {
        return not_found().await;
    }
    match policy::load_access(&state.config) {
        Ok(access) if access.allows_share(&user.0, &share) => {}
        Ok(_) => return forbidden(),
        Err(e) => {
            tracing::warn!("cannot read access policy: {e}");
            return forbidden();
        }
    }
    let path = state.config.access_manifest_path(&user.0, &share);
    match tokio::fs::metadata(&path).await {
        Ok(metadata) if metadata.is_file() => serve_file(path, method, headers).await,
        _ => text_response(
            StatusCode::SERVICE_UNAVAILABLE,
            "server has no manifest for this account; ask the operator to run make rescan",
        ),
    }
}

/// Handler for `/_lanpull/share/<share>/file/<path>`.
async fn serve_share_file(
    State(state): State<AppState>,
    Extension(user): Extension<User>,
    AxumPath((share, path)): AxumPath<(String, String)>,
    method: Method,
    headers: HeaderMap,
) -> Response {
    let Some(root) = state.config.shares.get(&share) else {
        return not_found().await;
    };
    if relpath::validate(&path).is_err() {
        return not_found().await;
    }
    match policy::load_access(&state.config) {
        Ok(access) if access.allows(&user.0, &share, &path) => {}
        Ok(_) => return forbidden(),
        Err(e) => {
            tracing::warn!("cannot read access policy: {e}");
            return forbidden();
        }
    }
    let Ok(full) = relpath::join(root, &path) else {
        return not_found().await;
    };
    match tokio::fs::symlink_metadata(&full).await {
        Ok(metadata) if metadata.is_file() && !metadata.file_type().is_symlink() => {
            serve_file(full, method, headers).await
        }
        _ => not_found().await,
    }
}

/// Handler for `/_lanpull/client/manifest.json`.
async fn serve_bundle_manifest(State(state): State<AppState>) -> Response {
    match bundle::build(&state.config.bundle_dir()) {
        Ok(manifest) => json_response(&manifest),
        Err(e) => text_response(StatusCode::SERVICE_UNAVAILABLE, &e.to_string()),
    }
}

/// Handler for `/_lanpull/client/<file>`.
async fn serve_bundle_file(
    State(state): State<AppState>,
    AxumPath(name): AxumPath<String>,
    method: Method,
    headers: HeaderMap,
) -> Response {
    if name.contains('/') || name.contains("..") {
        return not_found().await;
    }
    let directory = state.config.bundle_dir();
    let manifest = match bundle::build(&directory) {
        Ok(manifest) => manifest,
        Err(e) => return text_response(StatusCode::SERVICE_UNAVAILABLE, &e.to_string()),
    };
    if !bundle::contains(&manifest, &name) {
        return not_found().await;
    }
    let full = directory.join(&name);
    match tokio::fs::symlink_metadata(&full).await {
        Ok(metadata) if metadata.is_file() && !metadata.file_type().is_symlink() => {
            serve_file(full, method, headers).await
        }
        _ => not_found().await,
    }
}

/// Fallback handler for unknown paths.
async fn not_found() -> Response {
    text_response(StatusCode::NOT_FOUND, "not found")
}

/// Build a `403` response for a disallowed share or path.
fn forbidden() -> Response {
    Response::builder()
        .status(StatusCode::FORBIDDEN)
        .header(REASON_HEADER, "forbidden")
        .header(header::CONTENT_TYPE, "text/plain; charset=utf-8")
        .body(Body::from("forbidden\n"))
        .unwrap_or_else(|_| Response::new(Body::empty()))
}

/// Serve a file through `tower-http`'s `ServeFile`.
async fn serve_file(path: PathBuf, method: Method, headers: HeaderMap) -> Response {
    let mut request = Request::new(Body::empty());
    *request.method_mut() = method;
    *request.headers_mut() = headers;
    match ServeFile::new(path).oneshot(request).await {
        Ok(response) => {
            let (parts, body) = response.into_parts();
            Response::from_parts(parts, Body::new(body))
        }
        Err(e) => text_response(
            StatusCode::INTERNAL_SERVER_ERROR,
            &format!("cannot serve file: {e}"),
        ),
    }
}

/// Build a JSON response.
fn json_response<T: serde::Serialize>(value: &T) -> Response {
    match serde_json::to_vec(value) {
        Ok(bytes) => Response::builder()
            .status(StatusCode::OK)
            .header(header::CONTENT_TYPE, "application/json")
            .body(Body::from(bytes))
            .unwrap_or_else(|_| Response::new(Body::empty())),
        Err(e) => text_response(
            StatusCode::INTERNAL_SERVER_ERROR,
            &format!("cannot encode JSON: {e}"),
        ),
    }
}

/// Build a plain-text response.
fn text_response(status: StatusCode, message: &str) -> Response {
    Response::builder()
        .status(status)
        .header(header::CONTENT_TYPE, "text/plain; charset=utf-8")
        .body(Body::from(format!("{message}\n")))
        .unwrap_or_else(|_| Response::new(Body::empty()))
}

/// Build a `401` response carrying the rejection reason.
fn unauthorized(reason: &str) -> Response {
    Response::builder()
        .status(StatusCode::UNAUTHORIZED)
        .header(header::WWW_AUTHENTICATE, "Basic realm=\"lanpull\"")
        .header(REASON_HEADER, reason)
        .header(header::CONTENT_TYPE, "text/plain; charset=utf-8")
        .body(Body::from(format!("{reason}\n")))
        .unwrap_or_else(|_| Response::new(Body::empty()))
}

/// Decode HTTP Basic credentials.
fn basic_credentials(headers: &HeaderMap) -> Option<(String, String)> {
    let value = headers.get(header::AUTHORIZATION)?.to_str().ok()?;
    let (scheme, encoded) = value.split_once(' ')?;
    if !scheme.eq_ignore_ascii_case("basic") {
        return None;
    }
    let decoded = base64::engine::general_purpose::STANDARD
        .decode(encoded.trim())
        .ok()?;
    let text = String::from_utf8(decoded).ok()?;
    let (user, password) = text.split_once(':')?;
    Some((user.to_string(), password.to_string()))
}

/// Sanitize the advisory client hostname header.
fn sanitize_host(headers: &HeaderMap) -> String {
    let raw = headers
        .get(HOST_HEADER)
        .and_then(|value| value.to_str().ok())
        .unwrap_or("");
    raw.chars()
        .filter(|c| *c != '\r' && *c != '\n')
        .take(MAX_HOST_LEN)
        .collect()
}

/// Read the response size from `Content-Length` when present.
fn response_bytes(response: &Response) -> u64 {
    response
        .headers()
        .get(header::CONTENT_LENGTH)
        .and_then(|value| value.to_str().ok())
        .and_then(|value| value.parse::<u64>().ok())
        .unwrap_or(0)
}

/// Run the HTTPS server until shutdown.
pub async fn serve(config: Config) -> Result<()> {
    let clients = Clients::load(&config.clients_path)?;
    if clients.is_empty() {
        return Err(Error::Config(
            "no client accounts; run `lanpull add-client` first".to_string(),
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

    for warning in status::startup_warnings(&config) {
        tracing::warn!("{warning}");
    }

    let state = AppState {
        config: Arc::new(config.clone()),
    };

    let tls =
        axum_server::tls_rustls::RustlsConfig::from_pem_file(&config.cert_path, &config.key_path)
            .await
            .map_err(|e| Error::Server(format!("TLS setup failed: {e}")))?;

    let address = SocketAddr::new(config.bind, config.port);
    let handle = axum_server::Handle::new();
    let shutdown = handle.clone();
    tokio::spawn(async move {
        shutdown_signal().await;
        tracing::info!("shutting down");
        shutdown.graceful_shutdown(Some(Duration::from_secs(10)));
    });

    tracing::info!("listening on https://{address}");
    axum_server::bind_rustls(address, tls)
        .handle(handle)
        .serve(router(state).into_make_service_with_connect_info::<SocketAddr>())
        .await
        .map_err(|e| Error::Server(format!("server stopped: {e}")))
}

/// Resolve on `SIGINT` or `SIGTERM`.
async fn shutdown_signal() {
    let ctrl_c = async {
        let _ = tokio::signal::ctrl_c().await;
    };

    #[cfg(unix)]
    let terminate = async {
        match tokio::signal::unix::signal(tokio::signal::unix::SignalKind::terminate()) {
            Ok(mut signal) => {
                signal.recv().await;
            }
            Err(_) => std::future::pending::<()>().await,
        }
    };

    #[cfg(not(unix))]
    let terminate = std::future::pending::<()>();

    tokio::select! {
        () = ctrl_c => {},
        () = terminate => {},
    }
}
