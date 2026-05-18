use axum::body::Body;
use axum::extract::{Path, Query, State};
use axum::http::{header, HeaderValue, StatusCode};
use axum::response::{IntoResponse, Response};
use axum::routing::{get, post};
use axum::{Json, Router};

use crate::db::emails::{EmailDetail, EmailSummary};
use crate::error::{Error, Result};
use crate::http::dto::{ListMessagesQuery, SearchBody};
use crate::service::ServiceHandle;

pub fn router() -> Router<ServiceHandle> {
    Router::new()
        .route("/messages", get(list))
        .route("/messages/{id}", get(get_one).delete(delete_one))
        .route("/messages/{id}/raw", get(get_raw))
        .route(
            "/messages/{id}/attachments/{aid}",
            get(get_attachment),
        )
        .route("/messages/search", post(search))
        .route("/messages/{id}/read", post(mark_read))
        .route("/messages/{id}/pin", post(set_pin))
        .route("/messages/{id}/star", post(set_star))
        .route("/messages/{id}/note", post(set_note))
        .route("/messages/{id}/tag", post(set_tag))
        .route("/messages/{id}/release", post(release))
}

async fn list(
    State(h): State<ServiceHandle>,
    Query(q): Query<ListMessagesQuery>,
) -> Result<Json<Vec<EmailSummary>>> {
    let mb = q
        .mailbox_id
        .ok_or_else(|| Error::Invalid("mailboxId query param required".into()))?;
    let limit = q.limit.unwrap_or(100).min(1000);
    let offset = q.offset.unwrap_or(0);
    Ok(Json(h.as_service().list_emails(&mb, limit, offset).await?))
}

async fn get_one(
    State(h): State<ServiceHandle>,
    Path(id): Path<String>,
) -> Result<Json<EmailDetail>> {
    Ok(Json(h.as_service().get_email(&id).await?))
}

async fn get_raw(
    State(h): State<ServiceHandle>,
    Path(id): Path<String>,
) -> Result<Response> {
    let bytes = h.as_service().get_email_raw(&id).await?;
    let mut resp = (StatusCode::OK, bytes).into_response();
    resp.headers_mut().insert(
        header::CONTENT_TYPE,
        HeaderValue::from_static("message/rfc822"),
    );
    Ok(resp)
}

async fn get_attachment(
    State(h): State<ServiceHandle>,
    Path((email_id, attachment_id)): Path<(String, String)>,
) -> Result<Response> {
    // We validate email_id existence by fetching detail (cheap) so an
    // attachment id from a different email returns 404 instead of leaking.
    let detail = h.as_service().get_email(&email_id).await?;
    if !detail.attachments.iter().any(|a| a.id == attachment_id) {
        return Err(Error::AttachmentNotFound(attachment_id));
    }
    let (bytes, name, ct) = h.as_service().get_attachment_blob(&attachment_id).await?;
    let mut resp = Response::builder().status(StatusCode::OK).body(Body::from(bytes))?;
    if let Some(ct) = ct {
        if let Ok(v) = HeaderValue::from_str(&ct) {
            resp.headers_mut().insert(header::CONTENT_TYPE, v);
        }
    } else {
        resp.headers_mut().insert(
            header::CONTENT_TYPE,
            HeaderValue::from_static("application/octet-stream"),
        );
    }
    if let Some(name) = name {
        let disposition = format!("attachment; filename=\"{}\"", sanitize(&name));
        if let Ok(v) = HeaderValue::from_str(&disposition) {
            resp.headers_mut().insert(header::CONTENT_DISPOSITION, v);
        }
    }
    Ok(resp)
}

async fn delete_one(
    State(h): State<ServiceHandle>,
    Path(id): Path<String>,
) -> Result<Json<serde_json::Value>> {
    h.as_service().delete_email(&id).await?;
    Ok(Json(serde_json::json!({"deleted": true})))
}

async fn search(
    State(h): State<ServiceHandle>,
    Json(body): Json<SearchBody>,
) -> Result<Json<Vec<EmailSummary>>> {
    let limit = body.limit.unwrap_or(50).min(500);
    Ok(Json(
        h.as_service()
            .search_emails(&body.q, body.mailbox_id.as_deref(), limit)
            .await?,
    ))
}

#[derive(serde::Deserialize)]
struct ReadBody {
    read: bool,
}

async fn mark_read(
    State(h): State<ServiceHandle>,
    Path(id): Path<String>,
    Json(body): Json<ReadBody>,
) -> Result<Json<serde_json::Value>> {
    h.as_service().mark_read(&id, body.read).await?;
    Ok(Json(serde_json::json!({"read": body.read})))
}

#[derive(serde::Deserialize)]
struct PinBody {
    pinned: bool,
}

async fn set_pin(
    State(h): State<ServiceHandle>,
    Path(id): Path<String>,
    Json(body): Json<PinBody>,
) -> Result<Json<serde_json::Value>> {
    h.as_service().set_pinned(&id, body.pinned).await?;
    Ok(Json(serde_json::json!({"pinned": body.pinned})))
}

#[derive(serde::Deserialize)]
struct StarBody {
    starred: bool,
}

async fn set_star(
    State(h): State<ServiceHandle>,
    Path(id): Path<String>,
    Json(body): Json<StarBody>,
) -> Result<Json<serde_json::Value>> {
    h.as_service().set_starred(&id, body.starred).await?;
    Ok(Json(serde_json::json!({"starred": body.starred})))
}

#[derive(serde::Deserialize)]
struct NoteBody {
    /// `null` clears the note.
    note: Option<String>,
}

async fn set_note(
    State(h): State<ServiceHandle>,
    Path(id): Path<String>,
    Json(body): Json<NoteBody>,
) -> Result<Json<serde_json::Value>> {
    h.as_service().set_note(&id, body.note.as_deref()).await?;
    Ok(Json(serde_json::json!({"note": body.note})))
}

#[derive(serde::Deserialize)]
struct TagBody {
    /// `null` clears the tag.
    tag: Option<String>,
}

async fn set_tag(
    State(h): State<ServiceHandle>,
    Path(id): Path<String>,
    Json(body): Json<TagBody>,
) -> Result<Json<serde_json::Value>> {
    h.as_service().set_tag(&id, body.tag.as_deref()).await?;
    Ok(Json(serde_json::json!({"tag": body.tag})))
}

#[derive(serde::Deserialize)]
#[serde(rename_all = "camelCase")]
struct ReleaseBody {
    to: String,
    relay: crate::RelayConfig,
}

async fn release(
    State(h): State<ServiceHandle>,
    Path(id): Path<String>,
    Json(body): Json<ReleaseBody>,
) -> Result<Json<serde_json::Value>> {
    h.as_service().release_email(&id, &body.to, &body.relay).await?;
    Ok(Json(serde_json::json!({"released": true, "to": body.to})))
}

impl From<axum::http::Error> for Error {
    fn from(e: axum::http::Error) -> Self {
        Error::Internal(e.to_string())
    }
}

fn sanitize(s: &str) -> String {
    s.chars()
        .map(|c| if c == '"' || c == '\\' || c.is_control() { '_' } else { c })
        .collect()
}
