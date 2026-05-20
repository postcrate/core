//! `List-Unsubscribe` header syntax validation per RFC 2369 and the
//! one-click semantics in RFC 8058. Used in the marketing-mail
//! preflight that Gmail/Yahoo bulk-sender rules in 2024 made
//! effectively mandatory for high-volume senders.
//!
//! We check four things:
//!
//!   - Presence: was the header included at all?
//!   - Syntax: is it a comma-separated list of `<uri>` entries?
//!   - Method coverage: is at least one mailto OR https URI present?
//!   - One-click pairing: if `List-Unsubscribe-Post: List-Unsubscribe=One-Click`
//!     is set (RFC 8058), there must be at least one https URI.

use serde::Serialize;

use crate::mail::parse::Parsed;

#[derive(Debug, Clone, Serialize)]
#[cfg_attr(feature = "specta", derive(specta::Type))]
#[serde(rename_all = "camelCase")]
pub struct UnsubReport {
    pub present: bool,
    pub valid: bool,
    pub uris: Vec<UnsubUri>,
    pub one_click: bool,
    pub findings: Vec<UnsubFinding>,
}

#[derive(Debug, Clone, Serialize)]
#[cfg_attr(feature = "specta", derive(specta::Type))]
#[serde(rename_all = "camelCase")]
pub struct UnsubUri {
    pub raw: String,
    pub scheme: String,
}

#[derive(Debug, Clone, Serialize)]
#[cfg_attr(feature = "specta", derive(specta::Type))]
#[serde(rename_all = "camelCase")]
pub struct UnsubFinding {
    pub rule: &'static str,
    pub severity: &'static str,
    pub message: String,
}

pub fn analyze(parsed: &Parsed) -> UnsubReport {
    let headers = &parsed.headers_json;
    let header = headers
        .get("List-Unsubscribe")
        .and_then(|v| v.as_str())
        .map(str::trim);
    let post = headers
        .get("List-Unsubscribe-Post")
        .and_then(|v| v.as_str())
        .map(str::trim);

    let Some(raw) = header else {
        return UnsubReport {
            present: false,
            valid: false,
            uris: Vec::new(),
            one_click: false,
            findings: vec![UnsubFinding {
                rule: "MISSING_HEADER",
                severity: "info",
                message: "No List-Unsubscribe header. Required by Gmail/Yahoo bulk-sender rules.".into(),
            }],
        };
    };

    let mut findings: Vec<UnsubFinding> = Vec::new();
    let mut uris: Vec<UnsubUri> = Vec::new();
    let mut valid = true;

    for piece in split_csv_outside_brackets(raw) {
        let piece = piece.trim();
        if piece.is_empty() {
            continue;
        }
        if !(piece.starts_with('<') && piece.ends_with('>')) {
            findings.push(UnsubFinding {
                rule: "SYNTAX",
                severity: "error",
                message: format!("Entry {piece:?} is not wrapped in <...>"),
            });
            valid = false;
            continue;
        }
        let inner = &piece[1..piece.len() - 1];
        let scheme = inner
            .split(':')
            .next()
            .unwrap_or("")
            .to_ascii_lowercase();
        if scheme.is_empty() {
            findings.push(UnsubFinding {
                rule: "SYNTAX",
                severity: "error",
                message: format!("Entry {piece:?} has no URI scheme"),
            });
            valid = false;
            continue;
        }
        uris.push(UnsubUri {
            raw: inner.to_string(),
            scheme,
        });
    }

    let has_mailto = uris.iter().any(|u| u.scheme == "mailto");
    let has_https = uris.iter().any(|u| u.scheme == "https");
    let has_http = uris.iter().any(|u| u.scheme == "http");

    if has_http && !has_https {
        findings.push(UnsubFinding {
            rule: "INSECURE_HTTP",
            severity: "warning",
            message: "Plain http:// URI used; clients increasingly require https://".into(),
        });
    }

    if !has_mailto && !has_https && !has_http {
        findings.push(UnsubFinding {
            rule: "NO_USABLE_URI",
            severity: "error",
            message: "Header has no mailto: or https: URI; nothing for clients to act on".into(),
        });
        valid = false;
    }

    // RFC 8058 one-click: List-Unsubscribe-Post must be present and
    // equal to "List-Unsubscribe=One-Click". When set, an https URI
    // is required.
    let one_click = matches!(
        post,
        Some(p) if p.eq_ignore_ascii_case("List-Unsubscribe=One-Click")
    );
    if one_click && !has_https {
        findings.push(UnsubFinding {
            rule: "ONE_CLICK_NO_HTTPS",
            severity: "error",
            message: "List-Unsubscribe-Post claims One-Click, but no https URI in List-Unsubscribe".into(),
        });
        valid = false;
    }

    UnsubReport { present: true, valid, uris, one_click, findings }
}

/// Split a string on commas that aren't inside `<...>`. Conservative —
/// we don't support nested angle brackets because real headers don't.
fn split_csv_outside_brackets(s: &str) -> Vec<&str> {
    let mut out = Vec::new();
    let mut depth: i32 = 0;
    let mut last = 0;
    for (i, c) in s.char_indices() {
        match c {
            '<' => depth += 1,
            '>' => depth -= 1,
            ',' if depth == 0 => {
                out.push(&s[last..i]);
                last = i + 1;
            }
            _ => {}
        }
    }
    out.push(&s[last..]);
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn with(h: serde_json::Value) -> Parsed {
        Parsed {
            header_from: None,
            header_to: None,
            header_cc: None,
            header_subject: None,
            message_id: None,
            in_reply_to: None,
            text_body: None,
            html_body: None,
            has_text: false,
            has_html: false,
            headers_json: h,
            attachments: Vec::new(),
        }
    }

    #[test]
    fn missing_header() {
        let r = analyze(&with(json!({})));
        assert!(!r.present);
        assert!(!r.valid);
    }

    #[test]
    fn well_formed_mailto_only() {
        let r = analyze(&with(json!({
            "List-Unsubscribe": "<mailto:unsubscribe@example.com>",
        })));
        assert!(r.present);
        assert!(r.valid);
        assert_eq!(r.uris.len(), 1);
        assert!(!r.one_click);
    }

    #[test]
    fn one_click_requires_https() {
        let r = analyze(&with(json!({
            "List-Unsubscribe": "<mailto:unsub@example.com>",
            "List-Unsubscribe-Post": "List-Unsubscribe=One-Click",
        })));
        assert!(!r.valid);
        assert!(r.findings.iter().any(|f| f.rule == "ONE_CLICK_NO_HTTPS"));
    }

    #[test]
    fn one_click_with_https_valid() {
        let r = analyze(&with(json!({
            "List-Unsubscribe": "<https://example.com/unsub?id=1>, <mailto:u@example.com>",
            "List-Unsubscribe-Post": "List-Unsubscribe=One-Click",
        })));
        assert!(r.valid, "findings: {:?}", r.findings);
        assert!(r.one_click);
        assert_eq!(r.uris.len(), 2);
    }

    #[test]
    fn syntax_error_unwrapped_uri() {
        let r = analyze(&with(json!({
            "List-Unsubscribe": "mailto:u@example.com",
        })));
        assert!(!r.valid);
        assert!(r.findings.iter().any(|f| f.rule == "SYNTAX"));
    }
}
