//! `.postcrate` recording format (FR-TEST-30/31/32).
//!
//! A recording captures a sequence of received emails — their
//! envelopes plus base64-encoded raw bytes — into a single JSON
//! document. Two intended uses:
//!
//!   - **Test fixtures.** Load a `.postcrate` file in a test setup,
//!     replay it into an ephemeral mailbox, and run the test against
//!     a known inbox state.
//!   - **Reproduction.** Share captured traffic with a teammate or
//!     attach it to a bug report.
//!
//! The format is deliberately plain JSON with no external blobs so
//! the file is self-contained. Versioned via `version: 1` so future
//! additions (e.g. inline attachments) can be parsed by old readers.

use base64::engine::general_purpose::STANDARD as B64;
use base64::Engine;
use serde::{Deserialize, Serialize};

use crate::error::{Error, Result};

pub const RECORDING_VERSION: u32 = 1;

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Recording {
    pub version: u32,
    pub exported_at: i64,
    /// Optional human-readable label so a user can tell two files apart.
    #[serde(default)]
    pub label: Option<String>,
    pub messages: Vec<RecordedMessage>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct RecordedMessage {
    pub envelope: RecordedEnvelope,
    /// Base64-encoded RFC 5322 bytes (what the SMTP DATA phase
    /// produced before parsing). Storing raw keeps replay 1:1.
    pub raw_b64: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct RecordedEnvelope {
    pub mail_from: String,
    pub rcpt_to: Vec<String>,
    pub received_at: i64,
    pub ext_smtputf8: bool,
    pub ext_8bitmime: bool,
    /// Recorded for diagnostics; not used by replay (subject is in `raw`).
    pub subject: Option<String>,
}

impl Recording {
    pub fn new(label: Option<String>) -> Self {
        Self {
            version: RECORDING_VERSION,
            exported_at: chrono::Utc::now().timestamp_millis(),
            label,
            messages: Vec::new(),
        }
    }

    /// Validate the recording is something we can replay. Currently
    /// just checks the version is one we know.
    pub fn validate(&self) -> Result<()> {
        if self.version != RECORDING_VERSION {
            return Err(Error::Invalid(format!(
                "unsupported recording version {} (expected {})",
                self.version, RECORDING_VERSION
            )));
        }
        Ok(())
    }
}

/// Decode a message's raw bytes from its base64 payload.
pub fn decode_raw(msg: &RecordedMessage) -> Result<Vec<u8>> {
    B64.decode(&msg.raw_b64)
        .map_err(|e| Error::Invalid(format!("base64 decode: {e}")))
}

/// Encode raw bytes into a `RecordedMessage`'s base64 payload.
pub fn encode_raw(raw: &[u8]) -> String {
    B64.encode(raw)
}
