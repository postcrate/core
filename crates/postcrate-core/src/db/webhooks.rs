//! Webhook CRUD.

use chrono::Utc;
use serde::{Deserialize, Serialize};
use sqlx::{Row, SqlitePool};
use uuid::Uuid;

use crate::error::{Error, Result};

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Webhook {
    pub id: String,
    /// `None` means this is a global webhook (fires for every mailbox).
    pub mailbox_id: Option<String>,
    pub url: String,
    /// Sent as the `Authorization` header verbatim. Use this for
    /// `Bearer <token>` or basic-auth strings.
    pub auth_header: Option<String>,
    pub enabled: bool,
    pub created_at: i64,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CreateWebhook {
    pub mailbox_id: Option<String>,
    pub url: String,
    pub auth_header: Option<String>,
    pub enabled: Option<bool>,
}

pub(crate) async fn insert(pool: &SqlitePool, input: CreateWebhook) -> Result<Webhook> {
    let id = Uuid::new_v4().to_string();
    let now = Utc::now().timestamp_millis();
    let enabled = input.enabled.unwrap_or(true);
    sqlx::query(
        r"INSERT INTO webhooks (id, mailbox_id, url, auth_header, enabled, created_at)
          VALUES (?, ?, ?, ?, ?, ?)",
    )
    .bind(&id)
    .bind(&input.mailbox_id)
    .bind(&input.url)
    .bind(&input.auth_header)
    .bind(i64::from(enabled))
    .bind(now)
    .execute(pool)
    .await?;
    Ok(Webhook {
        id,
        mailbox_id: input.mailbox_id,
        url: input.url,
        auth_header: input.auth_header,
        enabled,
        created_at: now,
    })
}

pub(crate) async fn list(pool: &SqlitePool) -> Result<Vec<Webhook>> {
    let rows =
        sqlx::query("SELECT id, mailbox_id, url, auth_header, enabled, created_at FROM webhooks")
            .fetch_all(pool)
            .await?;
    Ok(rows.iter().map(row_to_webhook).collect())
}

pub(crate) async fn list_for_mailbox(pool: &SqlitePool, mailbox_id: &str) -> Result<Vec<Webhook>> {
    let rows = sqlx::query(
        r"SELECT id, mailbox_id, url, auth_header, enabled, created_at
          FROM webhooks
          WHERE enabled = 1 AND (mailbox_id IS NULL OR mailbox_id = ?)",
    )
    .bind(mailbox_id)
    .fetch_all(pool)
    .await?;
    Ok(rows.iter().map(row_to_webhook).collect())
}

pub(crate) async fn delete(pool: &SqlitePool, id: &str) -> Result<()> {
    let res = sqlx::query("DELETE FROM webhooks WHERE id = ?")
        .bind(id)
        .execute(pool)
        .await?;
    if res.rows_affected() == 0 {
        return Err(Error::Invalid(format!("webhook {id} not found")));
    }
    Ok(())
}

fn row_to_webhook(row: &sqlx::sqlite::SqliteRow) -> Webhook {
    Webhook {
        id: row.try_get("id").unwrap_or_default(),
        mailbox_id: row
            .try_get::<Option<String>, _>("mailbox_id")
            .ok()
            .flatten(),
        url: row.try_get("url").unwrap_or_default(),
        auth_header: row
            .try_get::<Option<String>, _>("auth_header")
            .ok()
            .flatten(),
        enabled: row.try_get::<i64, _>("enabled").unwrap_or(0) != 0,
        created_at: row.try_get("created_at").unwrap_or(0),
    }
}
