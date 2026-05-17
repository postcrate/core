//! Mailbox flavors.

use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum MailboxKind {
    Primary,
    Shared,
    Ephemeral,
}

impl MailboxKind {
    pub fn as_str(self) -> &'static str {
        match self {
            MailboxKind::Primary => "primary",
            MailboxKind::Shared => "shared",
            MailboxKind::Ephemeral => "ephemeral",
        }
    }

    pub fn from_str(s: &str) -> Option<Self> {
        match s {
            "primary" => Some(MailboxKind::Primary),
            "shared" => Some(MailboxKind::Shared),
            "ephemeral" => Some(MailboxKind::Ephemeral),
            _ => None,
        }
    }
}
