use axum::extract::{Path, State};
use axum::routing::{delete, get};
use axum::{Json, Router};

use crate::db::bounce_rules::BounceRule;
use crate::error::Result;
use crate::service::ServiceHandle;

pub fn router() -> Router<ServiceHandle> {
    Router::new()
        .route("/mailboxes/{id}/bounces", get(list).post(upsert))
        .route("/bounces/{id}", delete(delete_one))
}

async fn list(
    State(h): State<ServiceHandle>,
    Path(id): Path<String>,
) -> Result<Json<Vec<BounceRule>>> {
    Ok(Json(h.as_service().list_bounce_rules(&id).await?))
}

async fn upsert(
    State(h): State<ServiceHandle>,
    Path(mailbox_id): Path<String>,
    Json(mut rule): Json<BounceRule>,
) -> Result<Json<BounceRule>> {
    rule.mailbox_id = mailbox_id;
    Ok(Json(h.as_service().upsert_bounce_rule(rule).await?))
}

async fn delete_one(
    State(h): State<ServiceHandle>,
    Path(id): Path<String>,
) -> Result<Json<serde_json::Value>> {
    h.as_service().delete_bounce_rule(&id).await?;
    Ok(Json(serde_json::json!({"deleted": true})))
}
