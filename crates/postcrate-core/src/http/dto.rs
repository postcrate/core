//! Wire-format request/response shapes. Kept separate from the storage
//! types so we can evolve the API without touching the schema layer.

use serde::{Deserialize, Serialize};

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ListMessagesQuery {
    pub mailbox_id: Option<String>,
    pub limit: Option<u32>,
    pub offset: Option<u32>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ListMailboxesQuery {
    pub project_id: Option<String>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SearchBody {
    pub q: String,
    pub mailbox_id: Option<String>,
    pub limit: Option<u32>,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct InfoResponse {
    pub version: &'static str,
    pub uptime_sec: i64,
    pub running_mailboxes: u32,
    pub bind_host: String,
    pub http_port: u16,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct DeletedCount {
    pub deleted: u64,
}
