//! Safe file serving.
//!
//! `tower-http`'s `ServeFile` provides `Range`, conditionals, and traversal-safe
//! handling. This module adds the lanpull-specific hardening: the final
//! component is opened with `O_NOFOLLOW` and served through the already-open
//! descriptor, so it cannot be swapped for a symlink between the check and the
//! open.

use std::path::{Path, PathBuf};

use axum::body::Body;
use axum::http::{HeaderMap, Method, Request, StatusCode};
use axum::response::Response;
use tower::ServiceExt as _;
use tower_http::services::ServeFile;

use crate::http::respond;

/// Open `path` for reading without following a symlink as the final component.
pub(super) fn open_nofollow(path: &Path) -> std::io::Result<std::fs::File> {
    use std::os::unix::fs::OpenOptionsExt as _;
    std::fs::OpenOptions::new()
        .read(true)
        .custom_flags(libc::O_NOFOLLOW)
        .open(path)
}

/// Serve a file through `tower-http`'s `ServeFile`.
pub(super) async fn serve_file(path: PathBuf, method: Method, headers: HeaderMap) -> Response {
    let mut request = Request::new(Body::empty());
    *request.method_mut() = method;
    *request.headers_mut() = headers;
    match ServeFile::new(path).oneshot(request).await {
        Ok(response) => {
            let (parts, body) = response.into_parts();
            Response::from_parts(parts, Body::new(body))
        }
        Err(e) => respond::text_response(
            StatusCode::INTERNAL_SERVER_ERROR,
            &format!("cannot serve file: {e}"),
        ),
    }
}
