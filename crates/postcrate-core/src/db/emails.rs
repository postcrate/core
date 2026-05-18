//! Email row storage + FTS5 sync.

use serde::Serialize;
use sqlx::{Row, SqlitePool};
use uuid::Uuid;

use crate::db::attachments::AttachmentInsert;
use crate::error::{Error, Result};

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct EmailSummary {
    pub id: String,
    pub mailbox_id: String,
    pub received_at: i64,
    pub from: String,
    pub to: Vec<String>,
    pub subject: Option<String>,
    pub has_html: bool,
    pub has_text: bool,
    pub size_bytes: i64,
    pub read: bool,
    pub pinned: bool,
    pub starred: bool,
    pub tag: Option<String>,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct AttachmentMeta {
    pub id: String,
    pub filename: Option<String>,
    pub content_type: Option<String>,
    pub content_id: Option<String>,
    pub size_bytes: i64,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct EmailDetail {
    pub id: String,
    pub mailbox_id: String,
    pub received_at: i64,
    pub from: String,
    pub to: Vec<String>,
    pub subject: Option<String>,
    pub has_html: bool,
    pub has_text: bool,
    pub size_bytes: i64,
    pub read: bool,
    pub pinned: bool,
    pub starred: bool,
    pub note: Option<String>,
    pub tag: Option<String>,
    pub headers: serde_json::Value,
    pub text_body: Option<String>,
    pub html_body: Option<String>,
    pub attachments: Vec<AttachmentMeta>,
    pub message_id: Option<String>,
    pub in_reply_to: Option<String>,
    pub ext_smtputf8: bool,
    pub ext_8bitmime: bool,
}

#[derive(Debug, Clone)]
pub(crate) struct EmailInsert {
    pub mailbox_id: String,
    pub received_at: i64,
    pub smtp_from: String,
    pub smtp_to: Vec<String>,
    pub header_from: Option<String>,
    pub header_to: Option<String>,
    pub header_cc: Option<String>,
    pub header_subject: Option<String>,
    pub message_id: Option<String>,
    pub in_reply_to: Option<String>,
    pub size_bytes: i64,
    pub has_html: bool,
    pub has_text: bool,
    pub raw_path: String,
    pub parsed_json: serde_json::Value,
    pub ext_smtputf8: bool,
    pub ext_8bitmime: bool,
    pub attachments: Vec<AttachmentInsert>,
    /// For FTS: searchable body — text part if present, else html stripped.
    pub fts_body: String,
    /// Auto-detected category. `None` skips classification.
    pub tag: Option<String>,
}

#[derive(Debug, Clone)]
pub(crate) struct InsertOutcome {
    pub id: String,
    pub summary: EmailSummary,
}

/// Insert an email + its attachments + FTS row in one transaction.
pub(crate) async fn insert(pool: &SqlitePool, email: EmailInsert) -> Result<InsertOutcome> {
    let id = Uuid::new_v4().to_string();
    let smtp_to_json = serde_json::to_string(&email.smtp_to)?;
    let parsed_json_str = serde_json::to_string(&email.parsed_json)?;
    let fts_recipients = email.smtp_to.join(" ");
    let fts_subject = email.header_subject.clone().unwrap_or_default();
    let fts_sender = email.smtp_from.clone();

    let mut tx = pool.begin().await?;

    sqlx::query(
        r"INSERT INTO emails (
            id, mailbox_id, received_at, smtp_from, smtp_to_json,
            header_from, header_to, header_cc, header_subject,
            message_id, in_reply_to,
            size_bytes, has_html, has_text, raw_path, parsed_json,
            read_flag, ext_smtputf8, ext_8bitmime, tag
        ) VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, 0, ?, ?, ?)",
    )
    .bind(&id)
    .bind(&email.mailbox_id)
    .bind(email.received_at)
    .bind(&email.smtp_from)
    .bind(&smtp_to_json)
    .bind(&email.header_from)
    .bind(&email.header_to)
    .bind(&email.header_cc)
    .bind(&email.header_subject)
    .bind(&email.message_id)
    .bind(&email.in_reply_to)
    .bind(email.size_bytes)
    .bind(i64::from(email.has_html))
    .bind(i64::from(email.has_text))
    .bind(&email.raw_path)
    .bind(&parsed_json_str)
    .bind(i64::from(email.ext_smtputf8))
    .bind(i64::from(email.ext_8bitmime))
    .bind(&email.tag)
    .execute(&mut *tx)
    .await?;

    for att in &email.attachments {
        sqlx::query(
            r"INSERT INTO attachments
                (id, email_id, filename, content_type, content_id, size_bytes, blob_path)
              VALUES (?, ?, ?, ?, ?, ?, ?)",
        )
        .bind(&att.id)
        .bind(&id)
        .bind(&att.filename)
        .bind(&att.content_type)
        .bind(&att.content_id)
        .bind(att.size_bytes)
        .bind(&att.blob_path)
        .execute(&mut *tx)
        .await?;
    }

    // FTS sync — emails_fts stores `email_id` as an UNINDEXED column
    // (see migration 0004) so DELETE + SEARCH can both find rows by
    // the email's UUID without a Rust-side rowid hash.
    sqlx::query(
        r"INSERT INTO emails_fts(subject, sender, recipients, body, email_id)
          VALUES (?, ?, ?, ?, ?)",
    )
    .bind(&fts_subject)
    .bind(&fts_sender)
    .bind(&fts_recipients)
    .bind(&email.fts_body)
    .bind(&id)
    .execute(&mut *tx)
    .await?;

    tx.commit().await?;

    let summary = EmailSummary {
        id: id.clone(),
        mailbox_id: email.mailbox_id.clone(),
        received_at: email.received_at,
        from: email.smtp_from.clone(),
        to: email.smtp_to.clone(),
        subject: email.header_subject.clone(),
        has_html: email.has_html,
        has_text: email.has_text,
        size_bytes: email.size_bytes,
        read: false,
        pinned: false,
        starred: false,
        tag: email.tag.clone(),
    };

    Ok(InsertOutcome { id, summary })
}

