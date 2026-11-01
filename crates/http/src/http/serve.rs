//! TLS transport and graceful shutdown.
//!
//! [`serve_tls`] is the reusable entry point: it takes the transport settings,
//! an already-built [`Router`], and a shutdown future, and owns nothing about
//! lanpull's domain.

use std::future::Future;
use std::net::SocketAddr;
use std::time::Duration;

use axum::Router;

use lanpull_core::config::ServerSettings;
use lanpull_core::error::{Error, Result};

/// Run the HTTPS server until `shutdown` resolves.
///
/// The caller owns the preconditions (accounts, certificate files) and the
/// router; this function owns TLS, the listener, and the graceful drain.
pub async fn serve_tls(
    settings: ServerSettings,
    router: Router,
    shutdown: impl Future<Output = ()> + Send + 'static,
) -> Result<()> {
    let tls = axum_server::tls_rustls::RustlsConfig::from_pem_file(
        &settings.cert_path,
        &settings.key_path,
    )
    .await
    .map_err(|e| Error::Server(format!("TLS setup failed: {e}")))?;

    let address = SocketAddr::new(settings.bind, settings.port);
    let handle = axum_server::Handle::new();
    let shutdown_handle = handle.clone();
    tokio::spawn(async move {
        shutdown.await;
        tracing::info!("shutting down");
        shutdown_handle.graceful_shutdown(Some(Duration::from_secs(10)));
    });

    tracing::info!("listening on https://{address}");
    axum_server::bind_rustls(address, tls)
        .handle(handle)
        .serve(router.into_make_service_with_connect_info::<SocketAddr>())
        .await
        .map_err(|e| Error::Server(format!("server stopped: {e}")))
}

/// Resolve on `SIGINT` or `SIGTERM`.
pub(super) async fn shutdown_signal() {
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
