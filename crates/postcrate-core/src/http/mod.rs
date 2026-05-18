//! Axum HTTP API. Binds to loopback by default; honors
//! `settings.network.expose_on_lan` at startup. All routes consume a
//! `ServiceHandle` cloned into Axum's state.

pub mod dto;
pub mod error;
pub mod routes;

use std::net::SocketAddr;
use std::time::Duration;

use axum::Router;
use tokio::task::JoinHandle;
use tokio_util::sync::CancellationToken;
use tower_http::cors::CorsLayer;
use tower_http::limit::RequestBodyLimitLayer;
use tower_http::timeout::TimeoutLayer;
use tower_http::trace::TraceLayer;

use crate::config::BindHost;
use crate::error::{Error, Result};
use crate::service::ServiceHandle;

#[derive(Debug)]
pub struct HttpServerHandle {
    pub addr: SocketAddr,
    pub shutdown: CancellationToken,
    pub task: JoinHandle<()>,
}

pub async fn start(handle: ServiceHandle) -> Result<HttpServerHandle> {
    let cfg = handle.config();
    // Pull network settings; fall back to config if not yet persisted.
    let net = handle
        .inner
        .pool
        .clone();
    let _ = net;
    let settings = crate::db::settings::load_all(&handle.inner.pool).await?;
    // Precedence: an explicit `cfg.http_port = 0` means "let the OS
    // pick", and overrides settings (this is what integration tests
    // use). Otherwise the persisted setting wins so the user can
    // change ports at runtime; cfg is the boot-time default.
    let port = if cfg.http_port == 0 {
        0
    } else if settings.network.http_api_port != 0 {
        settings.network.http_api_port
    } else {
        cfg.http_port
    };
    let bind_host = if settings.network.expose_on_lan {
        BindHost::AllInterfaces
    } else {
        BindHost::Loopback
    };
    if matches!(bind_host, BindHost::AllInterfaces) {
        tracing::warn!(
            target: "postcrate::http",
            "HTTP API bound to 0.0.0.0 — accessible from other devices on this network"
        );
    }
    let bind = SocketAddr::new(bind_host.as_ip(), port);

    let app = Router::new()
        .merge(routes::router())
        .with_state(handle.clone())
        .layer(TraceLayer::new_for_http())
        .layer(CorsLayer::permissive())
        .layer(RequestBodyLimitLayer::new(50 * 1024 * 1024))
        .layer(TimeoutLayer::with_status_code(
            axum::http::StatusCode::REQUEST_TIMEOUT,
            Duration::from_secs(30),
        ));

    let listener = tokio::net::TcpListener::bind(bind)
        .await
        .map_err(|e| match e.kind() {
            std::io::ErrorKind::AddrInUse => Error::PortInUse(port),
            _ => Error::Io(e),
        })?;
    let local_addr = listener.local_addr()?;
    let shutdown = CancellationToken::new();
    let shutdown_child = shutdown.clone();

    let task = tokio::spawn(async move {
        let serve = axum::serve(listener, app)
            .with_graceful_shutdown(async move {
                shutdown_child.cancelled().await;
            });
        if let Err(e) = serve.await {
            tracing::error!(target: "postcrate::http", error = %e, "http server exited");
        }
    });

    tracing::info!(target: "postcrate::http", addr = %local_addr, "http api listening");

    Ok(HttpServerHandle {
        addr: local_addr,
        shutdown,
        task,
    })
}
