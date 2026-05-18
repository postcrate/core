//! Rendering preview + lint endpoints.

use axum::extract::{Path, Query, State};
use axum::routing::get;
use axum::{Json, Router};
use serde::Deserialize;

use crate::error::Result;
use crate::rendering::a11y::A11yReport;
use crate::rendering::lint::LintReport;
use crate::rendering::profile::{Profile, RenderedPreview};
use crate::service::ServiceHandle;

pub fn router() -> Router<ServiceHandle> {
    Router::new()
        .route("/messages/{id}/render", get(render))
        .route("/messages/{id}/lint", get(lint))
        .route("/messages/{id}/a11y", get(a11y))
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct RenderQuery {
    profile: Profile,
}

async fn render(
    State(h): State<ServiceHandle>,
    Path(id): Path<String>,
    Query(q): Query<RenderQuery>,
) -> Result<Json<RenderedPreview>> {
    Ok(Json(h.as_service().render_preview(&id, q.profile).await?))
}

async fn lint(
    State(h): State<ServiceHandle>,
    Path(id): Path<String>,
) -> Result<Json<LintReport>> {
    Ok(Json(h.as_service().lint_html(&id).await?))
}

async fn a11y(
    State(h): State<ServiceHandle>,
    Path(id): Path<String>,
) -> Result<Json<A11yReport>> {
    Ok(Json(h.as_service().audit_a11y(&id).await?))
}
