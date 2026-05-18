//! Link extraction + classification for FR-SCENARIO-40.
//!
//! Pure local parsing — we do **not** HEAD-check links by default
//! (NFR-PRIV-01). Each link gets a structured verdict for the UI:
//! is it `http://` (insecure)? Does it look like a tracking
//! redirect? Is it likely a mailto/tel that should render
//! differently? Optional online check is a separate API the caller
//! must explicitly opt into.

use std::collections::HashSet;

use regex::Regex;
use serde::Serialize;

use crate::mail::parse::Parsed;

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct LinkReport {
    pub links: Vec<DetectedLink>,
    pub counts: LinkCounts,
}

#[derive(Debug, Clone, Default, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct LinkCounts {
    pub total: u32,
    pub insecure_http: u32,
    pub mailto: u32,
    pub tel: u32,
    pub tracking_likely: u32,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct DetectedLink {
    pub url: String,
    pub kind: LinkKind,
    /// Surface in the UI as a small warning chip.
    pub warnings: Vec<&'static str>,
}

#[derive(Debug, Clone, Copy, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum LinkKind {
    Http,
    Https,
    Mailto,
    Tel,
    Other,
}

/// Extract every URL we can find from the HTML and text bodies.
pub fn extract(parsed: &Parsed) -> LinkReport {
    let mut seen: HashSet<String> = HashSet::new();
    let mut out: Vec<DetectedLink> = Vec::new();

    if let Some(html) = parsed.html_body.as_deref() {
        // href="..." needs the capture group; the regex's full match
        // includes the `href=...` prefix, which we don't want.
        for cap in href_regex().captures_iter(html) {
            if let Some(url) = cap.get(1) {
                push(&mut seen, &mut out, url.as_str());
            }
        }
        // Bare URLs in HTML too (sometimes templates render them
        // outside of href, e.g. plain-text footers).
        for url in bare_url_regex().find_iter(html) {
            push(&mut seen, &mut out, url.as_str());
        }
    }
    if let Some(text) = parsed.text_body.as_deref() {
        for url in bare_url_regex().find_iter(text) {
            push(&mut seen, &mut out, url.as_str());
        }
    }

    let counts = LinkCounts {
        total: out.len() as u32,
        insecure_http: out.iter().filter(|l| matches!(l.kind, LinkKind::Http)).count() as u32,
        mailto: out.iter().filter(|l| matches!(l.kind, LinkKind::Mailto)).count() as u32,
        tel: out.iter().filter(|l| matches!(l.kind, LinkKind::Tel)).count() as u32,
        tracking_likely: out
            .iter()
            .filter(|l| l.warnings.iter().any(|w| *w == "tracking-redirect"))
            .count() as u32,
    };
    LinkReport { links: out, counts }
}

fn push(seen: &mut HashSet<String>, out: &mut Vec<DetectedLink>, raw: &str) {
    let url = strip_attribute_wrappers(raw);
    if url.is_empty() || !seen.insert(url.to_string()) {
        return;
    }
    let kind = classify(&url);
    let mut warnings: Vec<&'static str> = Vec::new();
    if matches!(kind, LinkKind::Http) {
        warnings.push("insecure-http");
    }
    if looks_like_tracking(&url) {
        warnings.push("tracking-redirect");
    }
    out.push(DetectedLink { url, kind, warnings });
}

fn classify(url: &str) -> LinkKind {
    let lower = url.to_ascii_lowercase();
    if lower.starts_with("https://") {
        LinkKind::Https
    } else if lower.starts_with("http://") {
        LinkKind::Http
    } else if lower.starts_with("mailto:") {
        LinkKind::Mailto
    } else if lower.starts_with("tel:") {
        LinkKind::Tel
    } else {
        LinkKind::Other
    }
}

fn looks_like_tracking(url: &str) -> bool {
    // Subdomains that almost always mean redirect-through-tracker.
    for fragment in [
        "click.",
        "tracking.",
        "track.",
        "links.",
        "/click?",
        "/ck/",
        "utm_source=",
        "utm_medium=",
        "utm_campaign=",
        "?ref=",
        "&ref=",
    ] {
        if url.contains(fragment) {
            return true;
        }
    }
    false
}

fn strip_attribute_wrappers(s: &str) -> String {
    s.trim_matches(|c: char| c == '"' || c == '\'' || c.is_whitespace())
        .trim_end_matches(|c: char| matches!(c, '.' | ',' | ')' | ']' | ';'))
        .to_string()
}

fn href_regex() -> &'static Regex {
    static R: std::sync::OnceLock<Regex> = std::sync::OnceLock::new();
    R.get_or_init(|| {
        // Matches the URL inside `href="..."` / `href='...'`.
        Regex::new(r#"(?i)href\s*=\s*["']([^"']+)["']"#).unwrap()
    })
}

fn bare_url_regex() -> &'static Regex {
    static R: std::sync::OnceLock<Regex> = std::sync::OnceLock::new();
    R.get_or_init(|| {
        Regex::new(
            r"(?i)\b(?:https?://|mailto:|tel:)[^\s<>()\[\]\\\^`{|}]+"
        ).unwrap()
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn parsed_with_html(html: &str) -> Parsed {
        Parsed {
            header_from: None,
            header_to: None,
            header_cc: None,
            header_subject: None,
            message_id: None,
            in_reply_to: None,
            text_body: None,
            html_body: Some(html.into()),
            has_text: false,
            has_html: true,
            headers_json: json!({}),
            attachments: Vec::new(),
        }
    }

    #[test]
    fn finds_http_and_marks_insecure() {
        let p = parsed_with_html(r#"<a href="http://example.com/path">x</a>"#);
        let r = extract(&p);
        assert_eq!(r.links.len(), 1);
        assert!(r.links[0].warnings.contains(&"insecure-http"));
        assert_eq!(r.counts.insecure_http, 1);
    }

    #[test]
    fn detects_tracking_redirect() {
        let p = parsed_with_html(r#"<a href="https://click.brand.example/r/abc">x</a>"#);
        let r = extract(&p);
        assert!(r.links[0].warnings.contains(&"tracking-redirect"));
    }

    #[test]
    fn dedupes_identical_urls() {
        let html = r#"<a href="https://example.com">a</a><a href="https://example.com">b</a>"#;
        let r = extract(&parsed_with_html(html));
        assert_eq!(r.links.len(), 1);
    }
}
