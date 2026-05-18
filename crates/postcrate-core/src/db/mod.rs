//! SQLite-backed storage. Every persistent piece of the engine lives here.
//!
//! The public boundary is one type per concern (`emails`, `attachments`,
//! `mailboxes`, ...). Modules outside `db::` hold `sqlx::SqlitePool` and
//! call free functions, rather than carrying a stateful "repository"
//! struct around.

pub mod attachments;
pub mod audit;
pub mod bounce_rules;
pub mod chaos_configs;
pub mod emails;
pub mod forwarding;
pub mod mailboxes;
pub mod migrate;
pub mod pool;
pub mod settings;
pub mod webhooks;
