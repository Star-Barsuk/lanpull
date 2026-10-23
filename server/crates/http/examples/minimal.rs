//! A minimal HTTPS server on the reusable transport.
//!
//! `serve_tls` needs only the transport settings, a router, and a shutdown
//! future; it knows nothing about lanpull state, shares, or policy. Run it
//! with a certificate and key from `lanpull cert`:
//!
//! ```console
//! LANPULL_CERT=server.crt LANPULL_KEY=server.key cargo run --example minimal
//! curl --cacert server.crt https://127.0.0.1:8443/health
//! ```
#![allow(clippy::print_stdout)]

use std::net::IpAddr;
use std::path::PathBuf;

use axum::routing::get;
use axum::Router;
use lanpull_core::config::ServerSettings;
use lanpull_http::serve_tls;

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let cert_path = PathBuf::from(std::env::var("LANPULL_CERT")?);
    let key_path = PathBuf::from(std::env::var("LANPULL_KEY")?);
    let bind: IpAddr = std::env::var("LANPULL_BIND")
        .unwrap_or_else(|_| "127.0.0.1".to_string())
        .parse()?;
    let port: u16 = std::env::var("LANPULL_PORT")
        .unwrap_or_else(|_| "8443".to_string())
        .parse()?;

    let router = Router::new().route("/health", get(|| async { "ok\n" }));
    println!("listening on https://{bind}:{port}");
    serve_tls(
        ServerSettings {
            bind,
            port,
            cert_path,
            key_path,
        },
        router,
        async {
            let _ = tokio::signal::ctrl_c().await;
        },
    )
    .await?;
    Ok(())
}