/// Most-recent `limit` summaries across *all* mailboxes, newest first.
/// Used by `Service::wait_for_email` when the caller didn't specify a
/// mailbox filter.
pub(crate) async fn list_recent_across(
    pool: &SqlitePool,
    limit: u32,
) -> Result<Vec<EmailSummary>> {
    let rows = sqlx::query(
        r"SELECT id, mailbox_id, received_at, smtp_from, smtp_to_json,
                 header_subject, has_html, has_text, size_bytes, read_flag,
                 pinned, starred, tag
          FROM emails
          ORDER BY received_at DESC
          LIMIT ?",
    )
    .bind(i64::from(limit))
    .fetch_all(pool)
    .await?;
    let mut out = Vec::with_capacity(rows.len());
    for row in rows {
        out.push(row_to_summary(&row)?);
    }
    Ok(out)
}

pub(crate) async fn list(
    pool: &SqlitePool,
    mailbox_id: &str,
    limit: u32,
    offset: u32,
) -> Result<Vec<EmailSummary>> {
    // Pinned emails sort first regardless of received_at.
    let rows = sqlx::query(
        r"SELECT id, mailbox_id, received_at, smtp_from, smtp_to_json,
                 header_subject, has_html, has_text, size_bytes, read_flag,
                 pinned, starred, tag
          FROM emails
          WHERE mailbox_id = ?
          ORDER BY pinned DESC, received_at DESC
          LIMIT ? OFFSET ?",
    )
    .bind(mailbox_id)
    .bind(i64::from(limit))
    .bind(i64::from(offset))
    .fetch_all(pool)
    .await?;

    let mut out = Vec::with_capacity(rows.len());
    for row in rows {
        out.push(row_to_summary(&row)?);
    }
    Ok(out)
}

