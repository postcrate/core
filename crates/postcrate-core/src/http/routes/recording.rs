//! Export / import / replay endpoints for `.postcrate` recordings.

use axum::extract::{Path, State};
use axum::routing::post;
use axum::{Json, Router};
use serde::Deserialize;

use crate::error::Result;
use crate::recording::Recording;
use crate::service::ServiceHandle;

pub fn router() -> Router<ServiceHandle> {
    Router::new()
        .route("/mailboxes/{id}/export", post(export))
        .route("/mailboxes/{id}/import", post(import))
        .route("/messages/{id}/replay", post(replay_one))
}

#[derive(Debug, Deserialize, Default)]
#[serde(rename_all = "camelCase")]
struct ExportBody {
    #[serde(default)]
    label: Option<String>,
}

async fn export(
    State(h): State<ServiceHandle>,
    Path(id): Path<String>,
    Json(body): Json<ExportBody>,
) -> Result<Json<Recording>> {
    Ok(Json(h.as_service().export_recording(&id, body.label).await?))
}

async fn import(
    State(h): State<ServiceHandle>,
    Path(id): Path<String>,
    Json(recording): Json<Recording>,
) -> Result<Json<serde_json::Value>> {
    let n = h.as_service().replay_recording(&id, &recording).await?;
    Ok(Json(serde_json::json!({"imported": n})))
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct ReplayBody {
    mailbox_id: String,
}

async fn replay_one(
    State(h): State<ServiceHandle>,
    Path(id): Path<String>,
    Json(body): Json<ReplayBody>,
) -> Result<Json<serde_json::Value>> {
    h.as_service().replay_email(&id, &body.mailbox_id).await?;
    Ok(Json(serde_json::json!({"replayed": true})))
}
