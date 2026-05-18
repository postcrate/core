//! Minimal outbound SMTP client used by `Service::release_email` to
//! forward a captured message to a real address through a real relay
//! (e.g. a developer's transactional provider).
//!
//! Deliberately tiny:
//!   - plain TCP only (no STARTTLS, no AUTH yet);
//!   - dot-stuffs the body on the way out;
//!   - drops the connection on the first error.
//!
//! Per PROD.md §9.3 the "release" action is opt-in and audit-logged;
//! callers are responsible for not pointing it at production relays
//! unintentionally.

use std::time::Duration;

use serde::{Deserialize, Serialize};
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::net::TcpStream;

use crate::error::{Error, Result};

#[derive(Debug, Clone, Serialize, Deserialize)]
#[cfg_attr(feature = "specta", derive(specta::Type))]
#[serde(rename_all = "camelCase")]
pub struct RelayConfig {
    /// Relay host, e.g. `smtp.resend.com` or `127.0.0.1`.
    pub host: String,
    /// Relay port. 25 for legacy, 587 for submission, 1025 for local.
    pub port: u16,
    /// Connect+IO timeout (defaults to 30s).
    #[serde(default)]
    pub timeout_seconds: Option<u32>,
    /// Glob-pattern allowlist of recipient addresses the relay is
    /// allowed to deliver to. `["alice@example.com", "*@test.local"]`.
    /// `None` or empty means "any recipient" (no restriction). Used
    /// by `Service::release_email` to prevent accidentally pointing
    /// the relay at production recipients.
    #[serde(default)]
    pub allowed_recipients: Option<Vec<String>>,
}

impl RelayConfig {
    fn timeout(&self) -> Duration {
        Duration::from_secs(u64::from(self.timeout_seconds.unwrap_or(30).max(1)))
    }
}

/// Forward `raw` to `relay` with the given envelope. The raw bytes
/// are sent unchanged (only dot-stuffed for transport).
///
/// Recipients are filtered against `relay.allowed_recipients` (glob
/// matching) before any network call. Any recipient that fails the
/// allowlist makes the whole release fail — we'd rather error than
/// silently drop a recipient.
pub async fn relay_message(
    relay: &RelayConfig,
    mail_from: &str,
    rcpt_to: &[String],
    raw: &[u8],
) -> Result<()> {
    if rcpt_to.is_empty() {
        return Err(Error::Invalid("release requires at least one recipient".into()));
    }
    if let Some(allow) = relay.allowed_recipients.as_ref().filter(|v| !v.is_empty()) {
        for rcpt in rcpt_to {
            if !allow.iter().any(|pat| glob_match(pat, rcpt)) {
                return Err(Error::Invalid(format!(
                    "recipient {rcpt:?} not in relay allowlist"
                )));
            }
        }
    }
    let to = (relay.host.as_str(), relay.port);
    let stream = tokio::time::timeout(relay.timeout(), TcpStream::connect(to))
        .await
        .map_err(|_| Error::Internal(format!("connect timeout to {}:{}", relay.host, relay.port)))?
        .map_err(|e| Error::Internal(format!("connect {}:{}: {e}", relay.host, relay.port)))?;
    let (r, mut w) = stream.into_split();
    let mut br = BufReader::new(r);

    expect_code(&mut br, "220").await?;
    send_line(&mut w, &format!("EHLO postcrate.local\r\n")).await?;
    drain_multi(&mut br, "250").await?;
    send_line(&mut w, &format!("MAIL FROM:<{}>\r\n", mail_from)).await?;
    expect_code(&mut br, "250").await?;
    for rcpt in rcpt_to {
        send_line(&mut w, &format!("RCPT TO:<{}>\r\n", rcpt)).await?;
        expect_code(&mut br, "250").await?;
    }
    send_line(&mut w, "DATA\r\n").await?;
    expect_code(&mut br, "354").await?;

    let stuffed = dot_stuff(raw);
    w.write_all(&stuffed).await?;
    if !stuffed.ends_with(b"\r\n") {
        w.write_all(b"\r\n").await?;
    }
    w.write_all(b".\r\n").await?;
    w.flush().await?;
    expect_code(&mut br, "250").await?;
    let _ = send_line(&mut w, "QUIT\r\n").await;
    let _ = expect_code(&mut br, "221").await;
    Ok(())
}

async fn send_line<W: AsyncWriteExt + Unpin>(w: &mut W, s: &str) -> Result<()> {
    w.write_all(s.as_bytes()).await?;
    w.flush().await?;
    Ok(())
}

async fn expect_code<R: tokio::io::AsyncRead + Unpin>(
    br: &mut BufReader<R>,
    code: &str,
) -> Result<()> {
    let mut line = String::new();
    br.read_line(&mut line).await?;
    if !line.starts_with(code) {
        return Err(Error::Internal(format!(
            "relay expected {code}, got {}",
            line.trim_end()
        )));
    }
    Ok(())
}

async fn drain_multi<R: tokio::io::AsyncRead + Unpin>(
    br: &mut BufReader<R>,
    code: &str,
) -> Result<()> {
    loop {
        let mut line = String::new();
        let n = br.read_line(&mut line).await?;
        if n == 0 {
            return Err(Error::Internal("relay closed mid-reply".into()));
        }
        if !line.starts_with(code) {
            return Err(Error::Internal(format!(
                "relay expected {code}, got {}",
                line.trim_end()
            )));
        }
        // Final line of a multi-line reply has a space at index 3.
        if line.len() >= 4 && line.as_bytes()[3] == b' ' {
            return Ok(());
        }
    }
}

/// Match `address` against a glob pattern. Same semantics as the
/// bounce-rule matcher: `*` is the only wildcard, comparison is
/// case-insensitive.
fn glob_match(pattern: &str, address: &str) -> bool {
    let p = pattern.to_lowercase();
    let a = address.to_lowercase();
    glob_inner(&p, &a)
}

fn glob_inner(p: &str, a: &str) -> bool {
    let mut pi = p.chars().peekable();
    let mut ai = a.chars().peekable();
    loop {
        match (pi.peek().copied(), ai.peek().copied()) {
            (None, None) => return true,
            (None, Some(_)) => return false,
            (Some('*'), _) => {
                pi.next();
                if pi.peek().is_none() {
                    return true;
                }
                let rest_p: String = pi.clone().collect();
                let mut rest_a: String = ai.clone().collect();
                loop {
                    if glob_inner(&rest_p, &rest_a) {
                        return true;
                    }
                    if rest_a.is_empty() {
                        return false;
                    }
                    rest_a.remove(0);
                }
            }
            (Some(pc), Some(ac)) if pc == ac => {
                pi.next();
                ai.next();
            }
            _ => return false,
        }
    }
}

fn dot_stuff(body: &[u8]) -> Vec<u8> {
    let mut out = Vec::with_capacity(body.len());
    let mut at_line_start = true;
    for &b in body {
        if at_line_start && b == b'.' {
            out.push(b'.');
        }
        out.push(b);
        at_line_start = b == b'\n';
    }
    out
}
