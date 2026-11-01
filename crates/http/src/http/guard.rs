//! The request guard: authenticate once, then audit the outcome.
//!
//! Authentication runs on a blocking worker because argon2 is CPU-bound and
//! the account cache touches the filesystem. The audit write is also a
//! blocking task, awaited so a completed request always has its record on
//! disk before the client sees the response.

use std::net::SocketAddr;
use std::sync::Arc;
use std::time::Instant;

use axum::body::Body;
use axum::extract::{ConnectInfo, State};
use axum::http::{Request, StatusCode};
use axum::middleware::Next;
use axum::response::Response;
use tracing::Instrument as _;

use lanpull_core::audit::Record;
use lanpull_core::timeutil;

use crate::http::audit::{next_request_id, response_bytes, sanitize_host};
use crate::http::auth::basic_credentials;
use crate::http::{respond, AppState};

/// Authenticate the request and write one audit record for the outcome.
pub(super) async fn guard(
    State(state): State<AppState>,
    ConnectInfo(peer): ConnectInfo<SocketAddr>,
    mut request: Request<Body>,
    next: Next,
) -> Response {
    let method = request.method().clone();
    let path = request.uri().path().to_string();
    let host = sanitize_host(request.headers());
    let ip = peer.ip();
    let request_id = next_request_id();
    let started = Instant::now();
    let credentials = basic_credentials(request.headers());
    let attempted_user = credentials
        .as_ref()
        .map_or_else(String::new, |credentials| credentials.user.clone());

    let authenticator = Arc::clone(&state.auth);
    let verified =
        tokio::task::spawn_blocking(move || authenticator.authenticate(credentials, ip)).await;

    match verified {
        Ok(Ok(principal)) => {
            request.extensions_mut().insert(principal.clone());
            let span = tracing::info_span!(
                "request",
                id = %request_id,
                method = %method,
                path = %path
            );
            let response = next.run(request).instrument(span).await;
            let bytes = response_bytes(&response);
            let reason = (response.status() == StatusCode::FORBIDDEN)
                .then_some("forbidden")
                .map(str::to_string);
            record(
                &state,
                Record {
                    ts: timestamp(),
                    user: principal.name,
                    ip: ip.to_string(),
                    host,
                    method: method.to_string(),
                    path,
                    status: response.status().as_u16(),
                    bytes,
                    reason,
                    request_id,
                    latency_ms: elapsed_ms(started),
                },
            )
            .await;
            response
        }
        Ok(Err(error)) => {
            let response = respond::unauthorized(error.reason());
            record(
                &state,
                Record {
                    ts: timestamp(),
                    user: attempted_user,
                    ip: ip.to_string(),
                    host,
                    method: method.to_string(),
                    path,
                    status: StatusCode::UNAUTHORIZED.as_u16(),
                    bytes: 0,
                    reason: Some(error.reason().to_string()),
                    request_id,
                    latency_ms: elapsed_ms(started),
                },
            )
            .await;
            response
        }
        Err(e) => {
            tracing::error!(error = %e, "authentication worker failed");
            record(
                &state,
                Record {
                    ts: timestamp(),
                    user: attempted_user,
                    ip: ip.to_string(),
                    host,
                    method: method.to_string(),
                    path,
                    status: StatusCode::INTERNAL_SERVER_ERROR.as_u16(),
                    bytes: 0,
                    reason: Some("internal_error".to_string()),
                    request_id,
                    latency_ms: elapsed_ms(started),
                },
            )
            .await;
            respond::text_response(StatusCode::INTERNAL_SERVER_ERROR, "internal error")
        }
    }
}

/// Write one record off the async worker and wait for it to finish.
async fn record(state: &AppState, record: Record) {
    let sink = Arc::clone(&state.audit);
    if tokio::task::spawn_blocking(move || sink.record(&record))
        .await
        .is_err()
    {
        tracing::warn!("audit worker failed");
    }
}

/// The current time as an RFC 3339 UTC timestamp.
fn timestamp() -> String {
    timeutil::iso8601(timeutil::now_unix())
}

/// Milliseconds elapsed since `started`, saturating on overflow.
fn elapsed_ms(started: Instant) -> u64 {
    u64::try_from(started.elapsed().as_millis()).unwrap_or(u64::MAX)
}
