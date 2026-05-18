//! Auto-tagging.
//!
//! Classifies each captured email as one of:
//!   - `transactional_auth`     — password reset, sign-in code, 2FA, account verify
//!   - `transactional_billing`  — invoice, payment, receipt, refund
//!   - `transactional_notification` — order shipped, comment reply, calendar invite
//!   - `marketing`              — newsletter, promo, sale, unsubscribe in body
//!   - `system`                 — bounce, autoresponder, MTA delivery report
//!   - `unknown`                — none of the above matched strongly enough
//!
//! Strictly local: header + subject + body heuristics, no LLM, no
//! network. The scoring is intentionally simple — we err toward
//! `unknown` so the UI surfaces classifier uncertainty rather than
//! pretending to know more than it does.

use serde::{Deserialize, Serialize};

use crate::mail::parse::Parsed;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "specta", derive(specta::Type))]
#[serde(rename_all = "snake_case")]
pub enum EmailTag {
    TransactionalAuth,
    TransactionalBilling,
    TransactionalNotification,
    Marketing,
    System,
    Unknown,
}

impl EmailTag {
    pub fn as_str(self) -> &'static str {
        match self {
            EmailTag::TransactionalAuth => "transactional_auth",
            EmailTag::TransactionalBilling => "transactional_billing",
            EmailTag::TransactionalNotification => "transactional_notification",
            EmailTag::Marketing => "marketing",
            EmailTag::System => "system",
            EmailTag::Unknown => "unknown",
        }
    }

    pub fn from_str(s: &str) -> Self {
        match s {
            "transactional_auth" => EmailTag::TransactionalAuth,
            "transactional_billing" => EmailTag::TransactionalBilling,
            "transactional_notification" => EmailTag::TransactionalNotification,
            "marketing" => EmailTag::Marketing,
            "system" => EmailTag::System,
            _ => EmailTag::Unknown,
        }
    }
}

/// Extract a plus-address tag (RFC 5233 §3.1) from the first
/// recipient that uses one. `alice+invoices@example.com` → "invoices".
///
/// We use this at ingest time as a higher-priority signal than the
/// heuristic classifier: a user typing `+invoices` is explicitly
/// asking for that bucket, regardless of what the message looks
/// like. Returns `None` when no recipient uses the form.
pub fn extract_plus_tag(rcpts: &[String]) -> Option<String> {
    for rcpt in rcpts {
        let Some(at) = rcpt.find('@') else { continue };
        let local = &rcpt[..at];
        let Some(plus) = local.find('+') else { continue };
        let tag = &local[plus + 1..];
        if tag.is_empty() {
            continue;
        }
        // Sanitize: tag column should be small + safe. Limit length
        // and strip everything that isn't a typical tag character.
        let cleaned: String = tag
            .chars()
            .filter(|c| c.is_alphanumeric() || matches!(*c, '-' | '_' | '.'))
            .take(64)
            .collect();
        if !cleaned.is_empty() {
            return Some(cleaned);
        }
    }
    None
}

/// Classify a parsed email. The returned tag is the highest-scoring
/// category, or `Unknown` when no signal crossed the threshold.
pub fn classify(parsed: &Parsed) -> EmailTag {
    let subject = parsed.header_subject.as_deref().unwrap_or("").to_lowercase();
    let body = parsed
        .text_body
        .as_deref()
        .or(parsed.html_body.as_deref())
        .unwrap_or("")
        .to_lowercase();
    let from = parsed.header_from.as_deref().unwrap_or("").to_lowercase();
    let headers = &parsed.headers_json;

    // System: autoresponder / bounce / DSN are unambiguous header
    // signals — check first so they don't get misclassified as auth.
    if header_eq(headers, "Auto-Submitted", "auto-replied")
        || header_eq(headers, "Auto-Submitted", "auto-generated")
        || header_exists(headers, "X-Failed-Recipients")
        || header_contains(headers, "Content-Type", "delivery-status")
        || from.contains("mailer-daemon")
        || from.contains("postmaster")
    {
        return EmailTag::System;
    }

    let mut scores = [0_i32; 5];
    // 0=Auth 1=Billing 2=Notification 3=Marketing 4=Unknown(ignored)

    // Auth signals.
    for kw in [
        "password reset",
        "reset your password",
        "verify your email",
        "verification code",
        "sign-in code",
        "sign in code",
        "magic link",
        "two-factor",
        "2fa",
        "confirm your account",
        "one-time code",
        "otp",
    ] {
        if subject.contains(kw) {
            scores[0] += 3;
        }
        if body.contains(kw) {
            scores[0] += 1;
        }
    }
    if from.contains("noreply") || from.contains("no-reply") {
        // Weak signal — many transactional senders use this.
        scores[0] += 1;
        scores[1] += 1;
        scores[2] += 1;
    }

    // Billing signals.
    for kw in [
        "invoice",
        "receipt",
        "payment",
        "billing",
        "subscription",
        "refund",
        "charged",
        "your card",
        "credit card",
        "transaction id",
    ] {
        if subject.contains(kw) {
            scores[1] += 3;
        }
        if body.contains(kw) {
            scores[1] += 1;
        }
    }

    // Notification signals.
    for kw in [
        "order shipped",
        "your order",
        "comment on",
        "replied to",
        "calendar invite",
        "meeting",
        "reminder",
        "new message from",
        "new follower",
        "mentioned you",
    ] {
        if subject.contains(kw) {
            scores[2] += 3;
        }
        if body.contains(kw) {
            scores[2] += 1;
        }
    }

    // Marketing signals.
    let has_unsubscribe_header = header_exists(headers, "List-Unsubscribe");
    if has_unsubscribe_header {
        scores[3] += 4;
    }
    for kw in [
        "% off",
        "% off!",
        "limited time",
        "sale ends",
        "shop now",
        "free shipping",
        "newsletter",
        "exclusive offer",
        "deal of the day",
        "weekly digest",
        "daily digest",
    ] {
        if subject.contains(kw) {
            scores[3] += 2;
        }
        if body.contains(kw) {
            scores[3] += 1;
        }
    }
    if body.contains("unsubscribe") {
        scores[3] += 1;
    }

    // Pick the highest-scoring category, with a minimum threshold of 3.
    let (idx, &top) = scores
        .iter()
        .enumerate()
        .max_by_key(|(_, s)| *s)
        .unwrap_or((4, &0));
    if top < 3 {
        return EmailTag::Unknown;
    }
    match idx {
        0 => EmailTag::TransactionalAuth,
        1 => EmailTag::TransactionalBilling,
        2 => EmailTag::TransactionalNotification,
        3 => EmailTag::Marketing,
        _ => EmailTag::Unknown,
    }
}

