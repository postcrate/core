//! SMTP wire protocol implementation.
//!
//! The split keeps each concern in its own file: command parsing, reply
//! writing, EHLO extension advertising, dot-stuffed DATA reader, the
//! session state machine, the accept loop, and the chaos/bounce hooks.

pub mod bounce;
pub mod chaos;
pub mod codec;
pub mod command;
pub mod data_reader;
pub mod extensions;
pub mod listener;
pub mod relay;
pub mod response;
pub mod session;
pub mod tls;
