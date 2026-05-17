//! Bounce rule storage. Glob-matched at RCPT TO time by `smtp::bounce`.

use chrono::Utc;
use serde::{Deserialize, Serialize};
use sqlx::{Row, SqlitePool};
use uuid::Uuid;

use crate::error::{Error, Result};
use crate::events::BounceKind;

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct BounceRule {
    #[serde(default)]
    pub id: String,
    pub mailbox_id: String,
    pub address_pattern: String,
    pub bounce_kind: BounceKind,
    pub smtp_code: u16,
    pub smtp_message: String,
    #[serde(default = "default_enabled")]
    pub enabled: bool,
    #[serde(default)]
    pub created_at: i64,
}

fn default_enabled() -> bool {
    true
}

pub(crate) async fn list(pool: &SqlitePool, mailbox_id: &str) -> Result<Vec<BounceRule>> {
    let rows = sqlx::query(
        r"SELECT id, mailbox_id, address_pattern, bounce_kind, smtp_code,
                 smtp_message, enabled, created_at
          FROM bounce_rules
          WHERE mailbox_id = ?
          ORDER BY created_at ASC",
    )
    .bind(mailbox_id)
    .fetch_all(pool)
    .await?;
    Ok(rows.iter().map(row_to_rule).collect())
}

pub(crate) async fn list_enabled(pool: &SqlitePool, mailbox_id: &str) -> Result<Vec<BounceRule>> {
    let rows = sqlx::query(
        r"SELECT id, mailbox_id, address_pattern, bounce_kind, smtp_code,
                 smtp_message, enabled, created_at
          FROM bounce_rules
          WHERE mailbox_id = ? AND enabled = 1",
    )
    .bind(mailbox_id)
    .fetch_all(pool)
    .await?;
    Ok(rows.iter().map(row_to_rule).collect())
}

pub(crate) async fn upsert(pool: &SqlitePool, mut rule: BounceRule) -> Result<BounceRule> {
    if rule.id.is_empty() {
        rule.id = Uuid::new_v4().to_string();
    }
    if rule.created_at == 0 {
        rule.created_at = Utc::now().timestamp_millis();
    }
    if !(400..600).contains(&rule.smtp_code) {
        return Err(Error::Invalid(format!(
            "smtp_code {} must be 4xx or 5xx",
            rule.smtp_code
        )));
    }

    sqlx::query(
        r"INSERT INTO bounce_rules
            (id, mailbox_id, address_pattern, bounce_kind, smtp_code,
             smtp_message, enabled, created_at)
          VALUES (?, ?, ?, ?, ?, ?, ?, ?)
          ON CONFLICT(id) DO UPDATE SET
            address_pattern = excluded.address_pattern,
            bounce_kind     = excluded.bounce_kind,
            smtp_code       = excluded.smtp_code,
            smtp_message    = excluded.smtp_message,
            enabled         = excluded.enabled",
    )
    .bind(&rule.id)
    .bind(&rule.mailbox_id)
    .bind(&rule.address_pattern)
    .bind(rule.bounce_kind.as_str())
    .bind(i64::from(rule.smtp_code))
    .bind(&rule.smtp_message)
    .bind(i64::from(rule.enabled))
    .bind(rule.created_at)
    .execute(pool)
    .await?;

    Ok(rule)
}

pub(crate) async fn delete(pool: &SqlitePool, id: &str) -> Result<()> {
    let res = sqlx::query("DELETE FROM bounce_rules WHERE id = ?")
        .bind(id)
        .execute(pool)
        .await?;
    if res.rows_affected() == 0 {
        return Err(Error::BounceRuleNotFound(id.to_string()));
    }
    Ok(())
}

fn row_to_rule(row: &sqlx::sqlite::SqliteRow) -> BounceRule {
    let kind_str: String = row.try_get("bounce_kind").unwrap_or_else(|_| "hard".into());
    BounceRule {
        id: row.try_get("id").unwrap_or_default(),
        mailbox_id: row.try_get("mailbox_id").unwrap_or_default(),
        address_pattern: row.try_get("address_pattern").unwrap_or_default(),
        bounce_kind: BounceKind::from_str(&kind_str),
        smtp_code: (row.try_get::<i64, _>("smtp_code").unwrap_or(550)) as u16,
        smtp_message: row.try_get("smtp_message").unwrap_or_default(),
        enabled: row.try_get::<i64, _>("enabled").unwrap_or(1) != 0,
        created_at: row.try_get("created_at").unwrap_or(0),
    }
}
