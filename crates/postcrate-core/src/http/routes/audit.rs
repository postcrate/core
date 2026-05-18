//! Audit log HTTP endpoints. Read-only listing + a bulk-prune DELETE.

use axum::extract::{Query, State};
use axum::routing::get;
use axum::{Json, Router};
use serde::Deserialize;

use crate::db::audit::AuditEntry;
use crate::error::Result;
use crate::http::dto::DeletedCount;
use crate::service::ServiceHandle;

pub fn router() -> Router<ServiceHandle> {
    Router::new().route("/audit", get(list).delete(clear))
}

#[derive(Debug, Deserialize, Default)]
#[serde(rename_all = "camelCase")]
struct ListQuery {
    limit: Option<u32>,
    offset: Option<u32>,
}

#[derive(Debug, Deserialize, Default)]
#[serde(rename_all = "camelCase")]
struct ClearQuery {
    /// If set, only prune entries older than N days. If absent, clear all.
    older_than_days: Option<u32>,
}

async fn list(
    State(h): State<ServiceHandle>,
    Query(q): Query<ListQuery>,
) -> Result<Json<Vec<AuditEntry>>> {
    let limit = q.limit.unwrap_or(100).min(1000);
    let offset = q.offset.unwrap_or(0);
    Ok(Json(h.as_service().list_audit(limit, offset).await?))
}

async fn clear(
    State(h): State<ServiceHandle>,
    Query(q): Query<ClearQuery>,
) -> Result<Json<DeletedCount>> {
    let deleted = h.as_service().clear_audit(q.older_than_days).await?;
    Ok(Json(DeletedCount { deleted }))
}
