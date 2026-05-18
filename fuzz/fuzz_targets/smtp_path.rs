#![no_main]

//! Fuzz the SMTP path parser (`<addr> SIZE= BODY= SMTPUTF8 ...`).
//! Called on every MAIL FROM / RCPT TO line.

use libfuzzer_sys::fuzz_target;

fuzz_target!(|data: &[u8]| {
    let input = String::from_utf8_lossy(data);
    let _ = postcrate_core::fuzz::parse_smtp_path(&input);
});
