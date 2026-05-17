//! The single ingest worker. SMTP sessions push `CapturedEnvelope` onto
//! a bounded mpsc channel; this task drains it and writes to SQLite.
//!
//! Why one task: SQLite is fundamentally single-writer. Serializing
//! through one Tokio task gives trivial ordering, one place to apply
//! retention, and natural backpressure on bursts.

use std::path::PathBuf;
use std::sync::Arc;

use sqlx::SqlitePool;
use tokio::sync::mpsc;
use tokio::task::JoinHandle;
use tokio_util::sync::CancellationToken;
use uuid::Uuid;

use crate::db::attachments::AttachmentInsert;
use crate::db::emails::{EmailInsert, EmailSummary};
use crate::db::settings::InboxPrefs;
use crate::db::{emails as db_emails, settings as db_settings};
use crate::error::Result;
use crate::events::{CoreEvent, EventSink};
use crate::mail::parse::{self as parse_mail, ParsedAttachment};
use crate::pipeline::retention;
use crate::smtp::data_reader::{finalize_to_blob, load_bytes};
use crate::smtp::session::CapturedEnvelope;

pub fn spawn(
    pool: SqlitePool,
    sink: Arc<dyn EventSink>,
    mut rx: mpsc::Receiver<CapturedEnvelope>,
    raw_dir: PathBuf,
    att_dir: PathBuf,
    cancel: CancellationToken,
) -> JoinHandle<()> {
    tokio::spawn(async move {
        loop {
            tokio::select! {
                _ = cancel.cancelled() => return,
                env = rx.recv() => match env {
                    None => return,
                    Some(env) => {
                        if let Err(e) = ingest_one(&pool, &sink, env, &raw_dir, &att_dir).await {
                            tracing::error!(target: "postcrate::ingest", error = %e, "ingest failed");
                        }
                    }
                }
            }
        }
    })
}

async fn ingest_one(
    pool: &SqlitePool,
    sink: &Arc<dyn EventSink>,
    env: CapturedEnvelope,
    raw_dir: &std::path::Path,
    att_dir: &std::path::Path,
) -> Result<()> {
    // Materialize the raw bytes once. `mail-parser` needs a contiguous
    // slice; for OnDisk sources we already paid the disk write, so the
    // additional read is cheap relative to parsing.
    let raw_bytes = load_bytes(&env.raw).await?;
    let parsed = parse_mail::parse(&raw_bytes);

    // Pre-generate the email id so the blob filename is known.
    let email_id = Uuid::new_v4().to_string();
    let raw_path = finalize_to_blob(&env.raw, raw_dir, &email_id).await?;
    let raw_path_str = raw_path.to_string_lossy().to_string();

    // Write attachment blobs.
    let mut attachments = Vec::with_capacity(parsed.attachments.len());
    for att in &parsed.attachments {
        let id = Uuid::new_v4().to_string();
        let blob_path = write_attachment(att_dir, &id, att).await?;
        attachments.push(AttachmentInsert {
            id,
            filename: att.filename.clone(),
            content_type: att.content_type.clone(),
            content_id: att.content_id.clone(),
            size_bytes: att.data.len() as i64,
            blob_path,
        });
    }

    let parsed_json = parsed_to_json(&parsed);
    let fts_body = parse_mail::fts_body(&parsed);

    let insert = EmailInsert {
        mailbox_id: env.mailbox_id.clone(),
        received_at: env.received_at,
        smtp_from: env.mail_from,
        smtp_to: env.rcpt_to,
        header_from: parsed.header_from.clone(),
        header_to: parsed.header_to.clone(),
        header_cc: parsed.header_cc.clone(),
        header_subject: parsed.header_subject.clone(),
        message_id: parsed.message_id.clone(),
        in_reply_to: parsed.in_reply_to.clone(),
        size_bytes: raw_bytes.len() as i64,
        has_html: parsed.has_html,
        has_text: parsed.has_text,
        raw_path: raw_path_str.clone(),
        parsed_json,
        ext_smtputf8: env.ext_smtputf8,
        ext_8bitmime: env.ext_8bitmime,
        attachments,
        fts_body,
    };

    let outcome = db_emails::insert(pool, insert).await?;

    // Retention: enforce per-mailbox cap inline.
    let inbox_prefs = load_inbox_prefs(pool).await;
    if let Some(max) = (inbox_prefs.max_retained_emails > 0)
        .then_some(inbox_prefs.max_retained_emails)
    {
        retention::cap_per_mailbox(pool, &env.mailbox_id, i64::from(max), raw_dir).await?;
    }

    let summary = EmailSummary {
        id: outcome.id.clone(),
        ..outcome.summary
    };
    sink.emit(CoreEvent::NewEmail {
        mailbox_id: env.mailbox_id,
        email: summary,
    });

    Ok(())
}

async fn write_attachment(
    att_dir: &std::path::Path,
    id: &str,
    att: &ParsedAttachment,
) -> Result<String> {
    tokio::fs::create_dir_all(att_dir).await?;
    let path = att_dir.join(id);
    tokio::fs::write(&path, &att.data).await?;
    Ok(path.to_string_lossy().to_string())
}

async fn load_inbox_prefs(pool: &SqlitePool) -> InboxPrefs {
    db_settings::load_all(pool).await.map(|s| s.inbox).unwrap_or_default()
}

fn parsed_to_json(p: &parse_mail::Parsed) -> serde_json::Value {
    serde_json::json!({
        "headers":     p.headers_json,
        "text_body":   p.text_body,
        "html_body":   p.html_body,
        "has_text":    p.has_text,
        "has_html":    p.has_html,
        "subject":     p.header_subject,
        "from":        p.header_from,
        "to":          p.header_to,
        "cc":          p.header_cc,
        "message_id":  p.message_id,
        "in_reply_to": p.in_reply_to,
    })
}
