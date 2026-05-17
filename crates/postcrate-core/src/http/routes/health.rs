use axum::extract::State;
use axum::routing::get;
use axum::{Json, Router};

use crate::error::Result;
use crate::http::dto::InfoResponse;
use crate::service::ServiceHandle;

pub fn router() -> Router<ServiceHandle> {
    Router::new()
        .route("/healthz", get(healthz))
        .route("/info", get(info))
}

async fn healthz() -> &'static str {
    "ok"
}

async fn info(State(h): State<ServiceHandle>) -> Result<Json<InfoResponse>> {
    let svc = h.as_service();
    let status = svc.status();
    Ok(Json(InfoResponse {
        version: env!("CARGO_PKG_VERSION"),
        uptime_sec: 0,
        running_mailboxes: status.running_mailboxes,
        bind_host: h.config().bind_host.as_ip().to_string(),
        http_port: h.config().http_port,
    }))
}
