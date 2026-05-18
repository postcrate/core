//! ESMTP extension behavior: EHLO advertisement, SIZE check, 8BITMIME,
//! SMTPUTF8 flag round-trip, line length limit, VRFY/HELP.

mod common;

use std::time::Duration;

use common::TestService;

#[tokio::test(flavor = "multi_thread")]
async fn ehlo_advertises_expected_extensions() {
    let ts = TestService::boot().await;
    let eph = ts.create_ephemeral(60).await;

    let mut c = common::SmtpClient::connect(&eph.host, eph.port).await.unwrap();
    let banner = c.read_reply().await.unwrap();
    assert!(banner[0].starts_with("220"));

    c.send("EHLO foo.example").await.unwrap();
    let reply = c.read_reply().await.unwrap();
    let joined = reply.join("\n");
    // Each capability appears on its own line; we just need the keyword present.
    for cap in [
        "PIPELINING",
        "SIZE",
        "8BITMIME",
        "SMTPUTF8",
        "ENHANCEDSTATUSCODES",
        "HELP",
    ] {
        assert!(joined.contains(cap), "missing {cap} in {joined}");
    }
    // Without --features tls, STARTTLS must NOT be advertised.
    #[cfg(not(feature = "tls"))]
    assert!(!joined.contains("STARTTLS"), "STARTTLS leaked without feature");
}

#[tokio::test(flavor = "multi_thread")]
async fn helo_works_for_legacy_clients() {
    let ts = TestService::boot().await;
    let eph = ts.create_ephemeral(60).await;

    let mut c = common::SmtpClient::connect(&eph.host, eph.port).await.unwrap();
    let _ = c.read_reply().await.unwrap();
    c.send("HELO foo").await.unwrap();
    let reply = c.read_reply().await.unwrap();
    assert!(reply[0].starts_with("250"), "got {reply:?}");
}

#[tokio::test(flavor = "multi_thread")]
async fn size_envelope_exceeds_max() {
    let ts = TestService::boot_with(|cfg| cfg.max_message_bytes = 1024).await;
    let eph = ts.create_ephemeral(60).await;

    let mut c = common::SmtpClient::connect(&eph.host, eph.port).await.unwrap();
    let _ = c.read_reply().await.unwrap();
    c.send("EHLO test").await.unwrap();
    let _ = c.read_reply().await.unwrap();
    // Declare a SIZE larger than the limit.
    c.send("MAIL FROM:<a@b> SIZE=99999999").await.unwrap();
    let reply = c.read_reply().await.unwrap();
    assert!(reply[0].starts_with("552"), "got {reply:?}");
}

#[tokio::test(flavor = "multi_thread")]
async fn smtputf8_flag_recorded_on_envelope() {
    let ts = TestService::boot().await;
    let eph = ts.create_ephemeral(60).await;

    let mut c = common::SmtpClient::connect(&eph.host, eph.port).await.unwrap();
    let _ = c.read_reply().await.unwrap();
    c.send("EHLO test").await.unwrap();
    let _ = c.read_reply().await.unwrap();
    c.send("MAIL FROM:<a@b> SMTPUTF8 BODY=8BITMIME").await.unwrap();
    let _ = c.read_reply().await.unwrap();
    c.send("RCPT TO:<c@d>").await.unwrap();
    let _ = c.read_reply().await.unwrap();
    let raw = b"From: a@b\r\nTo: c@d\r\nSubject: utf8\r\nDate: Mon, 1 Jan 2024 00:00:00 +0000\r\n\r\nbody";
    let reply = c.send_data(raw).await.unwrap();
    assert!(reply[0].starts_with("250"));
    c.quit().await.unwrap();

    // Verify flags persisted.
    let summaries = poll_emails(&ts, &eph.id, 1, Duration::from_secs(5)).await;
    let detail = ts.service.get_email(&summaries[0].id).await.unwrap();
    assert!(detail.ext_smtputf8, "smtputf8 flag should be set");
    assert!(detail.ext_8bitmime, "8bitmime flag should be set");
}

#[tokio::test(flavor = "multi_thread")]
async fn vrfy_returns_252() {
    let ts = TestService::boot().await;
    let eph = ts.create_ephemeral(60).await;
    let mut c = common::SmtpClient::connect(&eph.host, eph.port).await.unwrap();
    let _ = c.read_reply().await.unwrap();
    c.send("VRFY postmaster").await.unwrap();
    let reply = c.read_reply().await.unwrap();
    assert!(reply[0].starts_with("252"), "got {reply:?}");
}

#[tokio::test(flavor = "multi_thread")]
async fn line_too_long_500() {
    let ts = TestService::boot().await;
    let eph = ts.create_ephemeral(60).await;
    let mut c = common::SmtpClient::connect(&eph.host, eph.port).await.unwrap();
    let _ = c.read_reply().await.unwrap();
    // SMTP max line is 1000 incl CRLF — send 2000 chars.
    let mut huge = String::from("EHLO ");
    huge.extend(std::iter::repeat('a').take(2000));
    c.send(&huge).await.unwrap();
    let reply = c.read_reply().await;
    // The server may surface a 500 line-too-long, or close the socket.
    match reply {
        Ok(r) => assert!(r[0].starts_with("500"), "got {r:?}"),
        Err(_) => {} // socket closed — acceptable
    }
}

#[tokio::test(flavor = "multi_thread")]
async fn bad_sequence_rcpt_before_mail() {
    let ts = TestService::boot().await;
    let eph = ts.create_ephemeral(60).await;
    let mut c = common::SmtpClient::connect(&eph.host, eph.port).await.unwrap();
    let _ = c.read_reply().await.unwrap();
    c.send("EHLO test").await.unwrap();
    let _ = c.read_reply().await.unwrap();
    c.send("RCPT TO:<x@y>").await.unwrap();
    let r = c.read_reply().await.unwrap();
    assert!(r[0].starts_with("503"), "got {r:?}");
}

#[tokio::test(flavor = "multi_thread")]
async fn null_sender_accepted() {
    let ts = TestService::boot().await;
    let eph = ts.create_ephemeral(60).await;
    let mut c = common::SmtpClient::connect(&eph.host, eph.port).await.unwrap();
    let _ = c.read_reply().await.unwrap();
    c.send("EHLO test").await.unwrap();
    let _ = c.read_reply().await.unwrap();
    c.send("MAIL FROM:<>").await.unwrap();
    let r = c.read_reply().await.unwrap();
    assert!(r[0].starts_with("250"), "got {r:?}");
}

async fn poll_emails(
    ts: &TestService,
    mb: &str,
    n: usize,
    timeout: Duration,
) -> Vec<postcrate_core::EmailSummary> {
    let start = std::time::Instant::now();
    loop {
        let s = ts.service.list_emails(mb, 1000, 0).await.unwrap();
        if s.len() >= n || start.elapsed() >= timeout {
            return s;
        }
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
}
