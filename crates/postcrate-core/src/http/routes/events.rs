//! Server-Sent Events stream of [`CoreEvent`] — `GET /api/v1/events`.
//!
//! Provides a real-time event feed to consumers that aren't linked
//! against `postcrate-core` (the React frontend, the MCP server when
//! it runs out-of-process, the matcher packages). Each line on the
//! wire follows the SSE spec:
//!
//! ```text
//! event: newEmail
//! data: { "kind": "newEmail", "mailboxId": "...", "email": { ... } }
//!
//! event: mailboxStateChanged
//! data: { ... }
//! ```
//!
//! 15-second keep-alive comments prevent intermediaries (browser idle
//! reaper, proxies) from severing the connection.

use std::convert::Infallible;
use std::time::Duration;

use axum::extract::State;
use axum::response::sse::{Event, KeepAlive, Sse};
use axum::routing::get;
use axum::Router;
use futures_util::stream::{Stream, StreamExt};
use tokio_stream::wrappers::errors::BroadcastStreamRecvError;
use tokio_stream::wrappers::BroadcastStream;

use crate::events::CoreEvent;
use crate::service::ServiceHandle;

pub fn router() -> Router<ServiceHandle> {
    Router::new().route("/events", get(stream))
}

async fn stream(
    State(h): State<ServiceHandle>,
) -> Sse<impl Stream<Item = Result<Event, Infallible>>> {
    let rx = h.as_service().subscribe();
    let stream = BroadcastStream::new(rx).filter_map(|res| async move {
        match res {
            Ok(ev) => {
                let kind = match &ev {
                    CoreEvent::NewEmail { .. } => "newEmail",
                    CoreEvent::MailboxStateChanged { .. } => "mailboxStateChanged",
                    CoreEvent::ServerStatusChanged { .. } => "serverStatusChanged",
                    CoreEvent::SettingsChanged { .. } => "settingsChanged",
                    CoreEvent::AuditAppended { .. } => "auditAppended",
                };
                let payload = serde_json::to_string(&ev).ok()?;
                Some(Ok::<_, Infallible>(Event::default().event(kind).data(payload)))
            }
            Err(BroadcastStreamRecvError::Lagged(n)) => {
                // Tell the client they missed N events; they should
                // resync via a REST query.
                Some(Ok(Event::default().event("lagged").data(n.to_string())))
            }
        }
    });
    Sse::new(stream).keep_alive(
        KeepAlive::new()
            .interval(Duration::from_secs(15))
            .text("keep-alive"),
    )
}
