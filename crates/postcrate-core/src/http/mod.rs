//! Axum HTTP API. Binds to loopback by default; honors
//! `settings.network.expose_on_lan` at startup. All routes consume a
//! `ServiceHandle` cloned into Axum's state.

pub mod auth;
pub mod dto;
pub mod error;
pub mod routes;

use std::net::SocketAddr;
use std::time::Duration;

use axum::middleware;
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

    let mut app = Router::new()
        .merge(routes::router())
        .with_state(handle.clone())
        .layer(TraceLayer::new_for_http())
        .layer(CorsLayer::permissive())
        .layer(RequestBodyLimitLayer::new(50 * 1024 * 1024))
        .layer(TimeoutLayer::with_status_code(
            axum::http::StatusCode::REQUEST_TIMEOUT,
            Duration::from_secs(30),
        ));

    // Optional bearer-token auth on every /api/v1/... route.
    if let Some(token) = settings.network.api_auth_token.clone().filter(|s| !s.is_empty()) {
        tracing::info!(
            target: "postcrate::http",
            "HTTP API bearer-token auth enabled"
        );
        app = app.layer(middleware::from_fn(move |req, next| {
            let token = token.clone();
            async move { auth::require_bearer(&token, req, next).await }
        }));
    }

    let listener = tokio::net::TcpListener::bind(bind)
        .await
        .map_err(|e| match e.kind() {
            std::io::ErrorKind::AddrInUse => Error::PortInUse(port),
            _ => Error::Io(e),
        })?;
    let local_addr = listener.local_addr()?;
    let shutdown = CancellationToken::new();
    let shutdown_child = shutdown.clone();

    // HTTPS path: requires the `tls` feature + a configured cert+key.
    // Without the feature the API always runs plaintext (the cert
    // config is ignored).
    #[cfg(feature = "tls")]
    let api_tls = settings.network.api_tls
        && cfg.tls.enabled
        && cfg.tls.cert_path.is_some()
        && cfg.tls.key_path.is_some();
    #[cfg(not(feature = "tls"))]
    let api_tls = false;

    #[cfg(feature = "tls")]
    let task = if api_tls {
        // axum-server is built without a default rustls crypto
        // provider; install ring here (tokio-rustls already pulled
        // it in transitively). Idempotent — subsequent calls no-op.
        let _ = tokio_rustls::rustls::crypto::ring::default_provider().install_default();
        let cert = cfg.tls.cert_path.as_ref().unwrap().clone();
        let key = cfg.tls.key_path.as_ref().unwrap().clone();
        drop(listener);
        let shutdown_child = shutdown_child.clone();
        tokio::spawn(async move {
            let tls_config = match axum_server::tls_rustls::RustlsConfig::from_pem_file(&cert, &key)
                .await
            {
                Ok(c) => c,
                Err(e) => {
                    tracing::error!(target: "postcrate::http", error = %e, "load TLS pem failed");
                    return;
                }
            };
            let handle = axum_server::Handle::new();
            let handle_for_shutdown = handle.clone();
            tokio::spawn(async move {
                shutdown_child.cancelled().await;
                handle_for_shutdown.graceful_shutdown(Some(Duration::from_secs(5)));
            });
            if let Err(e) = axum_server::bind_rustls(bind, tls_config)
                .handle(handle)
                .serve(app.into_make_service())
                .await
            {
                tracing::error!(target: "postcrate::http", error = %e, "https server exited");
            }
        })
    } else {
        tokio::spawn(async move {
            let serve = axum::serve(listener, app)
                .with_graceful_shutdown(async move {
                    shutdown_child.cancelled().await;
                });
            if let Err(e) = serve.await {
                tracing::error!(target: "postcrate::http", error = %e, "http server exited");
            }
        })
    };

    #[cfg(not(feature = "tls"))]
    let task = tokio::spawn(async move {
        let serve = axum::serve(listener, app)
            .with_graceful_shutdown(async move {
                shutdown_child.cancelled().await;
            });
        if let Err(e) = serve.await {
            tracing::error!(target: "postcrate::http", error = %e, "http server exited");
        }
    });

    tracing::info!(
        target: "postcrate::http",
        addr = %local_addr,
        tls = api_tls,
        "http api listening"
    );

    Ok(HttpServerHandle {
        addr: local_addr,
        shutdown,
        task,
    })
}
