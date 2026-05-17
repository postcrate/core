//! App-wide error type. Every public method on [`crate::Service`] returns
//! `Result<_, Error>`. Boundary adapters (Axum, Tauri shims) convert this
//! into HTTP status codes or `String`s.

use std::io;

use thiserror::Error;

pub type Result<T> = std::result::Result<T, Error>;

/// Every fallible operation in `postcrate-core` returns this.
#[derive(Debug, Error)]
pub enum Error {
    #[error("database error: {0}")]
    Db(#[from] sqlx::Error),

    #[error("database migration error: {0}")]
    Migrate(#[from] sqlx::migrate::MigrateError),

    #[error("io error: {0}")]
    Io(#[from] io::Error),

    #[error("json error: {0}")]
    Json(#[from] serde_json::Error),

    #[error("parse error: {0}")]
    Parse(String),

    #[error("mailbox not found: {0}")]
    MailboxNotFound(String),

    #[error("email not found: {0}")]
    EmailNotFound(String),

    #[error("attachment not found: {0}")]
    AttachmentNotFound(String),

    #[error("bounce rule not found: {0}")]
    BounceRuleNotFound(String),

    #[error("port {0} is already in use")]
    PortInUse(u16),

    #[error("port {0} not in allowed range")]
    PortOutOfRange(u16),

    #[error("ephemeral port range exhausted")]
    PortRangeExhausted,

    #[error("mailbox name '{0}' already exists in project")]
    DuplicateMailbox(String),

    #[error("smtp protocol error: {0}")]
    SmtpProto(String),

    #[error("invalid input: {0}")]
    Invalid(String),

    #[error("not implemented: {0}")]
    NotImplemented(&'static str),

    #[error("internal: {0}")]
    Internal(String),
}

impl Error {
    /// HTTP status mapping for the Axum layer.
    pub fn http_status(&self) -> http::StatusCode {
        use http::StatusCode as S;
        match self {
            Error::MailboxNotFound(_)
            | Error::EmailNotFound(_)
            | Error::AttachmentNotFound(_)
            | Error::BounceRuleNotFound(_) => S::NOT_FOUND,
            Error::DuplicateMailbox(_) | Error::PortInUse(_) => S::CONFLICT,
            Error::Invalid(_) | Error::Parse(_) | Error::PortOutOfRange(_) => S::BAD_REQUEST,
            Error::NotImplemented(_) => S::NOT_IMPLEMENTED,
            Error::PortRangeExhausted => S::SERVICE_UNAVAILABLE,
            _ => S::INTERNAL_SERVER_ERROR,
        }
    }

    /// Short machine-readable code for the `{error, message}` JSON body.
    pub fn code(&self) -> &'static str {
        match self {
            Error::Db(_) | Error::Migrate(_) => "db_error",
            Error::Io(_) => "io_error",
            Error::Json(_) => "json_error",
            Error::Parse(_) => "parse_error",
            Error::MailboxNotFound(_) => "mailbox_not_found",
            Error::EmailNotFound(_) => "email_not_found",
            Error::AttachmentNotFound(_) => "attachment_not_found",
            Error::BounceRuleNotFound(_) => "bounce_rule_not_found",
            Error::PortInUse(_) => "port_in_use",
            Error::PortOutOfRange(_) => "port_out_of_range",
            Error::PortRangeExhausted => "port_range_exhausted",
            Error::DuplicateMailbox(_) => "duplicate_mailbox",
            Error::SmtpProto(_) => "smtp_proto",
            Error::Invalid(_) => "invalid_input",
            Error::NotImplemented(_) => "not_implemented",
            Error::Internal(_) => "internal",
        }
    }
}
