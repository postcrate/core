//! # postcrate-core
//!
//! Standalone mail engine: a Tokio-native SMTP capture server with a local
//! HTTP API, multi-mailbox lifecycle, chaos simulation, and SQLite
//! persistence. Has no dependency on Tauri or any UI framework — consumers
//! plug in their own `EventSink` implementation.
//!
//! The public surface is intentionally narrow: one [`Service`] type and a
//! handful of input/output structs. Everything else is `pub(crate)`.

#![forbid(unsafe_code)]
#![warn(missing_debug_implementations)]
#![warn(clippy::pedantic)]
// Several public-API items (TLS scaffolding, reply helpers reserved for
// edge cases, diagnostic fields on listener handles) are dead from the
// compiler's POV because the consumers live in downstream repos. The
// lint isn't useful for us until the API stabilizes.
#![allow(dead_code)]
#![allow(clippy::module_name_repetitions)]
#![allow(clippy::missing_errors_doc)]
#![allow(clippy::missing_panics_doc)]
#![allow(clippy::must_use_candidate)]
#![allow(clippy::cast_possible_truncation)]
#![allow(clippy::cast_sign_loss)]
#![allow(clippy::cast_precision_loss)]
#![allow(clippy::similar_names)]
#![allow(clippy::too_many_lines)]

pub mod config;
pub mod error;
pub mod events;
pub mod service;

pub(crate) mod db;
pub(crate) mod http;
pub(crate) mod mail;
pub(crate) mod mailbox;
pub(crate) mod pipeline;
pub(crate) mod smtp;

pub use crate::config::{BindHost, CoreConfig};

/// Stable, panic-safe wrappers around internal parsers used by the
/// cargo-fuzz targets under `fuzz/`. Not intended for general use;
/// the public surface for normal callers is the [`Service`] type.
#[doc(hidden)]
pub mod fuzz {
    /// Run the SMTP command-line parser. Returns `Ok` or `Err`; either
    /// way, the parser must not panic.
    pub fn parse_smtp_command(input: &str) -> std::result::Result<(), String> {
        crate::smtp::command::SmtpCommand::parse(input)
            .map(|_| ())
            .map_err(|e| e.to_string())
    }

    /// Run the MIME parser. Must not panic on arbitrary input.
    pub fn parse_mail(bytes: &[u8]) {
        let _ = crate::mail::parse::parse(bytes);
    }

    /// Run the SMTP path parser (the inside of `MAIL FROM:<...>`).
    pub fn parse_smtp_path(input: &str) -> std::result::Result<(), String> {
        crate::mail::address::parse_path(input)
            .map(|_| ())
            .map_err(|e| e.to_string())
    }
}
pub use crate::db::audit::AuditEntry;
pub use crate::db::bounce_rules::BounceRule;
pub use crate::db::chaos_configs::ChaosConfig;
pub use crate::db::emails::{AttachmentMeta, EmailDetail, EmailSummary};
pub use crate::db::mailboxes::{
    CreateEphemeralInput, CreateMailboxInput, EphemeralHandle, Mailbox, UpdateMailboxInput,
};
pub use crate::db::settings::{
    AdvancedPrefs, AgentPrefs, BackendSettings, InboxPrefs, NetworkPrefs, SettingsPatch,
    SettingsSection,
};
pub use crate::error::{Error, Result};
pub use crate::events::{
    BounceKind, ChannelSink, CoreEvent, EventSink, LogSink, MailboxStateChange, ServerStatus,
};
pub use crate::mailbox::kinds::MailboxKind;
pub use crate::service::Service;
