//! Export → replay round-trip for the `.postcrate` recording format.

mod common;

use std::time::Duration;

use common::{quick_send, TestService};

async fn wait_count(ts: &TestService, mb: &str, n: usize) {
    let deadline = std::time::Instant::now() + Duration::from_secs(5);
    loop {
        let s = ts.service.list_emails(mb, 100, 0).await.unwrap();
        if s.len() >= n {
            return;
        }
        if std::time::Instant::now() > deadline {
            panic!("expected {n} emails, got {}", s.len());
        }
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
}

#[tokio::test(flavor = "multi_thread")]
async fn export_then_replay_into_fresh_mailbox() {
    let ts = TestService::boot().await;
    let source = ts.create_ephemeral(60).await;
    let target = ts.create_ephemeral(60).await;

    for i in 0..3 {
        quick_send(
            &source.host,
            source.port,
            "alice@example.com",
            "bob@example.com",
            &format!("recorded {i}"),
            &format!("body {i}"),
        )
        .await
        .unwrap();
    }
    wait_count(&ts, &source.id, 3).await;

    let recording = ts
        .service
        .export_recording(&source.id, Some("fixture".into()))
        .await
        .unwrap();
    assert_eq!(recording.messages.len(), 3);
    assert_eq!(recording.label.as_deref(), Some("fixture"));

    let n = ts.service.replay_recording(&target.id, &recording).await.unwrap();
    assert_eq!(n, 3);
    wait_count(&ts, &target.id, 3).await;

    // Subjects round-trip.
    let replayed = ts.service.list_emails(&target.id, 100, 0).await.unwrap();
    let mut subs: Vec<String> = replayed
        .into_iter()
        .filter_map(|s| s.subject)
        .collect();
    subs.sort();
    assert_eq!(subs, vec!["recorded 0", "recorded 1", "recorded 2"]);
}

#[tokio::test(flavor = "multi_thread")]
async fn replay_email_uses_smtp_path() {
    let ts = TestService::boot().await;
    let source = ts.create_ephemeral(60).await;
    let target = ts.create_ephemeral(60).await;
    quick_send(
        &source.host,
        source.port,
        "alice@example.com",
        "bob@example.com",
        "send me again",
        "body",
    )
    .await
    .unwrap();
    wait_count(&ts, &source.id, 1).await;

    let id = ts.service.list_emails(&source.id, 1, 0).await.unwrap()[0]
        .id
        .clone();
    ts.service.replay_email(&id, &target.id).await.unwrap();
    wait_count(&ts, &target.id, 1).await;

    let target_emails = ts.service.list_emails(&target.id, 10, 0).await.unwrap();
    assert_eq!(target_emails[0].subject.as_deref(), Some("send me again"));
}

#[tokio::test(flavor = "multi_thread")]
async fn rejects_wrong_recording_version() {
    let ts = TestService::boot().await;
    let mb = ts.create_ephemeral(60).await;

    let bad = postcrate_core::Recording {
        version: 999,
        exported_at: 0,
        label: None,
        messages: vec![],
    };
    let res = ts.service.replay_recording(&mb.id, &bad).await;
    assert!(res.is_err());
}
