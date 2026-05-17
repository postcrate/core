//! Header extraction helpers — kept separate from `parse` so the
//! `mail-parser` types stay quarantined behind a small surface.

use mail_parser::{Address, HeaderValue, Message};

/// Render an Address (single, list, group) as a comma-separated display
/// string. Used for the `header_from` / `header_to` summary columns
/// where we want a single string the UI can render without thinking.
pub fn render_address(addr: &Address<'_>) -> String {
    let mut out = String::new();
    let mut first = true;

    let mut push = |display: String| {
        if !first {
            out.push_str(", ");
        }
        first = false;
        out.push_str(&display);
    };

    if let Some(list) = addr.as_list() {
        for a in list {
            let mut s = String::new();
            if let Some(name) = a.name.as_ref() {
                s.push_str(name);
                s.push(' ');
            }
            if let Some(email) = a.address.as_ref() {
                if !s.is_empty() {
                    s.push('<');
                    s.push_str(email);
                    s.push('>');
                } else {
                    s.push_str(email);
                }
            }
            if !s.is_empty() {
                push(s);
            }
        }
    } else if let Some(group_list) = addr.as_group() {
        for g in group_list {
            let mut s = String::new();
            if let Some(name) = g.name.as_ref() {
                s.push_str(name);
                s.push_str(": ");
            }
            let inner: Vec<String> = g
                .addresses
                .iter()
                .filter_map(|a| a.address.as_ref().map(|x| x.to_string()))
                .collect();
            s.push_str(&inner.join(", "));
            push(s);
        }
    }

    out
}

/// Best-effort extraction of every header on the message into a JSON map.
/// The wire format here is what the UI displays in the "Headers" tab.
pub fn headers_to_json(msg: &Message<'_>) -> serde_json::Value {
    let mut obj = serde_json::Map::new();
    for header in msg.headers() {
        let name = header.name.as_str().to_string();
        let value = match &header.value {
            HeaderValue::Text(t) => serde_json::Value::String(t.as_ref().to_string()),
            HeaderValue::TextList(list) => serde_json::Value::Array(
                list.iter()
                    .map(|t| serde_json::Value::String(t.as_ref().to_string()))
                    .collect(),
            ),
            HeaderValue::DateTime(dt) => serde_json::Value::String(dt.to_rfc822()),
            HeaderValue::Address(addr) => serde_json::Value::String(render_address(addr)),
            HeaderValue::ContentType(ct) => {
                let mut s = ct.ctype().to_string();
                if let Some(sub) = ct.subtype() {
                    s.push('/');
                    s.push_str(sub);
                }
                serde_json::Value::String(s)
            }
            HeaderValue::Received(_) | HeaderValue::Empty => serde_json::Value::Null,
        };
        // Preserve multiple instances of the same header — turn the
        // first collision into an array.
        match obj.get_mut(&name) {
            None => {
                obj.insert(name, value);
            }
            Some(existing) => {
                if existing.is_array() {
                    existing
                        .as_array_mut()
                        .expect("just checked")
                        .push(value);
                } else {
                    let prev = std::mem::replace(existing, serde_json::Value::Null);
                    *existing = serde_json::Value::Array(vec![prev, value]);
                }
            }
        }
    }
    serde_json::Value::Object(obj)
}
