//! Per-client profile transforms.
//!
//! Each profile takes an HTML blob and returns it with the client's
//! known limitations applied. The transforms are deterministic
//! regex/HTML rewrites — no headless browser, no network access.
//!
//! When adding a new profile, extend the test coverage in this file
//! and update the profile's `fidelity` so callers can label previews
//! honestly. A profile that lies about what it simulates is worse
//! than no profile.

use regex::Regex;
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "specta", derive(specta::Type))]
#[serde(rename_all = "snake_case")]
pub enum Profile {
    GmailWeb,
    GmailIos,
    OutlookDesktop,
    OutlookWeb,
    AppleMailMac,
    AppleMailIos,
    YahooMail,
}

impl Profile {
    pub fn name(self) -> &'static str {
        match self {
            Profile::GmailWeb => "Gmail Web",
            Profile::GmailIos => "Gmail iOS",
            Profile::OutlookDesktop => "Outlook Desktop (Windows)",
            Profile::OutlookWeb => "Outlook Web",
            Profile::AppleMailMac => "Apple Mail (macOS)",
            Profile::AppleMailIos => "Apple Mail (iOS)",
            Profile::YahooMail => "Yahoo Mail",
        }
    }

    /// Honest fidelity report.
    pub fn fidelity(self) -> Fidelity {
        match self {
            // Apple Mail uses WebKit; render is closest to a stock browser.
            Profile::AppleMailMac | Profile::AppleMailIos => Fidelity::High,
            // Gmail strips `<style>` and rewrites `class` attributes.
            // We approximate the common cases.
            Profile::GmailWeb | Profile::GmailIos => Fidelity::Approximate,
            // Outlook Desktop is the hardest to simulate: it uses
            // Word's rendering engine. Mark experimental.
            Profile::OutlookDesktop => Fidelity::Experimental,
            // Outlook Web is closer to a real browser, but `<style>`
            // and CSS support is famously inconsistent.
            Profile::OutlookWeb => Fidelity::Approximate,
            // Yahoo: similar limitations to Outlook Web.
            Profile::YahooMail => Fidelity::Approximate,
        }
    }
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize)]
#[cfg_attr(feature = "specta", derive(specta::Type))]
#[serde(rename_all = "lowercase")]
pub enum Fidelity {
    High,
    Approximate,
    Experimental,
}

#[derive(Debug, Clone, Serialize)]
#[cfg_attr(feature = "specta", derive(specta::Type))]
#[serde(rename_all = "camelCase")]
pub struct RenderedPreview {
    pub profile: Profile,
    pub fidelity: Fidelity,
    pub html: String,
    /// Notes about transforms that ran — useful for "why does it
    /// look different here?" tooltips in the UI.
    pub applied: Vec<&'static str>,
}

/// Apply `profile` to `html`. Idempotent; safe to run multiple times.
pub fn apply(html: &str, profile: Profile) -> RenderedPreview {
    let (out, applied) = match profile {
        Profile::GmailWeb | Profile::GmailIos => transform_gmail(html),
        Profile::OutlookDesktop => transform_outlook_desktop(html),
        Profile::OutlookWeb => transform_outlook_web(html),
        Profile::AppleMailMac | Profile::AppleMailIos => transform_apple(html),
        Profile::YahooMail => transform_yahoo(html),
    };
    RenderedPreview {
        profile,
        fidelity: profile.fidelity(),
        html: out,
        applied,
    }
}

fn transform_gmail(html: &str) -> (String, Vec<&'static str>) {
    let mut out = html.to_string();
    let mut notes: Vec<&'static str> = Vec::new();

    // Gmail strips `<style>` blocks inside `<body>`.
    if has_style_in_body(&out) {
        out = strip_style_in_body(&out);
        notes.push("style-in-body stripped (Gmail removes inline <style> blocks)");
    }
    // Gmail strips CSS Grid: replace `display: grid` with `display: block`.
    if out.contains("display: grid") || out.contains("display:grid") {
        out = out.replace("display: grid", "display: block");
        out = out.replace("display:grid", "display:block");
        notes.push("CSS Grid replaced with block (Gmail strips grid)");
    }
    (out, notes)
}

