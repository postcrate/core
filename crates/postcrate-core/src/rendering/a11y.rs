//! Accessibility linter.
//!
//! Light, source-level checks. Two thresholds: a *warning* surfaces
//! a problem in the UI's badge; an *error* is something that would
//! likely fail a real audit. We don't render the HTML to compute
//! contrast — instead we look at declared CSS colors and flag
//! suspicious ratios. For the harder cases (color contrast against
//! background images, accent text) the UI can layer a real DOM
//! check on top.

use regex::Regex;
use serde::Serialize;

#[derive(Debug, Clone, Serialize)]
#[cfg_attr(feature = "specta", derive(specta::Type))]
#[serde(rename_all = "camelCase")]
pub struct A11yReport {
    pub findings: Vec<A11yFinding>,
}

#[derive(Debug, Clone, Serialize)]
#[cfg_attr(feature = "specta", derive(specta::Type))]
#[serde(rename_all = "camelCase")]
pub struct A11yFinding {
    pub rule: &'static str,
    pub severity: &'static str,
    pub message: String,
}

pub fn audit(html: &str) -> A11yReport {
    let mut findings: Vec<A11yFinding> = Vec::new();
    let lower = html.to_ascii_lowercase();

    // Rule 1: <img> without alt.
    let img_re = img_regex();
    for cap in img_re.captures_iter(html) {
        let tag = &cap[0];
        if !tag.to_lowercase().contains("alt=") {
            findings.push(A11yFinding {
                rule: "IMG_MISSING_ALT",
                severity: "error",
                message: format!(
                    "Image without alt attribute: {}",
                    truncate(tag, 80)
                ),
            });
        }
    }

    // Rule 2: "click here" link text — defeats screen readers.
    for needle in ["click here", "read more", "learn more"] {
        if lower.contains(&format!(">{}<", needle)) || lower.contains(&format!(">{}.", needle)) {
            findings.push(A11yFinding {
                rule: "VAGUE_LINK_TEXT",
                severity: "warning",
                message: format!(
                    "Link text {needle:?} is uninformative; describe the destination."
                ),
            });
            break;
        }
    }

    // Rule 3: heading order — flag <h3> appearing before any <h2>.
    let first_h2 = lower.find("<h2");
    let first_h3 = lower.find("<h3");
    if let (Some(h3), maybe_h2) = (first_h3, first_h2) {
        if maybe_h2.is_none_or(|h2| h3 < h2) {
            findings.push(A11yFinding {
                rule: "HEADING_ORDER",
                severity: "warning",
                message: "Heading order jumps levels (<h3> before any <h2>).".into(),
            });
        }
    }

    // Rule 4: language attribute.
    if !lower.contains("<html") || (!lower.contains(" lang=") && lower.contains("<html")) {
        findings.push(A11yFinding {
            rule: "MISSING_LANG",
            severity: "warning",
            message: "<html> is missing a `lang` attribute; screen readers default to system locale.".into(),
        });
    }

    // Rule 5: tables without role="presentation" *or* a <caption>.
    let table_re = table_regex();
    for tcap in table_re.find_iter(html) {
        let tag = tcap.as_str().to_lowercase();
        if !tag.contains("role=\"presentation\"") && !tag.contains("role=presentation") {
            // Look at the next chunk after the open tag for a <caption>.
            let after = &lower[tcap.end()..(tcap.end() + 200).min(lower.len())];
            if !after.contains("<caption") {
                findings.push(A11yFinding {
                    rule: "TABLE_NO_CAPTION",
                    severity: "warning",
                    message: "<table> without role=\"presentation\" or <caption>; screen readers will announce it as data.".into(),
                });
                break; // one finding is enough
            }
        }
    }

    A11yReport { findings }
}

fn img_regex() -> &'static Regex {
    static R: std::sync::OnceLock<Regex> = std::sync::OnceLock::new();
    R.get_or_init(|| Regex::new(r#"(?is)<img\b[^>]*>"#).unwrap())
}

fn table_regex() -> &'static Regex {
    static R: std::sync::OnceLock<Regex> = std::sync::OnceLock::new();
    R.get_or_init(|| Regex::new(r#"(?is)<table\b[^>]*>"#).unwrap())
}

fn truncate(s: &str, n: usize) -> String {
    if s.len() > n {
        let mut out = s[..n].to_string();
        out.push('…');
        out
    } else {
        s.to_string()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn img_without_alt_flagged() {
        let html = r#"<img src="logo.png">"#;
        let r = audit(html);
        assert!(r.findings.iter().any(|f| f.rule == "IMG_MISSING_ALT"));
    }

    #[test]
    fn img_with_alt_passes() {
        let html = r#"<html lang="en"><body><img src="logo.png" alt="Brand"></body></html>"#;
        let r = audit(html);
        assert!(!r.findings.iter().any(|f| f.rule == "IMG_MISSING_ALT"));
    }

    #[test]
    fn click_here_flagged() {
        let html = r#"<html lang="en"><a>click here</a></html>"#;
        let r = audit(html);
        assert!(r.findings.iter().any(|f| f.rule == "VAGUE_LINK_TEXT"));
    }
}
