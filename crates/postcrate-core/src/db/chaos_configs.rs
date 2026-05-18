//! Chaos config storage (one row per mailbox).

use serde::{Deserialize, Serialize};
use sqlx::{Row, SqlitePool};

use crate::error::Result;

#[derive(Debug, Clone, Serialize, Deserialize)]
#[cfg_attr(feature = "specta", derive(specta::Type))]
#[serde(rename_all = "camelCase")]
pub struct ChaosConfig {
    #[serde(default)]
    pub enabled: bool,
    #[serde(default)]
    pub reject_4xx_prob: f32,
    #[serde(default)]
    pub reject_5xx_prob: f32,
    #[serde(default)]
    pub delay_ms_min: u32,
    #[serde(default)]
    pub delay_ms_max: u32,
    #[serde(default)]
    pub drop_during_data_prob: f32,
    #[serde(default)]
    pub malformed_resp_prob: f32,
    #[serde(default)]
    pub seed: Option<u64>,
}

impl Default for ChaosConfig {
    fn default() -> Self {
        Self {
            enabled: false,
            reject_4xx_prob: 0.0,
            reject_5xx_prob: 0.0,
            delay_ms_min: 0,
            delay_ms_max: 0,
            drop_during_data_prob: 0.0,
            malformed_resp_prob: 0.0,
            seed: None,
        }
    }
}

pub(crate) async fn get(pool: &SqlitePool, mailbox_id: &str) -> Result<ChaosConfig> {
    let row = sqlx::query(
        r"SELECT enabled, reject_4xx_prob, reject_5xx_prob, delay_ms_min, delay_ms_max,
                 drop_during_data_prob, malformed_resp_prob, seed
          FROM chaos_configs WHERE mailbox_id = ?",
    )
    .bind(mailbox_id)
    .fetch_optional(pool)
    .await?;

    let Some(row) = row else {
        return Ok(ChaosConfig::default());
    };

    Ok(ChaosConfig {
        enabled: row.try_get::<i64, _>("enabled").unwrap_or(0) != 0,
        reject_4xx_prob: row.try_get::<f64, _>("reject_4xx_prob").unwrap_or(0.0) as f32,
        reject_5xx_prob: row.try_get::<f64, _>("reject_5xx_prob").unwrap_or(0.0) as f32,
        delay_ms_min: row.try_get::<i64, _>("delay_ms_min").unwrap_or(0) as u32,
        delay_ms_max: row.try_get::<i64, _>("delay_ms_max").unwrap_or(0) as u32,
        drop_during_data_prob: row
            .try_get::<f64, _>("drop_during_data_prob")
            .unwrap_or(0.0) as f32,
        malformed_resp_prob: row.try_get::<f64, _>("malformed_resp_prob").unwrap_or(0.0) as f32,
        seed: row.try_get::<i64, _>("seed").ok().map(|x| x as u64),
    })
}

pub(crate) async fn upsert(
    pool: &SqlitePool,
    mailbox_id: &str,
    cfg: &ChaosConfig,
) -> Result<()> {
    sqlx::query(
        r"INSERT INTO chaos_configs
            (mailbox_id, enabled, reject_4xx_prob, reject_5xx_prob,
             delay_ms_min, delay_ms_max, drop_during_data_prob,
             malformed_resp_prob, seed)
          VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?)
          ON CONFLICT(mailbox_id) DO UPDATE SET
            enabled = excluded.enabled,
            reject_4xx_prob = excluded.reject_4xx_prob,
            reject_5xx_prob = excluded.reject_5xx_prob,
            delay_ms_min = excluded.delay_ms_min,
            delay_ms_max = excluded.delay_ms_max,
            drop_during_data_prob = excluded.drop_during_data_prob,
            malformed_resp_prob = excluded.malformed_resp_prob,
            seed = excluded.seed",
    )
    .bind(mailbox_id)
    .bind(i64::from(cfg.enabled))
    .bind(f64::from(cfg.reject_4xx_prob))
    .bind(f64::from(cfg.reject_5xx_prob))
    .bind(i64::from(cfg.delay_ms_min))
    .bind(i64::from(cfg.delay_ms_max))
    .bind(f64::from(cfg.drop_during_data_prob))
    .bind(f64::from(cfg.malformed_resp_prob))
    .bind(cfg.seed.map(|s| s as i64))
    .execute(pool)
    .await?;
    Ok(())
}
