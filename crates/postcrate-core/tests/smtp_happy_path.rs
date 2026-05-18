//! End-to-end SMTP happy-path test. Boot the engine, create an
//! ephemeral mailbox, send a plain text message, confirm it lands.

mod common;

use std::time::Duration;

use common::{quick_send, TestService};

#[tokio::test(flavor = "multi_thread")]
async fn captures_a_plain_text_email() {
    let ts = TestService::boot().await;
    let eph = ts.create_ephemeral(60).await;

    quick_send(
        &eph.host,
        eph.port,
        "alice@example.com",
        "bob@example.com",
        "hello postcrate",
        "this is the body",
    )
    .await
    .expect("send");

    // The ingest pipeline is async — poll briefly.
    let summaries = wait_for_emails(&ts, &eph.id, 1, Duration::from_secs(5)).await;
    assert_eq!(summaries.len(), 1);
    let s = &summaries[0];
    assert_eq!(s.from, "alice@example.com");
    assert_eq!(s.to, vec!["bob@example.com".to_string()]);
    assert_eq!(s.subject.as_deref(), Some("hello postcrate"));
    assert!(s.has_text);

    let detail = ts.service.get_email(&s.id).await.expect("detail");
    assert!(detail
        .text_body
        .as_deref()
        .unwrap_or("")
        .contains("this is the body"));
}

#[tokio::test(flavor = "multi_thread")]
async fn multiple_recipients_in_one_envelope() {
    let ts = TestService::boot().await;
    let eph = ts.create_ephemeral(60).await;

    let mut c = common::SmtpClient::connect(&eph.host, eph.port).await.unwrap();
    let _ = c.read_reply().await.unwrap();
    c.send("EHLO test").await.unwrap();
    let _ = c.read_reply().await.unwrap();
    c.send("MAIL FROM:<a@b>").await.unwrap();
    let _ = c.read_reply().await.unwrap();
    c.send("RCPT TO:<c@d>").await.unwrap();
    let _ = c.read_reply().await.unwrap();
    c.send("RCPT TO:<e@f>").await.unwrap();
    let _ = c.read_reply().await.unwrap();
    c.send("RCPT TO:<g@h>").await.unwrap();
    let _ = c.read_reply().await.unwrap();
    let raw =
        b"From: a@b\r\nTo: c@d, e@f, g@h\r\nSubject: multi\r\nDate: Mon, 1 Jan 2024 00:00:00 +0000\r\n\r\nhi all";
    let reply = c.send_data(raw).await.unwrap();
    assert!(reply[0].starts_with("250"));
    c.quit().await.unwrap();

    let summaries = wait_for_emails(&ts, &eph.id, 1, Duration::from_secs(5)).await;
    assert_eq!(summaries.len(), 1);
    assert_eq!(summaries[0].to.len(), 3);
}

#[tokio::test(flavor = "multi_thread")]
async fn rset_clears_envelope() {
    let ts = TestService::boot().await;
    let eph = ts.create_ephemeral(60).await;

    let mut c = common::SmtpClient::connect(&eph.host, eph.port).await.unwrap();
    let _ = c.read_reply().await.unwrap();
    c.send("EHLO test").await.unwrap();
    let _ = c.read_reply().await.unwrap();
    c.send("MAIL FROM:<a@b>").await.unwrap();
    let _ = c.read_reply().await.unwrap();
    c.send("RCPT TO:<c@d>").await.unwrap();
    let _ = c.read_reply().await.unwrap();
    c.send("RSET").await.unwrap();
    let reset_reply = c.read_reply().await.unwrap();
    assert!(reset_reply[0].starts_with("250"));

    // After RSET, RCPT without MAIL FROM should be a bad sequence.
    c.send("RCPT TO:<c@d>").await.unwrap();
    let bad = c.read_reply().await.unwrap();
    assert!(
        bad[0].starts_with("503"),
        "expected 503 after RSET; got {bad:?}"
    );
    c.quit().await.unwrap();

    // No emails captured.
    tokio::time::sleep(Duration::from_millis(200)).await;
    let s = ts.service.list_emails(&eph.id, 100, 0).await.unwrap();
    assert!(s.is_empty(), "expected no emails after RSET-only session");
}

async fn wait_for_emails(
    ts: &TestService,
    mailbox: &str,
    expected: usize,
    deadline: Duration,
) -> Vec<postcrate_core::EmailSummary> {
    let start = std::time::Instant::now();
    loop {
        let s = ts.service.list_emails(mailbox, 1000, 0).await.unwrap();
        if s.len() >= expected || start.elapsed() >= deadline {
            return s;
        }
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
}