fn header_exists(h: &serde_json::Value, name: &str) -> bool {
    h.get(name).is_some()
}

fn header_eq(h: &serde_json::Value, name: &str, want: &str) -> bool {
    h.get(name)
        .and_then(|v| v.as_str())
        .map_or(false, |s| s.eq_ignore_ascii_case(want))
}

fn header_contains(h: &serde_json::Value, name: &str, needle: &str) -> bool {
    h.get(name)
        .and_then(|v| v.as_str())
        .map_or(false, |s| s.to_lowercase().contains(needle))
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn parsed(subject: &str, body: &str, from: &str, headers: serde_json::Value) -> Parsed {
        Parsed {
            header_from: Some(from.into()),
            header_to: None,
            header_cc: None,
            header_subject: Some(subject.into()),
            message_id: None,
            in_reply_to: None,
            text_body: Some(body.into()),
            html_body: None,
            has_text: true,
            has_html: false,
            headers_json: headers,
            attachments: Vec::new(),
        }
    }

    #[test]
    fn auth_email() {
        let p = parsed(
            "Password reset request",
            "Click here to reset your password",
            "noreply@bank.example",
            json!({}),
        );
        assert_eq!(classify(&p), EmailTag::TransactionalAuth);
    }

    #[test]
    fn billing_email() {
        let p = parsed(
            "Your invoice for October",
            "Total: $42.00 charged to your card.",
            "billing@stripe.example",
            json!({}),
        );
        assert_eq!(classify(&p), EmailTag::TransactionalBilling);
    }

    #[test]
    fn marketing_with_list_unsubscribe() {
        let p = parsed(
            "Big sale this week!",
            "Shop now for 50% off",
            "promo@shop.example",
            json!({"List-Unsubscribe": "<mailto:unsub@shop.example>"}),
        );
        assert_eq!(classify(&p), EmailTag::Marketing);
    }

    #[test]
    fn system_bounce() {
        let p = parsed(
            "Undeliverable: Your message",
            "The following message could not be delivered.",
            "MAILER-DAEMON@example.com",
            json!({"X-Failed-Recipients": "bad@dest"}),
        );
        assert_eq!(classify(&p), EmailTag::System);
    }

    #[test]
    fn unknown_when_no_signal() {
        let p = parsed("hi", "see you tomorrow", "alice@friend.example", json!({}));
        assert_eq!(classify(&p), EmailTag::Unknown);
    }

    #[test]
    fn plus_tag_extraction() {
        assert_eq!(
            extract_plus_tag(&["alice+invoices@example.com".into()]),
            Some("invoices".into())
        );
        assert_eq!(
            extract_plus_tag(&["alice@example.com".into(), "bob+ci-run@example.com".into()]),
            Some("ci-run".into())
        );
        assert_eq!(
            extract_plus_tag(&["alice@example.com".into()]),
            None
        );
        // Empty tag after `+` returns None.
        assert_eq!(
            extract_plus_tag(&["alice+@example.com".into()]),
            None
        );
        // Disallowed characters are stripped.
        assert_eq!(
            extract_plus_tag(&["alice+!!hello!!@example.com".into()]),
            Some("hello".into())
        );
    }

    #[test]
    fn notification_wins_over_weak_others() {
        let p = parsed(
            "Your order has shipped",
            "Tracking number: 1Z999",
            "shipping@store.example",
            json!({}),
        );
        assert_eq!(classify(&p), EmailTag::TransactionalNotification);
    }
}
