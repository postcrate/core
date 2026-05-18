#![no_main]

//! Fuzz the RFC 5322 / MIME parser. Captured bytes go straight from
//! the SMTP DATA phase into `mail_parser::parse` — a panic on
//! malformed input would crash the ingest worker for *every*
//! subsequent mailbox.

use libfuzzer_sys::fuzz_target;

fuzz_target!(|data: &[u8]| {
    postcrate_core::fuzz::parse_mail(data);
});
