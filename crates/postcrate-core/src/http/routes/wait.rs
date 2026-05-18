//! `POST /messages/wait` and `POST /messages/:id/assert`.
//!
//! These expose the shared matcher primitives over HTTP so the MCP
//! server, the matcher packages, and direct `curl` users all hit the
//! same code path as the in-process `Service::wait_for_email`.

use std::time::Duration;

use axum::extract::{Path, State};
use axum::routing::post;
use axum::{Json, Router};
use serde::Deserialize;

use crate::error::Result;
use crate::matcher::{EmailPredicate, MatchResult, WaitOutcome};
use crate::service::ServiceHandle;

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct WaitBody {
    #[serde(default)]
    predicate: EmailPredicate,
    /// 1–300 seconds; default 30 (matches FR-AI-04). Values are clamped.
    timeout_seconds: Option<u32>,
}

pub fn router() -> Router<ServiceHandle> {
    Router::new()
        .route("/messages/wait", post(wait))
        .route("/messages/{id}/assert", post(assert))
}

async fn wait(
    State(h): State<ServiceHandle>,
    Json(body): Json<WaitBody>,
) -> Result<Json<WaitOutcome>> {
    let secs = body.timeout_seconds.unwrap_or(30).clamp(1, 300);
    let outcome = h
        .as_service()
        .wait_for_email(body.predicate, Duration::from_secs(u64::from(secs)))
        .await?;
    Ok(Json(outcome))
}

async fn assert(
    State(h): State<ServiceHandle>,
    Path(id): Path<String>,
    Json(predicate): Json<EmailPredicate>,
) -> Result<Json<MatchResult>> {
    Ok(Json(
        h.as_service().assert_email_matches(&id, &predicate).await?,
    ))
}
