//! Mailbox row storage.
//!
//! Note: this module is `pub` because [`crate::Service`] re-exports the
//! `Mailbox`, `CreateMailboxInput`, etc. types — they're part of the
//! library's wire surface.

use chrono::Utc;
use serde::{Deserialize, Serialize};
use sqlx::{Row, SqlitePool};
use uuid::Uuid;

use crate::error::{Error, Result};
use crate::mailbox::kinds::MailboxKind;

/// A mailbox row joined with its current message count.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Mailbox {
    pub id: String,
    pub project_id: String,
    pub name: String,
    pub port: u16,
    pub kind: MailboxKind,
    pub ttl_seconds: Option<u64>,
    pub expires_at: Option<i64>,
    pub failed: bool,
    pub fail_reason: Option<String>,
    pub created_at: i64,
    pub count: i64,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CreateMailboxInput {
    pub project_id: String,
    pub name: String,
    pub kind: MailboxKind,
    pub port: Option<u16>,
    pub ttl_seconds: Option<u64>,
}

#[derive(Debug, Clone, Default, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct UpdateMailboxInput {
    pub name: Option<String>,
    pub port: Option<u16>,
    pub ttl_seconds: Option<Option<u64>>, // None means leave alone; Some(None) means clear
}

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CreateEphemeralInput {
    pub project_id: String,
    pub name: Option<String>,
    pub ttl_seconds: u64,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct EphemeralHandle {
    pub id: String,
    pub host: String,
    pub port: u16,
    pub expires_at: i64,
}

#[derive(Debug, Clone)]
pub(crate) struct MailboxRow {
    pub id: String,
    pub project_id: String,
    pub name: String,
    pub port: u16,
    pub kind: MailboxKind,
    pub ttl_seconds: Option<u64>,
    pub expires_at: Option<i64>,
    pub failed: bool,
    pub fail_reason: Option<String>,
    pub created_at: i64,
}

impl MailboxRow {
    pub(crate) fn with_count(self, count: i64) -> Mailbox {
        Mailbox {
            id: self.id,
            project_id: self.project_id,
            name: self.name,
            port: self.port,
            kind: self.kind,
            ttl_seconds: self.ttl_seconds,
            expires_at: self.expires_at,
            failed: self.failed,
            fail_reason: self.fail_reason,
            created_at: self.created_at,
            count,
        }
    }
}

pub(crate) async fn insert(
    pool: &SqlitePool,
    project_id: &str,
    name: &str,
    port: u16,
    kind: MailboxKind,
    ttl_seconds: Option<u64>,
) -> Result<MailboxRow> {
    let now = Utc::now().timestamp_millis();
    let expires_at = ttl_seconds.map(|t| now + (t as i64) * 1000);
    let id = Uuid::new_v4().to_string();

    let res = sqlx::query(
        r"INSERT INTO mailboxes
            (id, project_id, name, port, kind, ttl_seconds, expires_at, created_at)
          VALUES (?, ?, ?, ?, ?, ?, ?, ?)",
    )
    .bind(&id)
    .bind(project_id)
    .bind(name)
    .bind(i64::from(port))
    .bind(kind.as_str())
    .bind(ttl_seconds.map(|t| t as i64))
    .bind(expires_at)
    .bind(now)
    .execute(pool)
    .await;

    match res {
        Ok(_) => Ok(MailboxRow {
            id,
            project_id: project_id.to_string(),
            name: name.to_string(),
            port,
            kind,
            ttl_seconds,
            expires_at,
            failed: false,
            fail_reason: None,
            created_at: now,
        }),
        Err(sqlx::Error::Database(e)) if e.is_unique_violation() => {
            // Either name collision in project, or port collision globally.
            let msg = e.message().to_lowercase();
            if msg.contains("mailboxes.port") {
                Err(Error::PortInUse(port))
            } else {
                Err(Error::DuplicateMailbox(name.to_string()))
            }
        }
        Err(e) => Err(e.into()),
    }
}

pub(crate) async fn get(pool: &SqlitePool, id: &str) -> Result<MailboxRow> {
    let row = sqlx::query(
        r"SELECT id, project_id, name, port, kind, ttl_seconds, expires_at,
                 failed, fail_reason, created_at
          FROM mailboxes WHERE id = ?",
    )
    .bind(id)
    .fetch_optional(pool)
    .await?
    .ok_or_else(|| Error::MailboxNotFound(id.to_string()))?;

    Ok(row_to_mailbox_row(&row))
}

pub(crate) async fn list(pool: &SqlitePool, project_id: Option<&str>) -> Result<Vec<Mailbox>> {
    let sql = if project_id.is_some() {
        r"SELECT m.id, m.project_id, m.name, m.port, m.kind, m.ttl_seconds, m.expires_at,
                 m.failed, m.fail_reason, m.created_at,
                 COALESCE(c.cnt, 0) AS cnt
          FROM mailboxes m
          LEFT JOIN (SELECT mailbox_id, COUNT(*) AS cnt FROM emails GROUP BY mailbox_id) c
                 ON c.mailbox_id = m.id
          WHERE m.project_id = ?
          ORDER BY m.created_at ASC"
    } else {
        r"SELECT m.id, m.project_id, m.name, m.port, m.kind, m.ttl_seconds, m.expires_at,
                 m.failed, m.fail_reason, m.created_at,
                 COALESCE(c.cnt, 0) AS cnt
          FROM mailboxes m
          LEFT JOIN (SELECT mailbox_id, COUNT(*) AS cnt FROM emails GROUP BY mailbox_id) c
                 ON c.mailbox_id = m.id
          ORDER BY m.created_at ASC"
    };

    let mut q = sqlx::query(sql);
    if let Some(p) = project_id {
        q = q.bind(p);
    }
    let rows = q.fetch_all(pool).await?;

    Ok(rows
        .into_iter()
        .map(|row| {
            let mb = row_to_mailbox_row(&row);
            let count: i64 = row.try_get("cnt").unwrap_or(0);
            mb.with_count(count)
        })
        .collect())
}

pub(crate) async fn count_emails(pool: &SqlitePool, mailbox_id: &str) -> Result<i64> {
    let row = sqlx::query("SELECT COUNT(*) AS c FROM emails WHERE mailbox_id = ?")
        .bind(mailbox_id)
        .fetch_one(pool)
        .await?;
    Ok(row.try_get::<i64, _>("c").unwrap_or(0))
}

pub(crate) async fn update(
    pool: &SqlitePool,
    id: &str,
    patch: &UpdateMailboxInput,
) -> Result<MailboxRow> {
    let current = get(pool, id).await?;

    let new_name = patch.name.clone().unwrap_or(current.name.clone());
    let new_port = patch.port.unwrap_or(current.port);
    let new_ttl = match patch.ttl_seconds {
        None => current.ttl_seconds,
        Some(v) => v,
    };

    let res = sqlx::query(
        r"UPDATE mailboxes
            SET name = ?, port = ?, ttl_seconds = ?
          WHERE id = ?",
    )
    .bind(&new_name)
    .bind(i64::from(new_port))
    .bind(new_ttl.map(|t| t as i64))
    .bind(id)
    .execute(pool)
    .await;

    match res {
        Ok(_) => get(pool, id).await,
        Err(sqlx::Error::Database(e)) if e.is_unique_violation() => {
            let msg = e.message().to_lowercase();
            if msg.contains("mailboxes.port") {
                Err(Error::PortInUse(new_port))
            } else {
                Err(Error::DuplicateMailbox(new_name))
            }
        }
        Err(e) => Err(e.into()),
    }
}

pub(crate) async fn delete(pool: &SqlitePool, id: &str) -> Result<()> {
    let res = sqlx::query("DELETE FROM mailboxes WHERE id = ?")
        .bind(id)
        .execute(pool)
        .await?;
    if res.rows_affected() == 0 {
        return Err(Error::MailboxNotFound(id.to_string()));
    }
    Ok(())
}

pub(crate) async fn mark_failed(
    pool: &SqlitePool,
    id: &str,
    reason: Option<&str>,
) -> Result<()> {
    sqlx::query("UPDATE mailboxes SET failed = 1, fail_reason = ? WHERE id = ?")
        .bind(reason)
        .bind(id)
        .execute(pool)
        .await?;
    Ok(())
}

pub(crate) async fn clear_failed(pool: &SqlitePool, id: &str) -> Result<()> {
    sqlx::query("UPDATE mailboxes SET failed = 0, fail_reason = NULL WHERE id = ?")
        .bind(id)
        .execute(pool)
        .await?;
    Ok(())
}

/// Drop expired ephemerals at boot. Returns IDs that were swept so the
/// caller can also clean up any orphan raw blobs.
pub(crate) async fn sweep_expired_ephemerals(pool: &SqlitePool) -> Result<Vec<String>> {
    let now = Utc::now().timestamp_millis();
    let rows = sqlx::query(
        r"SELECT id FROM mailboxes
           WHERE kind = 'ephemeral'
             AND (expires_at IS NULL OR expires_at < ?)",
    )
    .bind(now)
    .fetch_all(pool)
    .await?;

    let ids: Vec<String> = rows
        .iter()
        .filter_map(|r| r.try_get::<String, _>("id").ok())
        .collect();

    if !ids.is_empty() {
        sqlx::query(
            r"DELETE FROM mailboxes
               WHERE kind = 'ephemeral'
                 AND (expires_at IS NULL OR expires_at < ?)",
        )
        .bind(now)
        .execute(pool)
        .await?;
    }
    Ok(ids)
}

pub(crate) async fn list_all_ports(pool: &SqlitePool) -> Result<Vec<u16>> {
    let rows = sqlx::query("SELECT port FROM mailboxes").fetch_all(pool).await?;
    Ok(rows
        .into_iter()
        .filter_map(|r| r.try_get::<i64, _>("port").ok())
        .map(|n| n as u16)
        .collect())
}

pub(crate) async fn list_active_for_boot(pool: &SqlitePool) -> Result<Vec<MailboxRow>> {
    let rows = sqlx::query(
        r"SELECT id, project_id, name, port, kind, ttl_seconds, expires_at,
                 failed, fail_reason, created_at
          FROM mailboxes",
    )
    .fetch_all(pool)
    .await?;
    Ok(rows.iter().map(row_to_mailbox_row).collect())
}

pub(crate) async fn list_expiring(pool: &SqlitePool) -> Result<Vec<(String, i64)>> {
    let rows = sqlx::query(
        r"SELECT id, expires_at FROM mailboxes
          WHERE kind = 'ephemeral' AND expires_at IS NOT NULL",
    )
    .fetch_all(pool)
    .await?;
    Ok(rows
        .into_iter()
        .filter_map(|r| {
            let id: String = r.try_get("id").ok()?;
            let exp: i64 = r.try_get("expires_at").ok()?;
            Some((id, exp))
        })
        .collect())
}

fn row_to_mailbox_row(row: &sqlx::sqlite::SqliteRow) -> MailboxRow {
    let kind_str: String = row.try_get("kind").unwrap_or_else(|_| "primary".into());
    let kind = MailboxKind::from_str(&kind_str).unwrap_or(MailboxKind::Primary);
    let ttl_seconds: Option<i64> = row.try_get("ttl_seconds").ok();
    let port_i64: i64 = row.try_get("port").unwrap_or(0);
    let failed_i: i64 = row.try_get("failed").unwrap_or(0);

    MailboxRow {
        id: row.try_get("id").unwrap_or_default(),
        project_id: row.try_get("project_id").unwrap_or_default(),
        name: row.try_get("name").unwrap_or_default(),
        port: port_i64 as u16,
        kind,
        ttl_seconds: ttl_seconds.map(|t| t as u64),
        expires_at: row.try_get("expires_at").ok(),
        failed: failed_i != 0,
        fail_reason: row.try_get("fail_reason").ok(),
        created_at: row.try_get("created_at").unwrap_or(0),
    }
}
