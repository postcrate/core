//! Mailtrap-compatible read API (FR-ADOPT-10).
//!
//! Goal: existing Mailtrap-integrated test code keeps working when
//! `MAILTRAP_API_URL` is swapped to Postcrate. We mirror the most
//! commonly-used GET endpoints from Mailtrap's documented API
//! (<https://api-docs.mailtrap.io/>), translating each one to a call
//! against our own `Service`. Writes are not aliased: this is a
//! one-way compatibility shim, not a Mailtrap clone.
//!
//! The Mailtrap URL shape is:
//!
//! ```text
//! GET /api/accounts/{account_id}/inboxes
//! GET /api/accounts/{account_id}/inboxes/{inbox_id}/messages
//! GET /api/accounts/{account_id}/inboxes/{inbox_id}/messages/{id}
//! GET /api/accounts/{account_id}/inboxes/{inbox_id}/messages/{id}/body.eml
//! GET /api/accounts/{account_id}/inboxes/{inbox_id}/messages/{id}/body.txt
//! GET /api/accounts/{account_id}/inboxes/{inbox_id}/messages/{id}/body.html
//! ```
//!
//! Postcrate doesn't have the "account" concept — local-first, no
//! multi-tenant. We accept (and ignore) any `{account_id}` so client
//! libraries hard-coded with `accounts/{id}` keep functioning.
//! `{inbox_id}` maps 1:1 to our mailbox id.

use axum::body::Body;
use axum::extract::{Path, State};
use axum::http::{header, HeaderValue, StatusCode};
use axum::response::{IntoResponse, Response};
use axum::routing::get;
use axum::{Json, Router};
use serde::Serialize;

use crate::error::{Error, Result};
use crate::service::ServiceHandle;

pub fn router() -> Router<ServiceHandle> {
    // Mailtrap's prefix is `/api/...`, *not* `/api/v1/...`, so we
    // mount at the top of the router and the rest of Postcrate's
    // surface is unaffected.
    Router::new()
        .route("/api/accounts/{account}/inboxes", get(list_inboxes))
        .route(
            "/api/accounts/{account}/inboxes/{inbox}/messages",
            get(list_messages),
        )
        .route(
            "/api/accounts/{account}/inboxes/{inbox}/messages/{id}",
            get(get_message),
        )
        .route(
            "/api/accounts/{account}/inboxes/{inbox}/messages/{id}/body.eml",
            get(get_raw),
        )
        .route(
            "/api/accounts/{account}/inboxes/{inbox}/messages/{id}/body.txt",
            get(get_text),
        )
        .route(
            "/api/accounts/{account}/inboxes/{inbox}/messages/{id}/body.html",
            get(get_html),
        )
}

#[derive(Serialize)]
struct MailtrapInbox {
    id: String,
    name: String,
    domain: String,
    pop3_domain: &'static str,
    email_domain: String,
    smtp_ports: Vec<u16>,
    pop3_ports: Vec<u16>,
    emails_count: i64,
    emails_unread_count: i64,
    last_message_sent_at: Option<i64>,
    max_size: u64,
    status: &'static str,
    email_username: String,
}

async fn list_inboxes(
    State(h): State<ServiceHandle>,
    Path(_account): Path<String>,
) -> Result<Json<Vec<MailtrapInbox>>> {
    let svc = h.as_service();
    let mailboxes = svc.list_mailboxes(None).await?;
    let cfg = svc.config();
    let mut out = Vec::with_capacity(mailboxes.len());
    for mb in mailboxes {
        let unread = svc
            .list_emails(&mb.id, 1000, 0)
            .await
            .map(|s| s.iter().filter(|x| !x.read).count() as i64)
            .unwrap_or(0);
        let last = svc
            .list_emails(&mb.id, 1, 0)
            .await
            .ok()
            .and_then(|s| s.first().map(|x| x.received_at));
        out.push(MailtrapInbox {
            id: mb.id.clone(),
            name: mb.name.clone(),
            domain: cfg.ehlo_hostname.clone(),
            pop3_domain: "unsupported",
            email_domain: cfg.ehlo_hostname.clone(),
            smtp_ports: vec![mb.port],
            pop3_ports: vec![],
            emails_count: mb.count,
            emails_unread_count: unread,
            last_message_sent_at: last,
            max_size: cfg.max_message_bytes,
            status: "active",
            email_username: mb.name,
        });
    }
    Ok(Json(out))
}