pub(crate) async fn get_detail(pool: &SqlitePool, id: &str) -> Result<EmailDetail> {
    let row = sqlx::query(
        r"SELECT id, mailbox_id, received_at, smtp_from, smtp_to_json,
                 header_subject, message_id, in_reply_to,
                 has_html, has_text, size_bytes, parsed_json, read_flag,
                 ext_smtputf8, ext_8bitmime, pinned, starred, note, tag
          FROM emails WHERE id = ?",
    )
    .bind(id)
    .fetch_optional(pool)
    .await?
    .ok_or_else(|| Error::EmailNotFound(id.to_string()))?;

    let parsed_json_str: String = row.try_get("parsed_json").unwrap_or_default();
    let parsed: serde_json::Value =
        serde_json::from_str(&parsed_json_str).unwrap_or(serde_json::Value::Null);

    let attachments = crate::db::attachments::list_for_email(pool, id).await?;

    let smtp_to_json: String = row.try_get("smtp_to_json").unwrap_or_default();
    let to: Vec<String> = serde_json::from_str(&smtp_to_json).unwrap_or_default();

    let headers = parsed.get("headers").cloned().unwrap_or(serde_json::Value::Null);
    let text_body = parsed
        .get("text_body")
        .and_then(|v| v.as_str())
        .map(|s| s.to_string());
    let html_body = parsed
        .get("html_body")
        .and_then(|v| v.as_str())
        .map(|s| s.to_string());

    Ok(EmailDetail {
        id: row.try_get("id").unwrap_or_default(),
        mailbox_id: row.try_get("mailbox_id").unwrap_or_default(),
        received_at: row.try_get("received_at").unwrap_or(0),
        from: row.try_get("smtp_from").unwrap_or_default(),
        to,
        subject: row.try_get("header_subject").ok(),
        has_html: row.try_get::<i64, _>("has_html").unwrap_or(0) != 0,
        has_text: row.try_get::<i64, _>("has_text").unwrap_or(0) != 0,
        size_bytes: row.try_get("size_bytes").unwrap_or(0),
        read: row.try_get::<i64, _>("read_flag").unwrap_or(0) != 0,
        headers,
        text_body,
        html_body,
        attachments,
        message_id: row.try_get("message_id").ok(),
        in_reply_to: row.try_get("in_reply_to").ok(),
        ext_smtputf8: row.try_get::<i64, _>("ext_smtputf8").unwrap_or(0) != 0,
        ext_8bitmime: row.try_get::<i64, _>("ext_8bitmime").unwrap_or(0) != 0,
        pinned: row.try_get::<i64, _>("pinned").unwrap_or(0) != 0,
        starred: row.try_get::<i64, _>("starred").unwrap_or(0) != 0,
        note: row
            .try_get::<Option<String>, _>("note")
            .ok()
            .flatten(),
        tag: row
            .try_get::<Option<String>, _>("tag")
            .ok()
            .flatten(),
    })
}

pub(crate) async fn get_raw_path(pool: &SqlitePool, id: &str) -> Result<String> {
    let row = sqlx::query("SELECT raw_path FROM emails WHERE id = ?")
        .bind(id)
        .fetch_optional(pool)
        .await?
        .ok_or_else(|| Error::EmailNotFound(id.to_string()))?;
    Ok(row.try_get::<String, _>("raw_path").unwrap_or_default())
}

pub(crate) async fn delete(pool: &SqlitePool, id: &str) -> Result<String> {
    let mut tx = pool.begin().await?;
    let raw_path: Option<String> = sqlx::query("SELECT raw_path FROM emails WHERE id = ?")
        .bind(id)
        .fetch_optional(&mut *tx)
        .await?
        .and_then(|r| r.try_get("raw_path").ok());
    let raw_path = raw_path.ok_or_else(|| Error::EmailNotFound(id.to_string()))?;

    sqlx::query("DELETE FROM emails WHERE id = ?")
        .bind(id)
        .execute(&mut *tx)
        .await?;
    sqlx::query("DELETE FROM emails_fts WHERE email_id = ?")
        .bind(id)
        .execute(&mut *tx)
        .await?;
    tx.commit().await?;
    Ok(raw_path)
}

/// Clear a mailbox. Returns (deleted_count, raw_paths_to_delete).
///
/// When `preserve_pinned` is true (the default in the Service layer),
/// rows with `pinned = 1` survive — mandates that pin acts
/// as a "keep this across inbox clears" marker.
pub(crate) async fn clear_mailbox(
    pool: &SqlitePool,
    mailbox_id: &str,
    preserve_pinned: bool,
) -> Result<(u64, Vec<String>)> {
    let mut tx = pool.begin().await?;
    let select_sql = if preserve_pinned {
        "SELECT id, raw_path FROM emails WHERE mailbox_id = ? AND pinned = 0"
    } else {
        "SELECT id, raw_path FROM emails WHERE mailbox_id = ?"
    };
    let rows = sqlx::query(select_sql)
        .bind(mailbox_id)
        .fetch_all(&mut *tx)
        .await?;
    let mut paths = Vec::with_capacity(rows.len());
    for r in &rows {
        let id: String = r.try_get("id").unwrap_or_default();
        let path: String = r.try_get("raw_path").unwrap_or_default();
        sqlx::query("DELETE FROM emails_fts WHERE email_id = ?")
            .bind(&id)
            .execute(&mut *tx)
            .await?;
        paths.push(path);
    }
    let delete_sql = if preserve_pinned {
        "DELETE FROM emails WHERE mailbox_id = ? AND pinned = 0"
    } else {
        "DELETE FROM emails WHERE mailbox_id = ?"
    };
    let res = sqlx::query(delete_sql)
        .bind(mailbox_id)
        .execute(&mut *tx)
        .await?;
    tx.commit().await?;
    Ok((res.rows_affected(), paths))
}

