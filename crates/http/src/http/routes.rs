//! The lanpull endpoints.
//!
//! This is the only module in the HTTP crate that knows about shares,
//! manifests, and the client bundle; everything else is transport.

use std::os::unix::io::AsRawFd as _;
use std::path::PathBuf;

use axum::extract::{Extension, Path as AxumPath, State};
use axum::http::{HeaderMap, Method, StatusCode};
use axum::response::Response;

use lanpull_core::relpath;
use lanpull_store::bundle;

use crate::http::auth::Principal;
use crate::http::files;
use crate::http::respond;
use crate::http::AppState;

/// Handler for `/_lanpull/share/<share>/manifest.json`.
pub(super) async fn serve_share_manifest(
    State(state): State<AppState>,
    Extension(user): Extension<Principal>,
    AxumPath(share): AxumPath<String>,
    method: Method,
    headers: HeaderMap,
) -> Response {
    if !state.config.shares.contains_key(&share) {
        return respond::not_found().await;
    }
    match state.access() {
        Ok(access) if access.allows_share(&user.name, &share) => {}
        Ok(_) => return respond::forbidden(),
        Err(e) => {
            tracing::warn!(error = %e, "cannot read access policy");
            return respond::forbidden();
        }
    }
    let path = state.config.access_manifest_path(&user.name, &share);
    match tokio::fs::metadata(&path).await {
        Ok(metadata) if metadata.is_file() => files::serve_file(path, method, headers).await,
        _ => respond::text_response(
            StatusCode::SERVICE_UNAVAILABLE,
            "server has no manifest for this account; run 'lanpull share rescan'",
        ),
    }
}

/// Handler for `/_lanpull/share/<share>/file/<path>`.
pub(super) async fn serve_share_file(
    State(state): State<AppState>,
    Extension(user): Extension<Principal>,
    AxumPath((share, path)): AxumPath<(String, String)>,
    method: Method,
    headers: HeaderMap,
) -> Response {
    let Some(root) = state.config.shares.get(&share) else {
        return respond::not_found().await;
    };
    if relpath::validate(&path).is_err() {
        return respond::not_found().await;
    }
    match state.access() {
        Ok(access) if access.allows(&user.name, &share, &path) => {}
        Ok(_) => return respond::forbidden(),
        Err(e) => {
            tracing::warn!(error = %e, "cannot read access policy");
            return respond::forbidden();
        }
    }
    // `path` passed `relpath::validate` above, so the join cannot escape `root`.
    let full = root.join(&path);
    // Open the final component without following a symlink, so it cannot be
    // swapped between a check and the open (TOCTOU). Intermediate components
    // are covered by the canonical containment check below.
    let Ok(file) = files::open_nofollow(&full) else {
        return respond::not_found().await;
    };
    if !file.metadata().is_ok_and(|metadata| metadata.is_file()) {
        return respond::not_found().await;
    }
    let (Ok(real_root), Ok(real_file)) = (
        tokio::fs::canonicalize(root).await,
        tokio::fs::canonicalize(&full).await,
    ) else {
        return respond::not_found().await;
    };
    if !real_file.starts_with(&real_root) {
        return respond::not_found().await;
    }
    // Serve the already-opened descriptor through `/proc` so `ServeFile` cannot
    // re-open a different inode; the descriptor stays open for the transfer.
    let proc_path = PathBuf::from(format!("/proc/self/fd/{}", file.as_raw_fd()));
    let response = files::serve_file(proc_path, method, headers).await;
    // `ServeFile` opens the `/proc/self/fd/N` path while building the response,
    // so the descriptor may be closed only after it has done so.
    drop(file);
    response
}

/// Handler for `/_lanpull/client/manifest.json`.
pub(super) async fn serve_bundle_manifest(State(state): State<AppState>) -> Response {
    let directory = state.config.bundle_dir();
    match tokio::task::spawn_blocking(move || bundle::build(&directory)).await {
        Ok(Ok(manifest)) => respond::json_response(&manifest),
        Ok(Err(e)) => respond::error_response(&e),
        Err(e) => {
            tracing::error!(error = %e, "bundle worker failed");
            respond::text_response(StatusCode::INTERNAL_SERVER_ERROR, "internal error")
        }
    }
}

/// Handler for `/_lanpull/client/<file>`.
pub(super) async fn serve_bundle_file(
    State(state): State<AppState>,
    AxumPath(name): AxumPath<String>,
    method: Method,
    headers: HeaderMap,
) -> Response {
    if !bundle::is_allowed(&name) {
        return respond::not_found().await;
    }
    let full = state.config.bundle_dir().join(&name);
    match tokio::fs::symlink_metadata(&full).await {
        Ok(metadata) if metadata.is_file() && !metadata.file_type().is_symlink() => {
            files::serve_file(full, method, headers).await
        }
        _ => respond::not_found().await,
    }
}
