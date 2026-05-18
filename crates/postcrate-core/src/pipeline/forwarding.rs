//! Fire-and-forget auto-forwarding fan-out.
//!
//! On every captured email, the ingest worker calls `dispatch` with
//! the new email's id and raw_path. We look up matching forwarding
//! rules (global + mailbox-scoped) and `relay_message` to each
//! target address in a detached task. Failures are logged + audited
//! but never propagated.
//!
//! The raw bytes are read from disk inside the spawned task so we
//! don't keep the ingest worker waiting on I/O.

use sqlx::SqlitePool;

use crate::db::audit::{self, AuditAppend};
use crate::db::forwarding as db_fwd;
use crate::smtp::relay::relay_message;

pub async fn dispatch(pool: SqlitePool, mailbox_id: String, email_id: String, raw_path: String) {
    let rules = match db_fwd::list_for_mailbox(&pool, &mailbox_id).await {
        Ok(v) if !v.is_empty() => v,
        _ => return,
    };

    for rule in rules {
        let pool = pool.clone();
        let raw_path = raw_path.clone();
        let email_id = email_id.clone();
        tokio::spawn(async move {
            let raw = match tokio::fs::read(&raw_path).await {
                Ok(b) => b,
                Err(e) => {
                    tracing::warn!(
                        target: "postcrate::forward",
                        error = %e,
                        path = %raw_path,
                        "read raw for forwarding failed"
                    );
                    return;
                }
            };
            // Pull the captured envelope sender so the forwarded mail
            // looks like it's coming from the original sender — the
            // relay itself will rewrite headers if it wants to.
            let from = match crate::db::emails::get_detail(&pool, &email_id).await {
                Ok(d) if !d.from.is_empty() => d.from,
                _ => "postcrate@localhost".to_string(),
            };

            if let Err(e) = relay_message(&rule.relay, &from, &rule.target_addresses, &raw).await {
                tracing::warn!(
                    target: "postcrate::forward",
                    error = %e,
                    rule = %rule.id,
                    "forwarding failed"
                );
                let _ = audit::append(
                    &pool,
                    AuditAppend {
                        actor: "system".into(),
                        action: "forwarding.failed".into(),
                        target_kind: Some("forwarding_rule".into()),
                        target_id: Some(rule.id.clone()),
                        metadata: Some(serde_json::json!({
                            "emailId": email_id,
                            "error": e.to_string(),
                        })),
                    },
                )
                .await;
            }
        });
    }
}
