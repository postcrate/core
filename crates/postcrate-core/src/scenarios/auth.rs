//! SPF / DKIM / DMARC inspection.
//!
//! Strictly header-only — we do not perform DNS lookups. The output
//! is framed as a *prediction*: "would pass at a typical receiver"
//! based on what's in the `Authentication-Results` header (or
//! equivalent), plus the presence of DKIM-Signature.
//!
//! When the headers say nothing we report `Unknown`, never a green
//! check. This matches the spec's "honest about being a prediction"
//! framing.

use serde::Serialize;

use crate::mail::parse::Parsed;

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct AuthReport {
    pub spf: AuthVerdict,
    pub dkim: AuthVerdict,
    pub dmarc: AuthVerdict,
    /// True iff a `DKIM-Signature` header is present (regardless of
    /// whether `Authentication-Results` confirmed it).
    pub has_dkim_signature: bool,
    /// What we found in `Authentication-Results`, if anything.
    pub authentication_results: Option<String>,
}

#[derive(Debug, Clone, Copy, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum AuthVerdict {
    Pass,
    Fail,
    Softfail,
    Neutral,
    None,
    /// We don't know — neither `Authentication-Results` nor
    /// per-protocol header gave us a verdict.
    Unknown,
}

pub fn analyze(parsed: &Parsed) -> AuthReport {
    let headers = &parsed.headers_json;
    let auth_results = headers
        .get("Authentication-Results")
        .and_then(|v| v.as_str())
        .map(|s| s.to_string());

    let (spf, dkim, dmarc) = match &auth_results {
        Some(s) => (
            parse_verdict(s, "spf="),
            parse_verdict(s, "dkim="),
            parse_verdict(s, "dmarc="),
        ),
        None => (AuthVerdict::Unknown, AuthVerdict::Unknown, AuthVerdict::Unknown),
    };

    let has_dkim_signature = headers.get("DKIM-Signature").is_some();
    // If we have a DKIM-Signature but no Authentication-Results verdict,
    // upgrade DKIM from Unknown to "would pass if cryptographically valid".
    let dkim = if matches!(dkim, AuthVerdict::Unknown) && has_dkim_signature {
        AuthVerdict::Neutral
    } else {
        dkim
    };

    AuthReport {
        spf,
        dkim,
        dmarc,
        has_dkim_signature,
        authentication_results: auth_results,
    }
}

fn parse_verdict(auth_results: &str, prefix: &str) -> AuthVerdict {
    let s = auth_results.to_lowercase();
    let Some(start) = s.find(prefix) else {
        return AuthVerdict::Unknown;
    };
    let after = &s[start + prefix.len()..];
    let verdict: String = after
        .chars()
        .take_while(|c| c.is_alphanumeric() || *c == '-' || *c == '_')
        .collect();
    match verdict.as_str() {
        "pass" => AuthVerdict::Pass,
        "fail" => AuthVerdict::Fail,
        "softfail" => AuthVerdict::Softfail,
        "neutral" | "permerror" | "temperror" => AuthVerdict::Neutral,
        "none" => AuthVerdict::None,
        _ => AuthVerdict::Unknown,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn with_headers(h: serde_json::Value) -> Parsed {
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
    fn unknown_when_nothing_present() {
        let r = analyze(&with_headers(json!({})));
        assert!(matches!(r.spf, AuthVerdict::Unknown));
        assert!(matches!(r.dkim, AuthVerdict::Unknown));
        assert!(matches!(r.dmarc, AuthVerdict::Unknown));
    }

    #[test]
    fn all_pass() {
        let r = analyze(&with_headers(json!({
            "Authentication-Results": "mx.google.com; spf=pass; dkim=pass; dmarc=pass",
        })));
        assert!(matches!(r.spf, AuthVerdict::Pass));
        assert!(matches!(r.dkim, AuthVerdict::Pass));
        assert!(matches!(r.dmarc, AuthVerdict::Pass));
    }

    #[test]
    fn dkim_signature_only_is_neutral_not_pass() {
        let r = analyze(&with_headers(json!({
            "DKIM-Signature": "v=1; a=rsa-sha256; ...",
        })));
        assert!(r.has_dkim_signature);
        assert!(matches!(r.dkim, AuthVerdict::Neutral));
    }

    #[test]
    fn softfail_recognized() {
        let r = analyze(&with_headers(json!({
            "Authentication-Results": "mx; spf=softfail; dkim=fail; dmarc=fail",
        })));
        assert!(matches!(r.spf, AuthVerdict::Softfail));
        assert!(matches!(r.dkim, AuthVerdict::Fail));
        assert!(matches!(r.dmarc, AuthVerdict::Fail));
    }
}
