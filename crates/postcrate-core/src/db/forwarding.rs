//! Auto-forwarding rule CRUD.

use chrono::Utc;
use serde::{Deserialize, Serialize};
use sqlx::{Row, SqlitePool};
use uuid::Uuid;

use crate::error::{Error, Result};
use crate::smtp::relay::RelayConfig;

#[derive(Debug, Clone, Serialize, Deserialize)]
#[cfg_attr(feature = "specta", derive(specta::Type))]
#[serde(rename_all = "camelCase")]
pub struct ForwardingRule {
    pub id: String,
    /// `None` means "forward emails from every mailbox".
    pub mailbox_id: Option<String>,
    pub target_addresses: Vec<String>,
    pub relay: RelayConfig,
    pub enabled: bool,
    pub created_at: i64,
}

#[derive(Debug, Clone, Deserialize)]
#[cfg_attr(feature = "specta", derive(specta::Type))]
#[serde(rename_all = "camelCase")]
pub struct CreateForwardingRule {
    pub mailbox_id: Option<String>,
    pub target_addresses: Vec<String>,
    pub relay: RelayConfig,
    pub enabled: Option<bool>,
}

pub(crate) async fn insert(pool: &SqlitePool, input: CreateForwardingRule) -> Result<ForwardingRule> {
    if input.target_addresses.is_empty() {
        return Err(Error::Invalid("forwarding rule needs at least one target".into()));
    }
    let id = Uuid::new_v4().to_string();
    let now = Utc::now().timestamp_millis();
    let enabled = input.enabled.unwrap_or(true);
    let targets = serde_json::to_string(&input.target_addresses)?;
    let relay = serde_json::to_string(&input.relay)?;
    sqlx::query(
        r"INSERT INTO forwarding_rules
            (id, mailbox_id, target_addresses, relay_json, enabled, created_at)
          VALUES (?, ?, ?, ?, ?, ?)",
    )
    .bind(&id)
    .bind(&input.mailbox_id)
    .bind(&targets)
    .bind(&relay)
    .bind(i64::from(enabled))
    .bind(now)
    .execute(pool)
    .await?;
    Ok(ForwardingRule {
        id,
        mailbox_id: input.mailbox_id,
        target_addresses: input.target_addresses,
        relay: input.relay,
        enabled,
        created_at: now,
    })
}

pub(crate) async fn list(pool: &SqlitePool) -> Result<Vec<ForwardingRule>> {
    let rows = sqlx::query(
        r"SELECT id, mailbox_id, target_addresses, relay_json, enabled, created_at
          FROM forwarding_rules",
    )
    .fetch_all(pool)
    .await?;
    let mut out = Vec::with_capacity(rows.len());
    for r in rows {
        out.push(row_to_rule(&r)?);
    }
    Ok(out)
}

pub(crate) async fn list_for_mailbox(
    pool: &SqlitePool,
    mailbox_id: &str,
) -> Result<Vec<ForwardingRule>> {
    let rows = sqlx::query(
        r"SELECT id, mailbox_id, target_addresses, relay_json, enabled, created_at
          FROM forwarding_rules
          WHERE enabled = 1 AND (mailbox_id IS NULL OR mailbox_id = ?)",
    )
    .bind(mailbox_id)
    .fetch_all(pool)
    .await?;
    let mut out = Vec::with_capacity(rows.len());
    for r in rows {
        out.push(row_to_rule(&r)?);
    }
    Ok(out)
}

pub(crate) async fn delete(pool: &SqlitePool, id: &str) -> Result<()> {
    let res = sqlx::query("DELETE FROM forwarding_rules WHERE id = ?")
        .bind(id)
        .execute(pool)
        .await?;
    if res.rows_affected() == 0 {
        return Err(Error::Invalid(format!("forwarding rule {id} not found")));
    }
    Ok(())
}

fn row_to_rule(row: &sqlx::sqlite::SqliteRow) -> Result<ForwardingRule> {
    let targets_json: String = row.try_get("target_addresses").unwrap_or_default();
    let relay_json: String = row.try_get("relay_json").unwrap_or_default();
    let target_addresses: Vec<String> = serde_json::from_str(&targets_json).unwrap_or_default();
    let relay: RelayConfig = serde_json::from_str(&relay_json)?;
    Ok(ForwardingRule {
        id: row.try_get("id").unwrap_or_default(),
        mailbox_id: row.try_get::<Option<String>, _>("mailbox_id").ok().flatten(),
        target_addresses,
        relay,
        enabled: row.try_get::<i64, _>("enabled").unwrap_or(0) != 0,
        created_at: row.try_get("created_at").unwrap_or(0),
    })
}
