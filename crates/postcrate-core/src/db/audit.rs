//! Append-only audit log.

use chrono::Utc;
use serde::Serialize;
use sqlx::{Row, SqlitePool};

use crate::error::Result;

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct AuditEntry {
    pub id: i64,
    pub at: i64,
    pub actor: String,
    pub action: String,
    pub target_kind: Option<String>,
    pub target_id: Option<String>,
    pub metadata: Option<serde_json::Value>,
}

#[derive(Debug, Clone)]
pub(crate) struct AuditAppend {
    pub actor: String,
    pub action: String,
    pub target_kind: Option<String>,
    pub target_id: Option<String>,
    pub metadata: Option<serde_json::Value>,
}

pub(crate) async fn append(pool: &SqlitePool, entry: AuditAppend) -> Result<AuditEntry> {
    let now = Utc::now().timestamp_millis();
    let metadata_json = entry
        .metadata
        .as_ref()
        .map(|v| serde_json::to_string(v).unwrap_or_else(|_| "null".into()));

    let res = sqlx::query(
        r"INSERT INTO audit_log (at, actor, action, target_kind, target_id, metadata_json)
          VALUES (?, ?, ?, ?, ?, ?)",
    )
    .bind(now)
    .bind(&entry.actor)
    .bind(&entry.action)
    .bind(&entry.target_kind)
    .bind(&entry.target_id)
    .bind(&metadata_json)
    .execute(pool)
    .await?;

    Ok(AuditEntry {
        id: res.last_insert_rowid(),
        at: now,
        actor: entry.actor,
        action: entry.action,
        target_kind: entry.target_kind,
        target_id: entry.target_id,
        metadata: entry.metadata,
    })
}

pub(crate) async fn list(pool: &SqlitePool, limit: u32, offset: u32) -> Result<Vec<AuditEntry>> {
    let rows = sqlx::query(
        r"SELECT id, at, actor, action, target_kind, target_id, metadata_json
          FROM audit_log
          ORDER BY at DESC
          LIMIT ? OFFSET ?",
    )
    .bind(i64::from(limit))
    .bind(i64::from(offset))
    .fetch_all(pool)
    .await?;
    Ok(rows.iter().map(row_to_entry).collect())
}

pub(crate) async fn prune_older_than(pool: &SqlitePool, days: u32) -> Result<u64> {
    let cutoff = Utc::now().timestamp_millis() - (i64::from(days) * 86_400_000);
    let res = sqlx::query("DELETE FROM audit_log WHERE at < ?")
        .bind(cutoff)
        .execute(pool)
        .await?;
    Ok(res.rows_affected())
}

pub(crate) async fn clear_all(pool: &SqlitePool) -> Result<u64> {
    let res = sqlx::query("DELETE FROM audit_log").execute(pool).await?;
    Ok(res.rows_affected())
}

fn row_to_entry(row: &sqlx::sqlite::SqliteRow) -> AuditEntry {
    let metadata_json: Option<String> = row.try_get("metadata_json").ok();
    let metadata = metadata_json
        .as_deref()
        .and_then(|s| serde_json::from_str(s).ok());
    AuditEntry {
        id: row.try_get("id").unwrap_or(0),
        at: row.try_get("at").unwrap_or(0),
        actor: row.try_get("actor").unwrap_or_default(),
        action: row.try_get("action").unwrap_or_default(),
        target_kind: row.try_get("target_kind").ok(),
        target_id: row.try_get("target_id").ok(),
        metadata,
    }
}
