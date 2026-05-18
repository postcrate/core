//! EHLO advertisement builder. The supported extensions are fixed, but
//! max-size and the optional `STARTTLS` / `AUTH` keywords flip per
//! config so a single builder covers every mailbox flavour.

use std::borrow::Cow;

use crate::smtp::response::SmtpReply;

#[derive(Debug, Clone)]
pub struct EhloAdvert {
    pub hostname: String,
    pub max_size: u64,
    pub starttls_enabled: bool,
    /// When true, advertise `AUTH PLAIN LOGIN`. The session accepts
    /// any credentials — AUTH is advertised for client compatibility,
    /// not for access control in a local capture server.
    pub auth_enabled: bool,
}

impl EhloAdvert {
    pub fn reply(&self, client_helo: &str) -> SmtpReply {
        let mut lines: Vec<Cow<'static, str>> = Vec::with_capacity(8);
        lines.push(Cow::Owned(format!(
            "{} greets {}",
            self.hostname,
            if client_helo.is_empty() { "client" } else { client_helo }
        )));
        lines.push(Cow::Borrowed("PIPELINING"));
        lines.push(Cow::Owned(format!("SIZE {}", self.max_size)));
        lines.push(Cow::Borrowed("8BITMIME"));
        lines.push(Cow::Borrowed("SMTPUTF8"));
        lines.push(Cow::Borrowed("ENHANCEDSTATUSCODES"));
        if self.starttls_enabled {
            lines.push(Cow::Borrowed("STARTTLS"));
        }
        if self.auth_enabled {
            lines.push(Cow::Borrowed("AUTH PLAIN LOGIN"));
        }
        lines.push(Cow::Borrowed("HELP"));
        SmtpReply::multi(250, lines)
    }
}
