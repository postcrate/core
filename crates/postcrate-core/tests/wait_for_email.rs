//! End-to-end tests for `Service::wait_for_email` and
//! `Service::assert_email_matches`, plus their HTTP routes. Exercises
//! the matcher primitive end to end against a live SMTP listener.

mod common;

use std::time::{Duration, Instant};

use common::{quick_send, TestService};
use postcrate_core::{EmailPredicate, HeaderPredicate};

#[tokio::test(flavor = "multi_thread")]
async fn wait_returns_already_present_email_immediately() {
    let ts = TestService::boot().await;
    let eph = ts.create_ephemeral(60).await;
    quick_send(&eph.host, eph.port, "alice@e.com", "bob@e.com", "Welcome", "hi").await.unwrap();

    // Poll briefly to make sure ingest has landed before we wait.
    for _ in 0..50 {
        if !ts.service.list_emails(&eph.id, 10, 0).await.unwrap().is_empty() {
            break;
        }
        tokio::time::sleep(Duration::from_millis(10)).await;
    }

    let p = EmailPredicate {
        mailbox_id: Some(eph.id.clone()),
        subject: Some("welcome".into()),
        ..Default::default()
    };
    let started = Instant::now();
    let out = ts.service.wait_for_email(p, Duration::from_secs(5)).await.unwrap();
    assert!(started.elapsed() < Duration::from_secs(1), "should return fast on pre-existing match");
    assert!(out.matched.is_some());
    assert_eq!(
        out.matched.unwrap().subject.as_deref(),
        Some("Welcome")
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn wait_unblocks_when_email_arrives() {
    let ts = TestService::boot().await;
    let eph = ts.create_ephemeral(60).await;

    let host = eph.host.clone();
    let port = eph.port;
    tokio::spawn(async move {
        tokio::time::sleep(Duration::from_millis(100)).await;
        let _ = quick_send(&host, port, "a@b", "c@d", "Verification code 123456", "body").await;
    });

    let p = EmailPredicate {
        subject_regex: Some(r"(?i)verification code \d+".into()),
        ..Default::default()
    };
    let started = Instant::now();
    let out = ts.service.wait_for_email(p, Duration::from_secs(5)).await.unwrap();
    let elapsed = started.elapsed();
    assert!(out.matched.is_some(), "no match within 5s; seen={:?}", out.seen_during_wait);
    assert!(elapsed < Duration::from_secs(2), "took too long: {:?}", elapsed);
}

#[tokio::test(flavor = "multi_thread")]
async fn wait_returns_seen_on_timeout() {
    let ts = TestService::boot().await;
    let eph = ts.create_ephemeral(60).await;

    // Send a couple of non-matching emails.
    quick_send(&eph.host, eph.port, "x@y", "u@v", "Some other thing", "body").await.unwrap();
    quick_send(&eph.host, eph.port, "x@y", "u@v", "Random subject", "body").await.unwrap();

    // Wait briefly for something that doesn't match.
    let p = EmailPredicate {
        subject: Some("nonexistent".into()),
        ..Default::default()
    };
    let out = ts.service.wait_for_email(p, Duration::from_millis(500)).await.unwrap();
    assert!(out.matched.is_none());
    assert!(
        out.seen_during_wait.len() >= 2,
        "expected to surface the unmatched emails, got {:?}",
        out.seen_during_wait
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn assert_returns_structured_mismatch() {
    let ts = TestService::boot().await;
    let eph = ts.create_ephemeral(60).await;
    quick_send(&eph.host, eph.port, "alice@e.com", "bob@e.com", "Order #42", "shipped today").await.unwrap();

    // Poll until ingest lands.
    let id = loop {
        let s = ts.service.list_emails(&eph.id, 10, 0).await.unwrap();
        if let Some(s0) = s.first() {
            break s0.id.clone();
        }
        tokio::time::sleep(Duration::from_millis(20)).await;
    };

    let p = EmailPredicate {
        subject: Some("Refund".into()),         // mismatch
        from: Some("alice@e.com".into()),       // match
        body_contains: Some("expired".into()),  // mismatch
        ..Default::default()
    };
    let r = ts.service.assert_email_matches(&id, &p).await.unwrap();
    assert!(!r.matched);
    assert_eq!(r.mismatches.len(), 2, "got {:?}", r.mismatches);
    let joined = r.mismatches.join("\n");
    assert!(joined.contains("subject"));
    assert!(joined.contains("bodyContains") || joined.contains("body"));
}

#[tokio::test(flavor = "multi_thread")]
async fn header_predicate_matches() {
    let ts = TestService::boot().await;
    let eph = ts.create_ephemeral(60).await;

    // Send a message that includes a custom header.
    let mut c = common::SmtpClient::connect(&eph.host, eph.port).await.unwrap();
    let _ = c.read_reply().await.unwrap();
    c.send("EHLO test").await.unwrap();
    let _ = c.read_reply().await.unwrap();
    c.send("MAIL FROM:<a@b>").await.unwrap();
    let _ = c.read_reply().await.unwrap();
    c.send("RCPT TO:<c@d>").await.unwrap();
    let _ = c.read_reply().await.unwrap();
    let raw = b"From: a@b\r\nTo: c@d\r\nSubject: hdr\r\nDate: Mon, 1 Jan 2024 00:00:00 +0000\r\nX-Tracking-Id: trk-998877\r\n\r\nbody";
    c.send_data(raw).await.unwrap();
    c.quit().await.unwrap();

    // Wait briefly for ingest.
    let id = loop {
        let s = ts.service.list_emails(&eph.id, 10, 0).await.unwrap();
        if let Some(s0) = s.first() {
            break s0.id.clone();
        }
        tokio::time::sleep(Duration::from_millis(20)).await;
    };

    let mut p = EmailPredicate::default();
    p.headers.push(HeaderPredicate {
        name: "X-Tracking-Id".into(),
        contains: Some("trk-".into()),
        regex: None,
    });
    let r = ts.service.assert_email_matches(&id, &p).await.unwrap();
    assert!(r.matched, "{:?}", r.mismatches);
}
