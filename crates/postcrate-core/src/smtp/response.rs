//! SMTP reply construction. Each variant maps to a numeric reply code +
//! one or more text lines. Multi-line replies use the `code-text` /
//! `code text` continuation form from RFC 5321 §4.2.1.

use std::borrow::Cow;

use tokio::io::{AsyncWrite, AsyncWriteExt};

use crate::error::Result;

#[derive(Debug, Clone)]
pub struct SmtpReply {
    pub code: u16,
    pub lines: Vec<Cow<'static, str>>,
}

impl SmtpReply {
    pub fn new(code: u16, line: impl Into<Cow<'static, str>>) -> Self {
        Self {
            code,
            lines: vec![line.into()],
        }
    }

    pub fn multi(code: u16, lines: Vec<Cow<'static, str>>) -> Self {
        Self { code, lines }
    }

    pub fn greet(host: &str) -> Self {
        SmtpReply::new(220, format!("{host} ESMTP Postcrate ready"))
    }

    pub fn ok() -> Self {
        SmtpReply::new(250, "OK")
    }

    pub fn ok_msg(msg: impl Into<Cow<'static, str>>) -> Self {
        SmtpReply::new(250, msg)
    }

    pub fn bye() -> Self {
        SmtpReply::new(221, "Bye")
    }

    pub fn start_mail_input() -> Self {
        SmtpReply::new(354, "End data with <CR><LF>.<CR><LF>")
    }

    pub fn bad_sequence() -> Self {
        SmtpReply::new(503, "Bad sequence of commands")
    }

    pub fn syntax_error() -> Self {
        SmtpReply::new(500, "Syntax error, command unrecognized")
    }

    pub fn command_not_implemented() -> Self {
        SmtpReply::new(502, "Command not implemented")
    }

    pub fn line_too_long() -> Self {
        SmtpReply::new(500, "Line too long")
    }

    pub fn transaction_failed() -> Self {
        SmtpReply::new(554, "Transaction failed")
    }

    pub fn size_exceeded() -> Self {
        SmtpReply::new(552, "Message size exceeds fixed maximum")
    }

    pub fn vrfy_cannot() -> Self {
        SmtpReply::new(252, "Cannot VRFY user; try RCPT")
    }

    pub fn help_lines() -> Self {
        SmtpReply::multi(
            214,
            vec![
                "Postcrate supports the following commands:".into(),
                "HELO EHLO MAIL RCPT DATA RSET NOOP QUIT VRFY HELP".into(),
            ],
        )
    }

    pub fn custom(code: u16, msg: impl Into<Cow<'static, str>>) -> Self {
        SmtpReply::new(code, msg)
    }
}

/// Writer wrapper that serializes a `SmtpReply` to the wire.
pub struct ReplyWriter<W> {
    inner: W,
}

impl<W: AsyncWrite + Unpin> ReplyWriter<W> {
    pub fn new(inner: W) -> Self {
        Self { inner }
    }

    pub fn into_inner(self) -> W {
        self.inner
    }

    pub async fn send(&mut self, reply: &SmtpReply) -> Result<()> {
        let n = reply.lines.len();
        if n == 0 {
            // Defensive — never write nothing.
            let line = format!("{} \r\n", reply.code);
            self.inner.write_all(line.as_bytes()).await?;
            self.inner.flush().await?;
            return Ok(());
        }
        for (i, l) in reply.lines.iter().enumerate() {
            let sep = if i + 1 == n { ' ' } else { '-' };
            let line = format!("{}{sep}{}\r\n", reply.code, l);
            self.inner.write_all(line.as_bytes()).await?;
        }
        self.inner.flush().await?;
        Ok(())
    }

    /// Write a literal byte sequence — used by chaos `malformed` injection.
    pub async fn send_raw(&mut self, bytes: &[u8]) -> Result<()> {
        self.inner.write_all(bytes).await?;
        self.inner.flush().await?;
        Ok(())
    }
}
