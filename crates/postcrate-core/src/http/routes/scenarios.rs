//! Scenario diagnostics endpoints.

use axum::extract::{Path, State};
use axum::routing::get;
use axum::{Json, Router};

use crate::error::Result;
use crate::scenarios::{
    auth::AuthReport, links::LinkReport, list_unsub::UnsubReport, spam::SpamReport,
};
use crate::service::ServiceHandle;

pub fn router() -> Router<ServiceHandle> {
    Router::new()
        .route("/messages/{id}/scenarios/spam", get(spam))
        .route("/messages/{id}/scenarios/links", get(links))
        .route("/messages/{id}/scenarios/auth", get(auth))
        .route("/messages/{id}/scenarios/list-unsub", get(list_unsub))
}

async fn spam(
    State(h): State<ServiceHandle>,
    Path(id): Path<String>,
) -> Result<Json<SpamReport>> {
    Ok(Json(h.as_service().analyze_spam(&id).await?))
}

async fn links(
    State(h): State<ServiceHandle>,
    Path(id): Path<String>,
) -> Result<Json<LinkReport>> {
    Ok(Json(h.as_service().analyze_links(&id).await?))
}

async fn auth(
    State(h): State<ServiceHandle>,
    Path(id): Path<String>,
) -> Result<Json<AuthReport>> {
    Ok(Json(h.as_service().analyze_auth(&id).await?))
}

async fn list_unsub(
    State(h): State<ServiceHandle>,
    Path(id): Path<String>,
) -> Result<Json<UnsubReport>> {
    Ok(Json(h.as_service().analyze_list_unsub(&id).await?))
}