#[derive(Serialize)]
struct MailtrapMessage {
    id: String,
    inbox_id: String,
    subject: Option<String>,
    sent_at: i64,
    from_email: String,
    to_email: Option<String>,
    is_read: bool,
    download_url: String,
    txt_url: String,
    html_url: String,
}

async fn list_messages(
    State(h): State<ServiceHandle>,
    Path((account, inbox)): Path<(String, String)>,
) -> Result<Json<Vec<MailtrapMessage>>> {
    let summaries = h.as_service().list_emails(&inbox, 1000, 0).await?;
    let out = summaries
        .into_iter()
        .map(|s| MailtrapMessage {
            download_url: format!(
                "/api/accounts/{account}/inboxes/{inbox}/messages/{}/body.eml",
                s.id
            ),
            txt_url: format!(
                "/api/accounts/{account}/inboxes/{inbox}/messages/{}/body.txt",
                s.id
            ),
            html_url: format!(
                "/api/accounts/{account}/inboxes/{inbox}/messages/{}/body.html",
                s.id
            ),
            id: s.id,
            inbox_id: s.mailbox_id,
            subject: s.subject,
            sent_at: s.received_at,
            from_email: s.from,
            to_email: s.to.first().cloned(),
            is_read: s.read,
        })
        .collect();
    Ok(Json(out))
}

async fn get_message(
    State(h): State<ServiceHandle>,
    Path((account, inbox, id)): Path<(String, String, String)>,
) -> Result<Json<serde_json::Value>> {
    let d = h.as_service().get_email(&id).await?;
    if d.mailbox_id != inbox {
        return Err(Error::EmailNotFound(id));
    }
    Ok(Json(serde_json::json!({
        "id": d.id,
        "inbox_id": d.mailbox_id,
        "subject": d.subject,
        "from_email": d.from,
        "to_email": d.to.first(),
        "sent_at": d.received_at,
        "is_read": d.read,
        "text_body": d.text_body,
        "html_body": d.html_body,
        "headers": d.headers,
        "attachments": d.attachments,
        "download_url": format!("/api/accounts/{account}/inboxes/{inbox}/messages/{}/body.eml", d.id),
    })))
}

async fn get_raw(
    State(h): State<ServiceHandle>,
    Path((_account, _inbox, id)): Path<(String, String, String)>,
) -> Result<Response> {
    let bytes = h.as_service().get_email_raw(&id).await?;
    let mut resp = (StatusCode::OK, bytes).into_response();
    resp.headers_mut().insert(
        header::CONTENT_TYPE,
        HeaderValue::from_static("message/rfc822"),
    );
    Ok(resp)
}

async fn get_text(
    State(h): State<ServiceHandle>,
    Path((_account, _inbox, id)): Path<(String, String, String)>,
) -> Result<Response> {
    let d = h.as_service().get_email(&id).await?;
    let body = d.text_body.unwrap_or_default();
    let mut resp = Response::builder()
        .status(StatusCode::OK)
        .body(Body::from(body))?;
    resp.headers_mut().insert(
        header::CONTENT_TYPE,
        HeaderValue::from_static("text/plain; charset=utf-8"),
    );
    Ok(resp)
}

async fn get_html(
    State(h): State<ServiceHandle>,
    Path((_account, _inbox, id)): Path<(String, String, String)>,
) -> Result<Response> {
    let d = h.as_service().get_email(&id).await?;
    let body = d.html_body.unwrap_or_default();
    let mut resp = Response::builder()
        .status(StatusCode::OK)
        .body(Body::from(body))?;
    resp.headers_mut().insert(
        header::CONTENT_TYPE,
        HeaderValue::from_static("text/html; charset=utf-8"),
    );
    Ok(resp)
}
