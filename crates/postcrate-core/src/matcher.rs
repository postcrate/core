//! Email predicates + structured matching.
//!
//! Shared primitive used by:
//!   - the MCP `wait_for_email` and `assert_email_matches` tools
//!     (FR-AI-04, FR-AI-05),
//!   - the CLI `wait` subcommand (FR-AI-10),
//!   - the matcher packages' `waitForEmail` / `toContainEmail`
//!     helpers (FR-TEST-40),
//!   - the HTTP "did it send" diagnostic endpoint (FR-AI-20).
//!
//! One type, two uses: cheap `matches_summary` for live-stream
//! filtering against the lightweight `EmailSummary`, and full
//! `check` returning a structured mismatch report against a parsed
//! `EmailDetail`.

use regex::Regex;
use serde::{Deserialize, Serialize};

use crate::db::emails::{EmailDetail, EmailSummary};

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct EmailPredicate {
    /// Restrict to a single mailbox.
    pub mailbox_id: Option<String>,
    /// Sender substring (case-insensitive).
    pub from: Option<String>,
    /// Sender regex.
    pub from_regex: Option<String>,
    /// Any recipient contains this substring (case-insensitive).
    pub to: Option<String>,
    /// Subject substring (case-insensitive).
    pub subject: Option<String>,
    /// Subject regex.
    pub subject_regex: Option<String>,
    /// Plain-text body substring.
    pub body_contains: Option<String>,
    /// Plain-text body regex.
    pub body_regex: Option<String>,
    /// `Some(true)` requires at least one attachment; `Some(false)`
    /// requires zero. `None` means "don't care".
    pub has_attachment: Option<bool>,
    /// Per-header predicates (any combination of substring/regex).
    #[serde(default)]
    pub headers: Vec<HeaderPredicate>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct HeaderPredicate {
    pub name: String,
    /// Match the header value via case-insensitive substring.
    pub contains: Option<String>,
    /// Match the header value via regex.
    pub regex: Option<String>,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct MatchResult {
    pub matched: bool,
    pub mismatches: Vec<String>,
}

impl MatchResult {
    pub fn ok() -> Self {
        Self { matched: true, mismatches: Vec::new() }
    }
}

impl EmailPredicate {
    /// True if the predicate could possibly match — used for cheap
    /// filtering against an `EmailSummary` before paying the cost of
    /// fetching the full detail.
    pub fn matches_summary(&self, s: &EmailSummary) -> bool {
        if let Some(mb) = &self.mailbox_id {
            if s.mailbox_id != *mb {
                return false;
            }
        }
        if let Some(needle) = &self.from {
            if !s.from.to_lowercase().contains(&needle.to_lowercase()) {
                return false;
            }
        }
        if let Some(needle) = &self.to {
            let nl = needle.to_lowercase();
            if !s.to.iter().any(|r| r.to_lowercase().contains(&nl)) {
                return false;
            }
        }
        if let Some(needle) = &self.subject {
            let nl = needle.to_lowercase();
            let got = s.subject.as_deref().unwrap_or("").to_lowercase();
            if !got.contains(&nl) {
                return false;
            }
        }
        // Regex/body/header checks defer to `check()` against the detail.
        true
    }

    /// Full check against a parsed detail. Populates `mismatches`
    /// with one human-readable line per failed clause; an empty
    /// `mismatches` means the predicate matched.
    pub fn check(&self, d: &EmailDetail) -> MatchResult {
        let mut out = MatchResult::ok();

        if let Some(mb) = &self.mailbox_id {
            if d.mailbox_id != *mb {
                out.matched = false;
                out.mismatches
                    .push(format!("mailboxId: expected {mb:?}, got {:?}", d.mailbox_id));
            }
        }

        if let Some(needle) = &self.from {
            if !d.from.to_lowercase().contains(&needle.to_lowercase()) {
                out.matched = false;
                out.mismatches
                    .push(format!("from: expected to contain {needle:?}, got {:?}", d.from));
            }
        }
        if let Some(pat) = &self.from_regex {
            check_regex(&mut out, "from", pat, &d.from);
        }

        if let Some(needle) = &self.to {
            let nl = needle.to_lowercase();
            if !d.to.iter().any(|r| r.to_lowercase().contains(&nl)) {
                out.matched = false;
                out.mismatches
                    .push(format!("to: expected one of {:?} to contain {needle:?}", d.to));
            }
        }

        let subject = d.subject.as_deref().unwrap_or("");
        if let Some(needle) = &self.subject {
            if !subject.to_lowercase().contains(&needle.to_lowercase()) {
                out.matched = false;
                out.mismatches
                    .push(format!("subject: expected to contain {needle:?}, got {subject:?}"));
            }
        }
        if let Some(pat) = &self.subject_regex {
            check_regex(&mut out, "subject", pat, subject);
        }

        let text_body = d.text_body.as_deref().unwrap_or("");
        if let Some(needle) = &self.body_contains {
            if !text_body.contains(needle) {
                out.matched = false;
                out.mismatches
                    .push(format!("bodyContains: {needle:?} not found in textBody"));
            }
        }
        if let Some(pat) = &self.body_regex {
            check_regex(&mut out, "bodyRegex", pat, text_body);
        }

        if let Some(want_some) = self.has_attachment {
            let got_some = !d.attachments.is_empty();
            if got_some != want_some {
                out.matched = false;
                out.mismatches.push(format!(
                    "hasAttachment: expected {want_some}, got {got_some}"
                ));
            }
        }

        // Headers. Each header predicate looks up the value in
        // `d.headers` (whose shape is `{ "Subject": "...", ... }` —
        // see mail::headers::headers_to_json) and runs its checks.
        for hp in &self.headers {
            let value = d
                .headers
                .get(&hp.name)
                .and_then(|v| v.as_str())
                .unwrap_or("");
            if let Some(needle) = &hp.contains {
                if !value.to_lowercase().contains(&needle.to_lowercase()) {
                    out.matched = false;
                    out.mismatches.push(format!(
                        "header {:?}: expected to contain {:?}, got {:?}",
                        hp.name, needle, value
                    ));
                }
            }
            if let Some(pat) = &hp.regex {
                check_regex(&mut out, &format!("header {:?}", hp.name), pat, value);
            }
        }

        out
    }
}

fn check_regex(out: &mut MatchResult, field: &str, pat: &str, haystack: &str) {
    match Regex::new(pat) {
        Ok(re) => {
            if !re.is_match(haystack) {
                out.matched = false;
                out.mismatches.push(format!(
                    "{field}: regex {pat:?} did not match {haystack:?}"
                ));
            }
        }
        Err(e) => {
            out.matched = false;
            out.mismatches
                .push(format!("{field}: invalid regex {pat:?}: {e}"));
        }
    }
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct WaitOutcome {
    /// The first email that satisfied the predicate, or `None` if we
    /// timed out without a match.
    pub matched: Option<EmailDetail>,
    /// Every captured email observed during the wait window (whether
    /// or not it matched). Lets agents diagnose "code didn't try" vs
    /// "code addressed it wrong".
    pub seen_during_wait: Vec<EmailSummary>,
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn summary(id: &str, from: &str, to: &[&str], subject: &str) -> EmailSummary {
        EmailSummary {
            id: id.into(),
            mailbox_id: "mb".into(),
            received_at: 0,
            from: from.into(),
            to: to.iter().map(|s| s.to_string()).collect(),
            subject: Some(subject.into()),
            has_html: false,
            has_text: true,
            size_bytes: 0,
            read: false,
            pinned: false,
            starred: false,
            tag: None,
        }
    }

    fn detail(s: &EmailSummary, body: &str) -> EmailDetail {
        EmailDetail {
            id: s.id.clone(),
            mailbox_id: s.mailbox_id.clone(),
            received_at: s.received_at,
            from: s.from.clone(),
            to: s.to.clone(),
            subject: s.subject.clone(),
            has_html: s.has_html,
            has_text: s.has_text,
            size_bytes: s.size_bytes,
            read: s.read,
            pinned: false,
            starred: false,
            note: None,
            tag: None,
            headers: json!({"X-Mailer": "Postcrate-test"}),
            text_body: Some(body.into()),
            html_body: None,
            attachments: Vec::new(),
            message_id: None,
            in_reply_to: None,
            ext_smtputf8: false,
            ext_8bitmime: false,
        }
    }

    #[test]
    fn summary_filter_case_insensitive() {
        let s = summary("1", "Alice@Example.com", &["Bob@example.com"], "Welcome!");
        let mut p = EmailPredicate::default();
        p.from = Some("alice".into());
        assert!(p.matches_summary(&s));
        p.to = Some("BOB".into());
        assert!(p.matches_summary(&s));
        p.subject = Some("welc".into());
        assert!(p.matches_summary(&s));
    }

    #[test]
    fn detail_check_reports_each_mismatch() {
        let s = summary("1", "a@b", &["c@d"], "Order shipped");
        let d = detail(&s, "Your package is en route.");
        let p = EmailPredicate {
            from: Some("nobody".into()),
            subject_regex: Some(r"^Refund".into()),
            body_contains: Some("expired".into()),
            has_attachment: Some(true),
            ..Default::default()
        };
        let r = p.check(&d);
        assert!(!r.matched);
        assert_eq!(r.mismatches.len(), 4, "got {:?}", r.mismatches);
    }

    #[test]
    fn detail_check_passes() {
        let s = summary("1", "alerts@bank.example", &["user@example.com"], "Password Reset");
        let d = detail(&s, "Click here to reset: https://bank.example/reset?t=abc");
        let p = EmailPredicate {
            from: Some("bank.example".into()),
            subject_regex: Some(r"(?i)password\s+reset".into()),
            body_regex: Some(r"https://\S+/reset\?t=\w+".into()),
            ..Default::default()
        };
        assert!(p.check(&d).matched, "{:?}", p.check(&d).mismatches);
    }

    #[test]
    fn header_predicate() {
        let s = summary("1", "a@b", &["c@d"], "hi");
        let d = detail(&s, "body");
        let mut p = EmailPredicate::default();
        p.headers.push(HeaderPredicate {
            name: "X-Mailer".into(),
            contains: Some("postcrate".into()),
            regex: None,
        });
        assert!(p.check(&d).matched);
    }
}
