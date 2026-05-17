//! Minimal RFC 5321 path parser. We accept what real SMTP clients send,
//! not what the grammar says they should — Mailpit and Postfix are the
//! reference baseline.
//!
//! Examples we have to handle:
//!   - `<a@b.com>`
//!   - `<>`                     (the null sender)
//!   - `<a@b.com> SIZE=12345 BODY=8BITMIME SMTPUTF8`
//!   - `a@b.com`                (no angle brackets — some clients omit them)
//!   - `<"odd name"@host>`      (quoted local part)

use crate::error::{Error, Result};

#[derive(Debug, Clone, Default)]
pub struct PathExtensions {
    pub size: Option<u64>,
    pub body_8bitmime: bool,
    pub smtputf8: bool,
    /// Extension params we didn't recognize but should round-trip.
    pub extras: Vec<(String, Option<String>)>,
}

#[derive(Debug, Clone)]
pub struct ParsedPath {
    /// The mailbox part (`a@b.com`). May be empty for `<>` (null sender).
    pub mailbox: String,
    pub extensions: PathExtensions,
}

/// Parse the tail after `MAIL FROM:` or `RCPT TO:`. The caller strips the
/// keyword + colon.
pub fn parse_path(input: &str) -> Result<ParsedPath> {
    let s = input.trim_start();

    let (mailbox, rest) = if let Some(stripped) = s.strip_prefix('<') {
        // bracketed form
        let end = find_closing(stripped)
            .ok_or_else(|| Error::SmtpProto("unterminated <...> in path".into()))?;
        let inside = &stripped[..end];
        let rest = &stripped[end + 1..];
        (inside.to_string(), rest.trim())
    } else {
        // bare address form — take until whitespace
        let (m, r) = match s.find(char::is_whitespace) {
            Some(i) => (&s[..i], &s[i..]),
            None => (s, ""),
        };
        (m.to_string(), r.trim())
    };

    let extensions = parse_extensions(rest);
    Ok(ParsedPath { mailbox, extensions })
}

fn find_closing(s: &str) -> Option<usize> {
    // Allow quoted-string local part inside <...>; only count `>` outside quotes.
    let mut in_quote = false;
    let mut prev_escape = false;
    for (i, c) in s.char_indices() {
        match c {
            '\\' if in_quote => {
                prev_escape = !prev_escape;
                continue;
            }
            '"' if !prev_escape => in_quote = !in_quote,
            '>' if !in_quote => return Some(i),
            _ => {}
        }
        prev_escape = false;
    }
    None
}

fn parse_extensions(tail: &str) -> PathExtensions {
    let mut ext = PathExtensions::default();
    for part in tail.split_ascii_whitespace() {
        if part.is_empty() {
            continue;
        }
        let (k, v) = match part.find('=') {
            Some(i) => (&part[..i], Some(&part[i + 1..])),
            None => (part, None),
        };
        let ku = k.to_ascii_uppercase();
        match (ku.as_str(), v) {
            ("SIZE", Some(num)) => {
                ext.size = num.parse().ok();
            }
            ("BODY", Some(v)) if v.eq_ignore_ascii_case("8BITMIME") => {
                ext.body_8bitmime = true;
            }
            ("SMTPUTF8", _) => {
                ext.smtputf8 = true;
            }
            (other, val) => {
                ext.extras
                    .push((other.to_string(), val.map(|s| s.to_string())));
            }
        }
    }
    ext
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn bracketed() {
        let p = parse_path(" <a@b.com>").unwrap();
        assert_eq!(p.mailbox, "a@b.com");
    }

    #[test]
    fn null_sender() {
        let p = parse_path("<>").unwrap();
        assert_eq!(p.mailbox, "");
    }

    #[test]
    fn bare() {
        let p = parse_path("a@b.com SIZE=12 BODY=8BITMIME SMTPUTF8 X-NEW=foo").unwrap();
        assert_eq!(p.mailbox, "a@b.com");
        assert_eq!(p.extensions.size, Some(12));
        assert!(p.extensions.body_8bitmime);
        assert!(p.extensions.smtputf8);
        assert_eq!(p.extensions.extras[0].0, "X-NEW");
    }

    #[test]
    fn quoted_local_part() {
        let p = parse_path("<\"strange@name\"@host>").unwrap();
        assert_eq!(p.mailbox, "\"strange@name\"@host");
    }
}
