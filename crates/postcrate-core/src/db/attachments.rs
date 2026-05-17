//! Attachment metadata + blob lookup.

use sqlx::{Row, SqlitePool};

use crate::db::emails::AttachmentMeta;
use crate::error::{Error, Result};

#[derive(Debug, Clone)]
pub(crate) struct AttachmentInsert {
    pub id: String,
    pub filename: Option<String>,
    pub content_type: Option<String>,
    pub content_id: Option<String>,
    pub size_bytes: i64,
    pub blob_path: String,
}

pub(crate) async fn list_for_email(
    pool: &SqlitePool,
    email_id: &str,
) -> Result<Vec<AttachmentMeta>> {
    let rows = sqlx::query(
        r"SELECT id, filename, content_type, content_id, size_bytes
          FROM attachments WHERE email_id = ?",
    )
    .bind(email_id)
    .fetch_all(pool)
    .await?;

    Ok(rows
        .into_iter()
        .map(|r| AttachmentMeta {
            id: r.try_get("id").unwrap_or_default(),
            filename: r.try_get("filename").ok(),
            content_type: r.try_get("content_type").ok(),
            content_id: r.try_get("content_id").ok(),
            size_bytes: r.try_get("size_bytes").unwrap_or(0),
        })
        .collect())
}

pub(crate) async fn get_blob_path(
    pool: &SqlitePool,
    attachment_id: &str,
) -> Result<(String, Option<String>, Option<String>)> {
    let row = sqlx::query(
        r"SELECT blob_path, filename, content_type
          FROM attachments WHERE id = ?",
    )
    .bind(attachment_id)
    .fetch_optional(pool)
    .await?
    .ok_or_else(|| Error::AttachmentNotFound(attachment_id.to_string()))?;
    Ok((
        row.try_get("blob_path").unwrap_or_default(),
        row.try_get("filename").ok(),
        row.try_get("content_type").ok(),
    ))
}
