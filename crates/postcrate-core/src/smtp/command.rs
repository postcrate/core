//! SMTP command parser. Strict enough to match RFC 5321 §4.1.1 but
//! generous about extra whitespace and unusual casing, because real
//! clients send unusual things.

use crate::error::{Error, Result};
use crate::mail::address::{parse_path, ParsedPath};

#[derive(Debug, Clone)]
pub enum SmtpCommand {
    Helo(String),
    Ehlo(String),
    MailFrom(ParsedPath),
    RcptTo(ParsedPath),
    Data,
    Rset,
    Noop,
    Quit,
    Vrfy(String),
    Help(Option<String>),
    /// `STARTTLS` — recognized but currently returns `502` (feature-gated).
    StartTls,
    /// Recognized but unimplemented (e.g. AUTH); we'll reply `502`.
    Unknown(String),
}

impl SmtpCommand {
    pub fn parse(line: &str) -> Result<Self> {
        let trimmed = line.trim_end_matches(['\r', '\n']);
        if trimmed.is_empty() {
            return Err(Error::SmtpProto("empty command".into()));
        }

        // The first whitespace splits keyword from arguments. SMTP
        // permits a `:` directly after MAIL/RCPT (MAIL FROM:<...>),
        // so we have to split carefully.
        let (kw_upper, rest) = split_keyword(trimmed);

        match kw_upper.as_str() {
            "HELO" => Ok(SmtpCommand::Helo(rest.trim().to_string())),
            "EHLO" => Ok(SmtpCommand::Ehlo(rest.trim().to_string())),
            "MAIL" => {
                let path = strip_keyword_colon(rest, "FROM")?;
                Ok(SmtpCommand::MailFrom(parse_path(path)?))
            }
            "RCPT" => {
                let path = strip_keyword_colon(rest, "TO")?;
                Ok(SmtpCommand::RcptTo(parse_path(path)?))
            }
            "DATA" => Ok(SmtpCommand::Data),
            "RSET" => Ok(SmtpCommand::Rset),
            "NOOP" => Ok(SmtpCommand::Noop),
            "QUIT" => Ok(SmtpCommand::Quit),
            "VRFY" => Ok(SmtpCommand::Vrfy(rest.trim().to_string())),
            "HELP" => {
                let r = rest.trim();
                Ok(SmtpCommand::Help(if r.is_empty() {
                    None
                } else {
                    Some(r.to_string())
                }))
            }
            "STARTTLS" => Ok(SmtpCommand::StartTls),
            other => Ok(SmtpCommand::Unknown(other.to_string())),
        }
    }
}

fn split_keyword(s: &str) -> (String, &str) {
    let mut iter = s.char_indices();
    let mut end = s.len();
    for (i, c) in &mut iter {
        if c.is_whitespace() || c == ':' {
            end = i;
            break;
        }
    }
    let kw = s[..end].to_ascii_uppercase();
    let rest = &s[end..];
    (kw, rest)
}

/// For MAIL/RCPT, the format is `MAIL FROM:<...>` or `MAIL FROM: <...>`.
/// `rest` here begins at the character after the keyword. We need to
/// also accept `MAIL  FROM:<...>` (double space) and `MAIL FROM :<...>`.
fn strip_keyword_colon<'a>(rest: &'a str, expected: &'static str) -> Result<&'a str> {
    let trimmed = rest.trim_start();
    let upper = trimmed.to_ascii_uppercase();
    if !upper.starts_with(expected) {
        return Err(Error::SmtpProto(format!("expected '{expected}:'")));
    }
    let after_kw = &trimmed[expected.len()..];
    let after_kw = after_kw.trim_start();
    let after_kw = after_kw
        .strip_prefix(':')
        .ok_or_else(|| Error::SmtpProto(format!("expected ':' after {expected}")))?;
    Ok(after_kw)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn parse(s: &str) -> SmtpCommand {
        SmtpCommand::parse(s).expect("parse")
    }

    #[test]
    fn helo_ehlo() {
        assert!(matches!(parse("HELO host"), SmtpCommand::Helo(_)));
        assert!(matches!(parse("ehlo host"), SmtpCommand::Ehlo(_)));
    }

    #[test]
    fn mail_from_variants() {
        assert!(matches!(parse("MAIL FROM:<a@b>"), SmtpCommand::MailFrom(_)));
        assert!(matches!(parse("MAIL FROM: <a@b>"), SmtpCommand::MailFrom(_)));
        assert!(matches!(parse("mail  from:<>"), SmtpCommand::MailFrom(_)));
    }

    #[test]
    fn rcpt_to() {
        let c = parse("RCPT TO:<x@y> NOTIFY=NEVER");
        assert!(matches!(c, SmtpCommand::RcptTo(_)));
    }

    #[test]
    fn singletons() {
        for s in ["DATA", "RSET", "NOOP", "QUIT", "STARTTLS"] {
            let _ = parse(s);
        }
    }

    #[test]
    fn vrfy_help() {
        assert!(matches!(parse("VRFY postmaster"), SmtpCommand::Vrfy(_)));
        assert!(matches!(parse("HELP MAIL"), SmtpCommand::Help(Some(_))));
        assert!(matches!(parse("HELP"), SmtpCommand::Help(None)));
    }

    #[test]
    fn unknown() {
        assert!(matches!(parse("FOO bar"), SmtpCommand::Unknown(_)));
    }
}
