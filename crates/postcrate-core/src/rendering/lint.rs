//! HTML linter for captured emails (FR-RENDER-10).
//!
//! Each rule is a cheap text pattern + a single human sentence. We
//! deliberately avoid HTML parsing: we want this to run on every
//! captured email cheaply, and for the warnings to point at literal
//! source lines.

use serde::Serialize;

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct LintReport {
    pub warnings: Vec<LintWarning>,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct LintWarning {
    pub rule: &'static str,
    /// "high" / "medium" / "low".
    pub severity: &'static str,
    pub message: &'static str,
    /// Where in the HTML we hit it — UI can highlight.
    pub byte_offset: Option<usize>,
    /// Which clients are affected by this issue.
    pub affects: &'static [&'static str],
}

pub fn lint(html: &str) -> LintReport {
    let mut warnings: Vec<LintWarning> = Vec::new();
    let lower = html.to_ascii_lowercase();

    // Rule 1: <style> inside <body>.
    if let Some(body_pos) = lower.find("<body") {
        if let Some(style_pos) = lower[body_pos..].find("<style") {
            warnings.push(LintWarning {
                rule: "STYLE_IN_BODY",
                severity: "high",
                message: "Gmail / Outlook Web strip <style> blocks inside <body>. Move them to <head> or inline.",
                byte_offset: Some(body_pos + style_pos),
                affects: &["Gmail Web", "Gmail iOS", "Outlook Web", "Yahoo Mail"],
            });
        }
    }

    // Rule 2: CSS Grid usage.
    if let Some(pos) = lower.find("display: grid").or_else(|| lower.find("display:grid")) {
        warnings.push(LintWarning {
            rule: "CSS_GRID",
            severity: "high",
            message: "CSS Grid is not supported in Outlook or older Gmail clients. Use tables.",
            byte_offset: Some(pos),
            affects: &["Outlook Desktop", "Outlook Web", "Gmail iOS"],
        });
    }

    // Rule 3: Flexbox.
    if let Some(pos) = lower.find("display: flex").or_else(|| lower.find("display:flex")) {
        warnings.push(LintWarning {
            rule: "CSS_FLEX",
            severity: "medium",
            message: "Flexbox is unsupported in Outlook Desktop. Provide a table fallback.",
            byte_offset: Some(pos),
            affects: &["Outlook Desktop"],
        });
    }

    // Rule 4: Web fonts via @import.
    if let Some(pos) = lower.find("@import url") {
        warnings.push(LintWarning {
            rule: "WEB_FONT_IMPORT",
            severity: "medium",
            message: "Outlook ignores @import @font-face. Declare a system-font fallback.",
            byte_offset: Some(pos),
            affects: &["Outlook Desktop"],
        });
    }

    // Rule 5: <link rel="stylesheet"> — almost always stripped.
    if let Some(pos) = lower.find("rel=\"stylesheet\"")
        .or_else(|| lower.find("rel='stylesheet'"))
        .or_else(|| lower.find("rel=stylesheet"))
    {
        warnings.push(LintWarning {
            rule: "EXTERNAL_STYLESHEET",
            severity: "high",
            message: "External stylesheets are not loaded by most email clients. Inline the CSS.",
            byte_offset: Some(pos),
            affects: &["Gmail Web", "Gmail iOS", "Outlook Desktop", "Outlook Web", "Yahoo Mail"],
        });
    }

    // Rule 6: <script>.
    if let Some(pos) = lower.find("<script") {
        warnings.push(LintWarning {
            rule: "SCRIPT_TAG",
            severity: "high",
            message: "JavaScript is stripped by every major email client. Remove <script> tags.",
            byte_offset: Some(pos),
            affects: &["All clients"],
        });
    }

    // Rule 7: <video> / <audio>.
    if lower.contains("<video") || lower.contains("<audio") {
        warnings.push(LintWarning {
            rule: "MEDIA_TAG",
            severity: "medium",
            message: "<video>/<audio> are not supported in most clients. Use a static preview image.",
            byte_offset: None,
            affects: &["Outlook Desktop", "Outlook Web", "Yahoo Mail"],
        });
    }

    // Rule 8: position: absolute / fixed.
    if lower.contains("position: absolute") || lower.contains("position:absolute") {
        warnings.push(LintWarning {
            rule: "POSITION_ABSOLUTE",
            severity: "high",
            message: "Absolute positioning is unreliable across clients; use tables for layout.",
            byte_offset: None,
            affects: &["Outlook Desktop", "Outlook Web", "Gmail Web"],
        });
    }

    LintReport { warnings }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn style_in_body_flagged() {
        let html = "<html><body><style>.x{}</style></body></html>";
        let r = lint(html);
        assert!(r.warnings.iter().any(|w| w.rule == "STYLE_IN_BODY"));
    }

    #[test]
    fn no_warnings_for_clean_html() {
        let html = "<html><head><style>.x{color:red}</style></head><body><p>hi</p></body></html>";
        let r = lint(html);
        assert!(r.warnings.is_empty(), "got {:?}", r.warnings);
    }

    #[test]
    fn script_flagged_high() {
        let html = "<body><script>alert(1)</script></body>";
        let r = lint(html);
        let w = r.warnings.iter().find(|w| w.rule == "SCRIPT_TAG").expect("script warning");
        assert_eq!(w.severity, "high");
    }
}
