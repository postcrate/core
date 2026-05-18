#![no_main]

//! Fuzz the SMTP command-line parser. The contract is: never panic
//! regardless of the input bytes. The parser is wired into every
//! connection on `smtp/session.rs`, so panics here would translate
//! to denial-of-service.

use libfuzzer_sys::fuzz_target;

fuzz_target!(|data: &[u8]| {
    // SmtpCommand::parse takes a &str; lossy UTF-8 is fine — real
    // clients send 7-bit ASCII, but our parser must tolerate junk.
    let input = String::from_utf8_lossy(data);
    let _ = postcrate_core::fuzz::parse_smtp_command(&input);
});