fn transform_outlook_desktop(html: &str) -> (String, Vec<&'static str>) {
    let mut out = html.to_string();
    let mut notes: Vec<&'static str> = Vec::new();

    // Outlook desktop ignores most modern CSS — strip <style> blocks
    // entirely so the preview falls back to inline + table layout.
    if has_style_tag(&out) {
        out = strip_all_style_tags(&out);
        notes.push("All <style> blocks stripped (Outlook Desktop uses Word renderer)");
    }
    // Web fonts: Outlook ignores @font-face. Force a stock fallback
    // by removing font-family declarations that look like web fonts.
    out = strip_webfont_imports(&out);
    notes.push("Web fonts ignored (Outlook falls back to system fonts)");

    // CSS Grid + Flexbox: unsupported. Same trick as Gmail.
    if out.contains("display: grid") || out.contains("display: flex") {
        out = out.replace("display: grid", "display: block");
        out = out.replace("display:grid", "display:block");
        out = out.replace("display: flex", "display: block");
        out = out.replace("display:flex", "display:block");
        notes.push("Grid/Flex replaced with block (Outlook only supports tables)");
    }
    (out, notes)
}

fn transform_outlook_web(html: &str) -> (String, Vec<&'static str>) {
    let mut out = html.to_string();
    let mut notes: Vec<&'static str> = Vec::new();
    if has_style_in_body(&out) {
        out = strip_style_in_body(&out);
        notes.push("style-in-body stripped (Outlook Web rewrites these)");
    }
    (out, notes)
}

fn transform_apple(html: &str) -> (String, Vec<&'static str>) {
    // Apple Mail uses WebKit; passes most modern CSS. Light touch:
    // surface no notes when nothing changes.
    (html.to_string(), Vec::new())
}

fn transform_yahoo(html: &str) -> (String, Vec<&'static str>) {
    let mut out = html.to_string();
    let mut notes: Vec<&'static str> = Vec::new();
    if has_style_in_body(&out) {
        out = strip_style_in_body(&out);
        notes.push("style-in-body stripped (Yahoo rewrites these)");
    }
    (out, notes)
}

// ---- helpers ------------------------------------------------------

fn style_tag_regex() -> &'static Regex {
    static R: std::sync::OnceLock<Regex> = std::sync::OnceLock::new();
    R.get_or_init(|| Regex::new(r"(?is)<style\b[^>]*>.*?</style>").unwrap())
}

fn body_open_regex() -> &'static Regex {
    static R: std::sync::OnceLock<Regex> = std::sync::OnceLock::new();
    R.get_or_init(|| Regex::new(r"(?is)<body\b[^>]*>").unwrap())
}

fn has_style_tag(s: &str) -> bool {
    style_tag_regex().is_match(s)
}

fn has_style_in_body(html: &str) -> bool {
    let Some(body_open) = body_open_regex().find(html) else {
        // No <body>; treat any <style> as "in body".
        return has_style_tag(html);
    };
    let after_body = &html[body_open.end()..];
    has_style_tag(after_body)
}

fn strip_all_style_tags(html: &str) -> String {
    style_tag_regex().replace_all(html, "").into_owned()
}

fn strip_style_in_body(html: &str) -> String {
    let Some(body_open) = body_open_regex().find(html) else {
        return strip_all_style_tags(html);
    };
    let split = body_open.end();
    let (head, body) = html.split_at(split);
    let body = style_tag_regex().replace_all(body, "").into_owned();
    format!("{head}{body}")
}

fn strip_webfont_imports(html: &str) -> String {
    static R: std::sync::OnceLock<Regex> = std::sync::OnceLock::new();
    let re = R.get_or_init(|| {
        Regex::new(r#"(?is)@import\s+url\(['"]?https?://[^)'"]*['"]?\);?"#).unwrap()
    });
    re.replace_all(html, "").into_owned()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn gmail_strips_style_in_body() {
        let html = r#"<html><head></head><body><style>.x{color:red}</style><p>hi</p></body></html>"#;
        let r = apply(html, Profile::GmailWeb);
        assert!(!r.html.contains("<style>"));
        assert!(!r.applied.is_empty());
    }

    #[test]
    fn gmail_preserves_style_in_head() {
        let html = r#"<html><head><style>.x{color:red}</style></head><body><p>hi</p></body></html>"#;
        let r = apply(html, Profile::GmailWeb);
        assert!(r.html.contains("<style>"));
        assert!(r.applied.is_empty());
    }

    #[test]
    fn outlook_strips_all_styles() {
        let html = r#"<html><head><style>.x{}</style></head><body><p>hi</p></body></html>"#;
        let r = apply(html, Profile::OutlookDesktop);
        assert!(!r.html.contains("<style>"));
    }

    #[test]
    fn outlook_replaces_grid() {
        let html = r#"<div style="display: grid">x</div>"#;
        let r = apply(html, Profile::OutlookDesktop);
        assert!(r.html.contains("display: block"));
    }

    #[test]
    fn apple_passes_through() {
        let html = r#"<div style="display: grid">x</div>"#;
        let r = apply(html, Profile::AppleMailMac);
        assert_eq!(r.html, html);
        assert!(r.applied.is_empty());
    }
}
