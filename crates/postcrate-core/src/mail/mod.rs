//! RFC 5322 message parsing (via `mail-parser`) and RFC 5321 path
//! parsing (hand-rolled). Kept thin: produces JSON-friendly shapes so the
//! storage layer can serialize without needing the upstream type.

pub mod address;
pub mod headers;
pub mod parse;
