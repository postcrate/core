//! Typed setting sections persisted in the `settings` table.
//!
//! The keys mirror the existing TypeScript zustand store sections
//! (`network`, `agents`, `inbox`, `advanced`) so a downstream UI can sync
//! to backend defaults.

use serde::{Deserialize, Serialize};
use sqlx::{Row, SqlitePool};

use crate::error::Result;

#[derive(Debug, Clone, Copy, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum SettingsSection {
    Network,
    Agents,
    Inbox,
    Advanced,
}

impl SettingsSection {
    fn as_str(self) -> &'static str {
        match self {
            SettingsSection::Network => "network",
            SettingsSection::Agents => "agents",
            SettingsSection::Inbox => "inbox",
            SettingsSection::Advanced => "advanced",
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct NetworkPrefs {
    pub smtp_port: u16,
    pub http_api_port: u16,
    pub mcp_enabled: bool,
    pub mcp_port: u16,
    pub expose_on_lan: bool,
    /// Serve the HTTP API over HTTPS, reusing the cert/key configured
    /// for STARTTLS. Requires `--features tls` and a valid cert.
    #[serde(default)]
    pub api_tls: bool,
    /// When set, every `/api/v1/...` request must carry
    /// `Authorization: Bearer <token>`. The healthz endpoint is
    /// always open so liveness probes still work.
    #[serde(default)]
    pub api_auth_token: Option<String>,
}

impl Default for NetworkPrefs {
    fn default() -> Self {
        Self {
            smtp_port: 1025,
            http_api_port: 1080,
            mcp_enabled: true,
            mcp_port: 1081,
            expose_on_lan: false,
            api_tls: false,
            api_auth_token: None,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AgentPrefs {
    pub default_wait_timeout_seconds: u32,
    pub log_agent_requests: bool,
    pub confirm_destructive_actions: bool,
}

impl Default for AgentPrefs {
    fn default() -> Self {
        Self {
            default_wait_timeout_seconds: 30,
            log_agent_requests: true,
            confirm_destructive_actions: true,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct InboxPrefs {
    pub max_retained_emails: u32,
    pub auto_clear_after_days: u32,
    pub thread_related: bool,
    pub auto_tag: bool,
}

impl Default for InboxPrefs {
    fn default() -> Self {
        Self {
            max_retained_emails: 5000,
            auto_clear_after_days: 14,
            thread_related: true,
            auto_tag: true,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AdvancedPrefs {
    pub debug_logging: bool,
    pub preserve_smtp_transcript: bool,
    pub audit_retain_days: u32,
}

impl Default for AdvancedPrefs {
    fn default() -> Self {
        Self {
            debug_logging: false,
            preserve_smtp_transcript: true,
            audit_retain_days: 90,
        }
    }
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct BackendSettings {
    pub network: NetworkPrefs,
    pub agents: AgentPrefs,
    pub inbox: InboxPrefs,
    pub advanced: AdvancedPrefs,
}

/// One-of patch — exactly one section is set per call.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", tag = "section", content = "value")]
pub enum SettingsPatch {
    Network(NetworkPrefs),
    Agents(AgentPrefs),
    Inbox(InboxPrefs),
    Advanced(AdvancedPrefs),
}

impl SettingsPatch {
    pub(crate) fn section(&self) -> SettingsSection {
        match self {
            SettingsPatch::Network(_) => SettingsSection::Network,
            SettingsPatch::Agents(_) => SettingsSection::Agents,
            SettingsPatch::Inbox(_) => SettingsSection::Inbox,
            SettingsPatch::Advanced(_) => SettingsSection::Advanced,
        }
    }
}

pub(crate) async fn get_section_raw(
    pool: &SqlitePool,
    section: SettingsSection,
) -> Result<serde_json::Value> {
    let rows = sqlx::query("SELECT key, value FROM settings WHERE section = ?")
        .bind(section.as_str())
        .fetch_all(pool)
        .await?;
    let mut map = serde_json::Map::new();
    for r in rows {
        let k: String = r.try_get("key").unwrap_or_default();
        let v: String = r.try_get("value").unwrap_or_default();
        let parsed: serde_json::Value =
            serde_json::from_str(&v).unwrap_or(serde_json::Value::String(v));
        map.insert(k, parsed);
    }
    Ok(serde_json::Value::Object(map))
}

pub(crate) async fn save_section(
    pool: &SqlitePool,
    section: SettingsSection,
    value: serde_json::Value,
) -> Result<()> {
    let serde_json::Value::Object(map) = value else {
        // Coerce a non-object payload into a single `_value` slot.
        let mut tx = pool.begin().await?;
        sqlx::query("DELETE FROM settings WHERE section = ?")
            .bind(section.as_str())
            .execute(&mut *tx)
            .await?;
        sqlx::query(
            r"INSERT INTO settings (section, key, value)
              VALUES (?, ?, ?)",
        )
        .bind(section.as_str())
        .bind("_value")
        .bind(serde_json::to_string(&value).unwrap_or_else(|_| "null".into()))
        .execute(&mut *tx)
        .await?;
        tx.commit().await?;
        return Ok(());
    };

    let mut tx = pool.begin().await?;
    sqlx::query("DELETE FROM settings WHERE section = ?")
        .bind(section.as_str())
        .execute(&mut *tx)
        .await?;
    for (k, v) in map {
        sqlx::query(
            r"INSERT INTO settings (section, key, value)
              VALUES (?, ?, ?)",
        )
        .bind(section.as_str())
        .bind(&k)
        .bind(serde_json::to_string(&v).unwrap_or_else(|_| "null".into()))
        .execute(&mut *tx)
        .await?;
    }
    tx.commit().await?;
    Ok(())
}

pub(crate) async fn load_all(pool: &SqlitePool) -> Result<BackendSettings> {
    let network = match get_section_raw(pool, SettingsSection::Network).await? {
        serde_json::Value::Object(m) if !m.is_empty() => {
            serde_json::from_value(serde_json::Value::Object(m)).unwrap_or_default()
        }
        _ => NetworkPrefs::default(),
    };
    let agents = match get_section_raw(pool, SettingsSection::Agents).await? {
        serde_json::Value::Object(m) if !m.is_empty() => {
            serde_json::from_value(serde_json::Value::Object(m)).unwrap_or_default()
        }
        _ => AgentPrefs::default(),
    };
    let inbox = match get_section_raw(pool, SettingsSection::Inbox).await? {
        serde_json::Value::Object(m) if !m.is_empty() => {
            serde_json::from_value(serde_json::Value::Object(m)).unwrap_or_default()
        }
        _ => InboxPrefs::default(),
    };
    let advanced = match get_section_raw(pool, SettingsSection::Advanced).await? {
        serde_json::Value::Object(m) if !m.is_empty() => {
            serde_json::from_value(serde_json::Value::Object(m)).unwrap_or_default()
        }
        _ => AdvancedPrefs::default(),
    };
    Ok(BackendSettings {
        network,
        agents,
        inbox,
        advanced,
    })
}

pub(crate) async fn apply_patch(pool: &SqlitePool, patch: &SettingsPatch) -> Result<()> {
    match patch {
        SettingsPatch::Network(v) => {
            save_section(pool, SettingsSection::Network, serde_json::to_value(v)?).await
        }
        SettingsPatch::Agents(v) => {
            save_section(pool, SettingsSection::Agents, serde_json::to_value(v)?).await
        }
        SettingsPatch::Inbox(v) => {
            save_section(pool, SettingsSection::Inbox, serde_json::to_value(v)?).await
        }
        SettingsPatch::Advanced(v) => {
            save_section(pool, SettingsSection::Advanced, serde_json::to_value(v)?).await
        }
    }
}
