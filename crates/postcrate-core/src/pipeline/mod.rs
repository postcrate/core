//! The one place that turns a captured SMTP envelope into a persisted
//! email row, and the retention worker that prunes old data.

pub mod forwarding;
pub mod ingest;
pub mod retention;
pub mod webhooks;