pub(crate) async fn mark_read(pool: &SqlitePool, id: &str, read: bool) -> Result<()> {
    let res = sqlx::query("UPDATE emails SET read_flag = ? WHERE id = ?")
        .bind(i64::from(read))
        .bind(id)
        .execute(pool)
        .await?;
    if res.rows_affected() == 0 {
        return Err(Error::EmailNotFound(id.to_string()));
    }
    Ok(())
}

pub(crate) async fn search(
    pool: &SqlitePool,
    q: &str,
    mailbox_id: Option<&str>,
    limit: u32,
) -> Result<Vec<EmailSummary>> {
    let cleaned = sanitize_fts(q);
    if cleaned.is_empty() {
        return Ok(Vec::new());
    }
    let fts_query = build_fts_query(&cleaned);
    if fts_query.is_empty() {
        return Ok(Vec::new());
    }

    let rows = if let Some(mb) = mailbox_id {
        sqlx::query(
            r"SELECT e.id, e.mailbox_id, e.received_at, e.smtp_from, e.smtp_to_json,
                     e.header_subject, e.has_html, e.has_text, e.size_bytes, e.read_flag
              FROM emails_fts f
              JOIN emails e ON e.id = f.email_id
              WHERE emails_fts MATCH ?1 AND e.mailbox_id = ?2
              ORDER BY e.received_at DESC
              LIMIT ?3",
        )
        .bind(&fts_query)
        .bind(mb)
        .bind(i64::from(limit))
        .fetch_all(pool)
        .await?
    } else {
        sqlx::query(
            r"SELECT e.id, e.mailbox_id, e.received_at, e.smtp_from, e.smtp_to_json,
                     e.header_subject, e.has_html, e.has_text, e.size_bytes, e.read_flag
              FROM emails_fts f
              JOIN emails e ON e.id = f.email_id
              WHERE emails_fts MATCH ?1
              ORDER BY e.received_at DESC
              LIMIT ?2",
        )
        .bind(&fts_query)
        .bind(i64::from(limit))
        .fetch_all(pool)
        .await?
    };

    let mut out = Vec::with_capacity(rows.len());
    for row in rows {
        out.push(row_to_summary(&row)?);
    }
    Ok(out)
}

/// IDs of emails older than `cutoff_ms` (used by retention).
pub(crate) async fn list_older_than(
    pool: &SqlitePool,
    cutoff_ms: i64,
) -> Result<Vec<(String, String, String)>> {
    let rows = sqlx::query(
        r"SELECT id, mailbox_id, raw_path FROM emails WHERE received_at < ?",
    )
    .bind(cutoff_ms)
    .fetch_all(pool)
    .await?;
    Ok(rows
        .into_iter()
        .map(|r| {
            (
                r.try_get("id").unwrap_or_default(),
                r.try_get("mailbox_id").unwrap_or_default(),
                r.try_get("raw_path").unwrap_or_default(),
            )
        })
        .collect())
}

/// Trim a mailbox down to `keep_max` newest rows; return ids/paths to remove.
///
/// Pinned rows are never trimmed — they don't count toward `keep_max`
/// and they're excluded from the candidate-for-deletion set. This
/// (pin survives retention).
pub(crate) async fn trim_mailbox(
    pool: &SqlitePool,
    mailbox_id: &str,
    keep_max: i64,
) -> Result<Vec<(String, String)>> {
    let rows = sqlx::query(
        r"SELECT id, raw_path FROM emails
          WHERE mailbox_id = ?
            AND pinned = 0
            AND id NOT IN (
                SELECT id FROM emails
                WHERE mailbox_id = ?
                  AND pinned = 0
                ORDER BY received_at DESC
                LIMIT ?
            )",
    )
    .bind(mailbox_id)
    .bind(mailbox_id)
    .bind(keep_max)
    .fetch_all(pool)
    .await?;

    Ok(rows
        .into_iter()
        .map(|r| {
            (
                r.try_get("id").unwrap_or_default(),
                r.try_get("raw_path").unwrap_or_default(),
            )
        })
        .collect())
}

pub(crate) async fn delete_by_ids(pool: &SqlitePool, ids: &[String]) -> Result<()> {
    if ids.is_empty() {
        return Ok(());
    }
    let mut tx = pool.begin().await?;
    for id in ids {
        sqlx::query("DELETE FROM emails WHERE id = ?")
            .bind(id)
            .execute(&mut *tx)
            .await?;
        sqlx::query("DELETE FROM emails_fts WHERE email_id = ?")
            .bind(id)
            .execute(&mut *tx)
            .await?;
    }
    tx.commit().await?;
    Ok(())
}

