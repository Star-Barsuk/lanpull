//! Response construction.
//!
//! One place turns a domain error into a status code and one place builds each
//! response shape, so handlers never assemble status codes by hand.

use axum::body::Body;
use axum::http::{header, StatusCode};
use axum::response::Response;
use serde::Serialize;

use lanpull_core::error::Error;

/// Header carrying the machine-readable rejection reason.
pub(super) const REASON_HEADER: &str = "x-lanpull-reason";

/// Fallback handler for unknown paths.
pub(super) async fn not_found() -> Response {
    text_response(StatusCode::NOT_FOUND, "not found")
}

/// Build a `403` response for a disallowed share or path.
pub(super) fn forbidden() -> Response {
    Response::builder()
        .status(StatusCode::FORBIDDEN)
        .header(REASON_HEADER, "forbidden")
        .header(header::CONTENT_TYPE, "text/plain; charset=utf-8")
        .body(Body::from("forbidden\n"))
        .unwrap_or_else(|_| Response::new(Body::empty()))
}

/// Build a `401` response carrying the rejection reason.
pub(super) fn unauthorized(reason: &str) -> Response {
    Response::builder()
        .status(StatusCode::UNAUTHORIZED)
        .header(header::WWW_AUTHENTICATE, "Basic realm=\"lanpull\"")
        .header(REASON_HEADER, reason)
        .header(header::CONTENT_TYPE, "text/plain; charset=utf-8")
        .body(Body::from(format!("{reason}\n")))
        .unwrap_or_else(|_| Response::new(Body::empty()))
}

/// Build a response for a domain error, using its HTTP mapping.
pub(super) fn error_response(error: &Error) -> Response {
    let status =
        StatusCode::from_u16(error.http_status()).unwrap_or(StatusCode::INTERNAL_SERVER_ERROR);
    text_response(status, &error.to_string())
}

/// Build a JSON response.
pub(super) fn json_response<T: Serialize>(value: &T) -> Response {
    match serde_json::to_vec(value) {
        Ok(bytes) => Response::builder()
            .status(StatusCode::OK)
            .header(header::CONTENT_TYPE, "application/json")
            .header(header::CONTENT_LENGTH, bytes.len().to_string())
            .body(Body::from(bytes))
            .unwrap_or_else(|_| Response::new(Body::empty())),
        Err(e) => text_response(
            StatusCode::INTERNAL_SERVER_ERROR,
            &format!("cannot encode JSON: {e}"),
        ),
    }
}

/// Build a plain-text response.
pub(super) fn text_response(status: StatusCode, message: &str) -> Response {
    Response::builder()
        .status(status)
        .header(header::CONTENT_TYPE, "text/plain; charset=utf-8")
        .body(Body::from(format!("{message}\n")))
        .unwrap_or_else(|_| Response::new(Body::empty()))
}
