//! Webhook + forwarding-rule HTTP CRUD.

use axum::extract::{Path, State};
use axum::routing::{delete, get};
use axum::{Json, Router};

use crate::db::forwarding::{CreateForwardingRule, ForwardingRule};
use crate::db::webhooks::{CreateWebhook, Webhook};
use crate::error::Result;
use crate::service::ServiceHandle;

pub fn router() -> Router<ServiceHandle> {
    Router::new()
        .route("/webhooks", get(list_webhooks).post(create_webhook))
        .route("/webhooks/{id}", delete(delete_webhook))
        .route("/forwarding", get(list_fwd).post(create_fwd))
        .route("/forwarding/{id}", delete(delete_fwd))
}

async fn list_webhooks(State(h): State<ServiceHandle>) -> Result<Json<Vec<Webhook>>> {
    Ok(Json(h.as_service().list_webhooks().await?))
}

async fn create_webhook(
    State(h): State<ServiceHandle>,
    Json(body): Json<CreateWebhook>,
) -> Result<Json<Webhook>> {
    Ok(Json(h.as_service().create_webhook(body).await?))
}

async fn delete_webhook(
    State(h): State<ServiceHandle>,
    Path(id): Path<String>,
) -> Result<Json<serde_json::Value>> {
    h.as_service().delete_webhook(&id).await?;
    Ok(Json(serde_json::json!({"deleted": true})))
}

async fn list_fwd(State(h): State<ServiceHandle>) -> Result<Json<Vec<ForwardingRule>>> {
    Ok(Json(h.as_service().list_forwarding_rules().await?))
}

async fn create_fwd(
    State(h): State<ServiceHandle>,
    Json(body): Json<CreateForwardingRule>,
) -> Result<Json<ForwardingRule>> {
    Ok(Json(h.as_service().create_forwarding_rule(body).await?))
}

async fn delete_fwd(
    State(h): State<ServiceHandle>,
    Path(id): Path<String>,
) -> Result<Json<serde_json::Value>> {
    h.as_service().delete_forwarding_rule(&id).await?;
    Ok(Json(serde_json::json!({"deleted": true})))
}
