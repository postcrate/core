//! Outbound webhook fan-out.
//!
//! On each new captured email the ingest worker calls
//! `dispatch_webhooks` with the new `EmailSummary`. We look up every
//! enabled webhook (global + mailbox-scoped), then POST a JSON body
//! to each URL in a detached task. Failures are logged and audited
//! but never propagated — ingest must not fail because a downstream
//! receiver is down.

use std::sync::Arc;
use std::time::Duration;

use serde::Serialize;
use sqlx::SqlitePool;

use crate::db::audit::{self, AuditAppend};
use crate::db::emails::EmailSummary;
use crate::db::webhooks;

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct WebhookPayload<'a> {
    event: &'static str,
    mailbox_id: &'a str,
    email: &'a EmailSummary,
}

pub async fn dispatch(pool: SqlitePool, mailbox_id: String, email: EmailSummary) {
    let hooks = match webhooks::list_for_mailbox(&pool, &mailbox_id).await {
        Ok(v) => v,
        Err(e) => {
            tracing::warn!(target: "postcrate::webhook", error = %e, "list webhooks failed");
            return;
        }
    };
    if hooks.is_empty() {
        return;
    }
    let client = Arc::new(http_client());
    let payload = serde_json::to_value(WebhookPayload {
        event: "new_email",
        mailbox_id: &mailbox_id,
        email: &email,
    })
    .unwrap_or(serde_json::Value::Null);

    for hook in hooks {
        let client = client.clone();
        let payload = payload.clone();
        let pool = pool.clone();
        tokio::spawn(async move {
            let mut req = client.post(&hook.url).json(&payload);
            if let Some(auth) = &hook.auth_header {
                req = req.header(reqwest::header::AUTHORIZATION, auth);
            }
            match req.send().await {
                Ok(resp) if resp.status().is_success() => {}
                Ok(resp) => {
                    let status = resp.status();
                    tracing::warn!(
                        target: "postcrate::webhook",
                        url = %hook.url,
                        status = %status,
                        "webhook returned non-2xx"
                    );
                    let _ = audit::append(
                        &pool,
                        AuditAppend {
                            actor: "system".into(),
                            action: "webhook.failed".into(),
                            target_kind: Some("webhook".into()),
                            target_id: Some(hook.id.clone()),
                            metadata: Some(serde_json::json!({
                                "url": hook.url,
                                "status": status.as_u16(),
                            })),
                        },
                    )
                    .await;
                }
                Err(e) => {
                    tracing::warn!(
                        target: "postcrate::webhook",
                        url = %hook.url,
                        error = %e,
                        "webhook delivery failed"
                    );
                    let _ = audit::append(
                        &pool,
                        AuditAppend {
                            actor: "system".into(),
                            action: "webhook.failed".into(),
                            target_kind: Some("webhook".into()),
                            target_id: Some(hook.id.clone()),
                            metadata: Some(serde_json::json!({
                                "url": hook.url,
                                "error": e.to_string(),
                            })),
                        },
                    )
                    .await;
                }
            }
        });
    }
}

fn http_client() -> reqwest::Client {
    reqwest::Client::builder()
        .timeout(Duration::from_secs(5))
        .connect_timeout(Duration::from_secs(3))
        .user_agent("postcrate-webhook/1")
        .build()
        .unwrap_or_else(|_| reqwest::Client::new())
}
