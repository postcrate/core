//! Parse raw RFC 5322 bytes into a `Parsed` shape ready for storage.
//!
//! We intentionally **don't** persist `mail_parser::Message` itself —
//! that type is borrowed from the input bytes and its lifetime fights
//! the async storage path. Instead we project everything we care about
//! into owned `String`/`Vec<u8>` immediately.

use mail_parser::{Address, MessageParser, MimeHeaders};

use crate::mail::headers::{headers_to_json, render_address};

#[derive(Debug, Clone)]
pub struct Parsed {
    pub header_from: Option<String>,
    pub header_to: Option<String>,
    pub header_cc: Option<String>,
    pub header_subject: Option<String>,
    pub message_id: Option<String>,
    pub in_reply_to: Option<String>,
    pub text_body: Option<String>,
    pub html_body: Option<String>,
    pub has_text: bool,
    pub has_html: bool,
    pub headers_json: serde_json::Value,
    pub attachments: Vec<ParsedAttachment>,
}

#[derive(Debug, Clone)]
pub struct ParsedAttachment {
    pub filename: Option<String>,
    pub content_type: Option<String>,
    pub content_id: Option<String>,
    pub data: Vec<u8>,
}

/// Parse the given raw bytes. Never panics — returns a mostly-empty
/// `Parsed` if the input is unrecognizable.
pub fn parse(raw: &[u8]) -> Parsed {
    let parser = MessageParser::default();
    let Some(msg) = parser.parse(raw) else {
        return Parsed {
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
            headers_json: serde_json::Value::Object(serde_json::Map::new()),
            attachments: Vec::new(),
        };
    };

    let header_from = first_address(msg.from());
    let header_to = first_address(msg.to());
    let header_cc = first_address(msg.cc());
    let header_subject = msg.subject().map(|s| s.to_string());
    let message_id = msg.message_id().map(|s| s.to_string());
    let in_reply_to = msg
        .in_reply_to()
        .as_text_list()
        .and_then(|l| l.first().map(|s| s.to_string()))
        .or_else(|| msg.in_reply_to().as_text().map(|s| s.to_string()));

    let text_body = msg.body_text(0).map(|c| c.as_ref().to_string());
    let html_body = msg.body_html(0).map(|c| c.as_ref().to_string());
    let has_text = msg.text_body_count() > 0;
    let has_html = msg.html_body_count() > 0;

    let mut attachments = Vec::with_capacity(msg.attachment_count());
    for i in 0..(msg.attachment_count() as u32) {
        let Some(att) = msg.attachment(i) else { continue };
        let filename = att
            .attachment_name()
            .map(|s| s.to_string())
            .or_else(|| att.content_type().and_then(|ct| ct.attribute("name")).map(|s| s.to_string()));
        let content_type = att.content_type().map(|ct| {
            let mut s = ct.ctype().to_string();
            if let Some(sub) = ct.subtype() {
                s.push('/');
                s.push_str(sub);
            }
            s
        });
        let content_id = att.content_id().map(|s| s.to_string());
        let data = att.contents().to_vec();
        attachments.push(ParsedAttachment {
            filename,
            content_type,
            content_id,
            data,
        });
    }

    let headers_json = headers_to_json(&msg);

    Parsed {
        header_from,
        header_to,
        header_cc,
        header_subject,
        message_id,
        in_reply_to,
        text_body,
        html_body,
        has_text,
        has_html,
        headers_json,
        attachments,
    }
}

/// Build the searchable body used for the FTS row.
pub fn fts_body(parsed: &Parsed) -> String {
    if let Some(t) = &parsed.text_body {
        return t.clone();
    }
    if let Some(h) = &parsed.html_body {
        // Crude tag-strip for indexing only. Good enough for "find that
        // password reset" — not a sanitization step.
        return strip_html(h);
    }
    String::new()
}

fn first_address(addr_opt: Option<&Address<'_>>) -> Option<String> {
    addr_opt.map(render_address).filter(|s| !s.is_empty())
}

fn strip_html(html: &str) -> String {
    let mut out = String::with_capacity(html.len() / 2);
    let mut in_tag = false;
    for c in html.chars() {
        match c {
            '<' => in_tag = true,
            '>' => in_tag = false,
            _ if !in_tag => out.push(c),
            _ => {}
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn minimal_text_message() {
        let raw = b"From: a@b.com\r\nTo: c@d.com\r\nSubject: hi\r\n\r\nhello world\r\n";
        let p = parse(raw);
        // mail-parser surfaces a default text/plain part as both a text
        // and (via its synthetic body view) an html body — that's a
        // library quirk we don't fight here. We just need the captured
        // content to be addressable.
        assert!(p.has_text);
        assert_eq!(p.header_subject.as_deref(), Some("hi"));
        assert!(p.text_body.as_deref().unwrap_or("").contains("hello"));
    }
}
