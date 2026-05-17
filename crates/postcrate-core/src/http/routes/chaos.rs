use axum::extract::{Path, State};
use axum::routing::get;
use axum::{Json, Router};

use crate::db::chaos_configs::ChaosConfig;
use crate::error::Result;
use crate::service::ServiceHandle;

pub fn router() -> Router<ServiceHandle> {
    Router::new().route("/mailboxes/{id}/chaos", get(get_cfg).put(set_cfg))
}

async fn get_cfg(
    State(h): State<ServiceHandle>,
    Path(id): Path<String>,
) -> Result<Json<ChaosConfig>> {
    Ok(Json(h.as_service().get_chaos(&id).await?))
}

async fn set_cfg(
    State(h): State<ServiceHandle>,
    Path(id): Path<String>,
    Json(cfg): Json<ChaosConfig>,
) -> Result<Json<ChaosConfig>> {
    h.as_service().set_chaos(&id, cfg.clone()).await?;
    Ok(Json(cfg))
}
