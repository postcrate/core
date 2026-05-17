use axum::extract::{Path, Query, State};
use axum::routing::{get, post};
use axum::{Json, Router};

use crate::db::mailboxes::{
    CreateEphemeralInput, CreateMailboxInput, EphemeralHandle, Mailbox, UpdateMailboxInput,
};
use crate::error::Result;
use crate::http::dto::{DeletedCount, ListMailboxesQuery};
use crate::service::ServiceHandle;

pub fn router() -> Router<ServiceHandle> {
    Router::new()
        .route("/mailboxes", get(list).post(create))
        .route(
            "/mailboxes/{id}",
            get(get_one).patch(update).delete(delete_one),
        )
        .route("/mailboxes/ephemeral", post(create_ephemeral))
        .route("/mailboxes/{id}/messages", axum::routing::delete(clear))
}

async fn list(
    State(h): State<ServiceHandle>,
    Query(q): Query<ListMailboxesQuery>,
) -> Result<Json<Vec<Mailbox>>> {
    let v = h.as_service().list_mailboxes(q.project_id.as_deref()).await?;
    Ok(Json(v))
}

async fn get_one(
    State(h): State<ServiceHandle>,
    Path(id): Path<String>,
) -> Result<Json<Mailbox>> {
    Ok(Json(h.as_service().get_mailbox(&id).await?))
}

async fn create(
    State(h): State<ServiceHandle>,
    Json(input): Json<CreateMailboxInput>,
) -> Result<Json<Mailbox>> {
    Ok(Json(h.as_service().create_mailbox(input).await?))
}

async fn update(
    State(h): State<ServiceHandle>,
    Path(id): Path<String>,
    Json(patch): Json<UpdateMailboxInput>,
) -> Result<Json<Mailbox>> {
    Ok(Json(h.as_service().update_mailbox(&id, patch).await?))
}

async fn delete_one(
    State(h): State<ServiceHandle>,
    Path(id): Path<String>,
) -> Result<Json<serde_json::Value>> {
    h.as_service().delete_mailbox(&id).await?;
    Ok(Json(serde_json::json!({"deleted": true})))
}

async fn create_ephemeral(
    State(h): State<ServiceHandle>,
    Json(input): Json<CreateEphemeralInput>,
) -> Result<Json<EphemeralHandle>> {
    Ok(Json(h.as_service().create_ephemeral(input).await?))
}

async fn clear(
    State(h): State<ServiceHandle>,
    Path(id): Path<String>,
) -> Result<Json<DeletedCount>> {
    let n = h.as_service().clear_mailbox(&id).await?;
    Ok(Json(DeletedCount { deleted: n }))
}