pub(crate) async fn list_all_raw_paths(pool: &SqlitePool) -> Result<Vec<String>> {
    let rows = sqlx::query("SELECT raw_path FROM emails").fetch_all(pool).await?;
    Ok(rows
        .into_iter()
        .filter_map(|r| r.try_get("raw_path").ok())
        .collect())
}

fn row_to_summary(row: &sqlx::sqlite::SqliteRow) -> Result<EmailSummary> {
    let smtp_to_json: String = row.try_get("smtp_to_json").unwrap_or_default();
    let to: Vec<String> = serde_json::from_str(&smtp_to_json).unwrap_or_default();
    Ok(EmailSummary {
        id: row.try_get("id").unwrap_or_default(),
        mailbox_id: row.try_get("mailbox_id").unwrap_or_default(),
        received_at: row.try_get("received_at").unwrap_or(0),
        from: row.try_get("smtp_from").unwrap_or_default(),
        to,
        subject: row.try_get("header_subject").ok(),
        has_html: row.try_get::<i64, _>("has_html").unwrap_or(0) != 0,
        has_text: row.try_get::<i64, _>("has_text").unwrap_or(0) != 0,
        size_bytes: row.try_get("size_bytes").unwrap_or(0),
        read: row.try_get::<i64, _>("read_flag").unwrap_or(0) != 0,
        pinned: row.try_get::<i64, _>("pinned").unwrap_or(0) != 0,
        starred: row.try_get::<i64, _>("starred").unwrap_or(0) != 0,
        tag: row
            .try_get::<Option<String>, _>("tag")
            .ok()
            .flatten(),
    })
}

pub(crate) async fn set_pinned(pool: &SqlitePool, id: &str, pinned: bool) -> Result<()> {
    let res = sqlx::query("UPDATE emails SET pinned = ? WHERE id = ?")
        .bind(i64::from(pinned))
        .bind(id)
        .execute(pool)
        .await?;
    if res.rows_affected() == 0 {
        return Err(Error::EmailNotFound(id.to_string()));
    }
    Ok(())
}

pub(crate) async fn set_starred(pool: &SqlitePool, id: &str, starred: bool) -> Result<()> {
    let res = sqlx::query("UPDATE emails SET starred = ? WHERE id = ?")
        .bind(i64::from(starred))
        .bind(id)
        .execute(pool)
        .await?;
    if res.rows_affected() == 0 {
        return Err(Error::EmailNotFound(id.to_string()));
    }
    Ok(())
}

pub(crate) async fn set_note(pool: &SqlitePool, id: &str, note: Option<&str>) -> Result<()> {
    let res = sqlx::query("UPDATE emails SET note = ? WHERE id = ?")
        .bind(note)
        .bind(id)
        .execute(pool)
        .await?;
    if res.rows_affected() == 0 {
        return Err(Error::EmailNotFound(id.to_string()));
    }
    Ok(())
}

pub(crate) async fn set_tag(pool: &SqlitePool, id: &str, tag: Option<&str>) -> Result<()> {
    let res = sqlx::query("UPDATE emails SET tag = ? WHERE id = ?")
        .bind(tag)
        .bind(id)
        .execute(pool)
        .await?;
    if res.rows_affected() == 0 {
        return Err(Error::EmailNotFound(id.to_string()));
    }
    Ok(())
}

/// Drop anything that could be interpreted as FTS5 query syntax. We
/// keep `.` `@` `_` because they appear in addresses and identifiers;
/// the tokenizer treats them as separators anyway. We deliberately
/// drop `-` because in an FTS5 expression an unquoted `-` is the NOT
/// operator (`foo -bar` ≡ "foo AND NOT bar"), which surprises users
/// who type hyphenated terms like `password-reset`.
fn sanitize_fts(q: &str) -> String {
    q.chars()
        .filter(|c| c.is_alphanumeric() || c.is_whitespace() || matches!(*c, '.' | '@' | '_'))
        .collect::<String>()
        .trim()
        .to_string()
}

/// Turn the sanitized query into an FTS5 MATCH expression. Each token
/// becomes a prefix term (`foo*`) so partial words match — `"alic"`
/// finds emails containing "alice". Tokens combine with AND (the FTS5
/// default), so multi-word queries narrow the result set.
fn build_fts_query(cleaned: &str) -> String {
    cleaned
        .split_whitespace()
        .filter(|t| !t.is_empty())
        .map(|t| format!("{t}*"))
        .collect::<Vec<_>>()
        .join(" ")
}
