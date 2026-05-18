//! UI-agnostic event emission. Consumers — desktop shells, CLI tail
//! commands, integration tests — implement [`EventSink`] and pass it
//! to [`crate::Service::build`]. The engine never touches a UI
//! framework directly.

use std::sync::Arc;

use serde::Serialize;
use tokio::sync::broadcast;

use crate::db::audit::AuditEntry;
use crate::db::emails::EmailSummary;
use crate::db::settings::SettingsSection;

/// Implementors receive all engine-level events.
pub trait EventSink: Send + Sync + 'static {
    fn emit(&self, event: CoreEvent);
}

/// Engine event surface. Adding a variant is a semver-minor change —
/// consumers using `Sink: EventSink` only have to handle what they know.
#[derive(Debug, Clone, Serialize)]
#[serde(tag = "kind", rename_all = "camelCase")]
pub enum CoreEvent {
    NewEmail {
        mailbox_id: String,
        email: EmailSummary,
    },
    MailboxStateChanged {
        mailbox_id: String,
        change: MailboxStateChange,
    },
    ServerStatusChanged {
        status: ServerStatus,
    },
    SettingsChanged {
        section: SettingsSection,
    },
    AuditAppended {
        entry: AuditEntry,
    },
}

#[derive(Debug, Clone, Serialize)]
#[serde(tag = "kind", rename_all = "camelCase")]
pub enum MailboxStateChange {
    Created,
    Updated,
    Deleted,
    Started,
    Stopped,
    Expired,
    Failed { error: String },
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ServerStatus {
    pub running_mailboxes: u32,
    pub http_running: bool,
    pub errors: Vec<String>,
}

#[derive(Debug, Clone, Copy, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum BounceKind {
    Hard,
    Soft,
}

impl BounceKind {
    pub fn as_str(self) -> &'static str {
        match self {
            BounceKind::Hard => "hard",
            BounceKind::Soft => "soft",
        }
    }
    pub fn from_str(s: &str) -> Self {
        if s.eq_ignore_ascii_case("soft") {
            BounceKind::Soft
        } else {
            BounceKind::Hard
        }
    }
}

// We can't derive Deserialize for BounceKind in this position because
// the Serialize block above already lives in `events.rs`. Instead, do
// it explicitly so the wire format (`"hard"` / `"soft"`) is preserved.
impl<'de> serde::Deserialize<'de> for BounceKind {
    fn deserialize<D>(de: D) -> std::result::Result<Self, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        let s = String::deserialize(de)?;
        Ok(Self::from_str(&s))
    }
}

// ---- built-in sinks -------------------------------------------------

/// Trace each event through `tracing::info!`. Handy default for the
/// headless `postcrate` binary and for tests that just want logs.
#[derive(Debug, Default, Clone, Copy)]
pub struct LogSink;

impl EventSink for LogSink {
    fn emit(&self, event: CoreEvent) {
        tracing::info!(target: "postcrate::event", event = ?event);
    }
}

/// Fan out via a `tokio::sync::broadcast` so multiple subscribers
/// (CLI `tail`, tests) can observe the same stream.
#[derive(Debug, Clone)]
pub struct ChannelSink {
    tx: broadcast::Sender<CoreEvent>,
}

impl ChannelSink {
    pub fn new(capacity: usize) -> Self {
        Self {
            tx: broadcast::channel(capacity).0,
        }
    }
    pub fn subscribe(&self) -> broadcast::Receiver<CoreEvent> {
        self.tx.subscribe()
    }
}

impl EventSink for ChannelSink {
    fn emit(&self, event: CoreEvent) {
        let _ = self.tx.send(event);
    }
}

/// Trivial fan-out wrapper so an embedder can plug in more than one
/// sink without writing a custom struct.
#[derive(Clone)]
pub struct ComposedSink {
    sinks: Vec<Arc<dyn EventSink>>,
}

impl std::fmt::Debug for ComposedSink {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ComposedSink")
            .field("len", &self.sinks.len())
            .finish()
    }
}

impl ComposedSink {
    pub fn new(sinks: Vec<Arc<dyn EventSink>>) -> Self {
        Self { sinks }
    }
}

impl EventSink for ComposedSink {
    fn emit(&self, event: CoreEvent) {
        for s in &self.sinks {
            s.emit(event.clone());
        }
    }
}
