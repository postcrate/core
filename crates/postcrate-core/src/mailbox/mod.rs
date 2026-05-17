//! Mailbox lifecycle. Each mailbox owns one TCP listener bound to its
//! own port; the [`MailboxService`] coordinates creation, deletion, and
//! TTL expiration.

pub mod kinds;
pub mod lifecycle;
pub mod ports;
pub mod service;
