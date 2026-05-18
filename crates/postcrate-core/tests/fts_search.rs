//! FTS5 search round-trip. Covers A1.6 — the LIKE fallback was
//! replaced with a proper MATCH against `emails_fts`, joined on the
//! UNINDEXED `email_id` column added in migration 0004.

mod common;

use std::time::Duration;

use common::{quick_send, TestService};

async fn send_with(host: &str, port: u16, subject: &str, body: &str) {
    quick_send(host, port, "alice@example.com", "bob@example.com", subject, body)
        .await
        .expect("send");
}

async fn wait_for_count(ts: &TestService, mb: &str, n: usize) {
    let deadline = std::time::Instant::now() + Duration::from_secs(5);
    loop {
        let summaries = ts.service.list_emails(mb, 1000, 0).await.unwrap();
        if summaries.len() >= n {
            return;
        }
        if std::time::Instant::now() > deadline {
            panic!("expected {n} emails; only saw {}", summaries.len());
        }
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
}

#[tokio::test(flavor = "multi_thread")]
async fn finds_by_subject_token() {
    let ts = TestService::boot().await;
    let eph = ts.create_ephemeral(60).await;
    send_with(&eph.host, eph.port, "Password Reset", "Click here to reset").await;
    send_with(&eph.host, eph.port, "Welcome", "Welcome to the service").await;
    wait_for_count(&ts, &eph.id, 2).await;

    let hits = ts
        .service
        .search_emails("password", Some(&eph.id), 100)
        .await
        .unwrap();
    assert_eq!(hits.len(), 1);
    assert_eq!(hits[0].subject.as_deref(), Some("Password Reset"));
}

#[tokio::test(flavor = "multi_thread")]
async fn finds_by_body_token() {
    let ts = TestService::boot().await;
    let eph = ts.create_ephemeral(60).await;
    send_with(&eph.host, eph.port, "Hello", "Your verification code is 123456").await;
    send_with(&eph.host, eph.port, "Goodbye", "Goodbye world").await;
    wait_for_count(&ts, &eph.id, 2).await;

    let hits = ts
        .service
        .search_emails("verification", Some(&eph.id), 100)
        .await
        .unwrap();
    assert_eq!(hits.len(), 1);
    assert_eq!(hits[0].subject.as_deref(), Some("Hello"));
}

#[tokio::test(flavor = "multi_thread")]
async fn prefix_match_finds_partial_word() {
    let ts = TestService::boot().await;
    let eph = ts.create_ephemeral(60).await;
    // Use a word that won't be present in any default fixture (sender,
    // recipient, body) so the prefix match is the only path.
    send_with(&eph.host, eph.port, "Pluvial weather report", "body").await;
    send_with(&eph.host, eph.port, "Random subject", "Bob did something").await;
    wait_for_count(&ts, &eph.id, 2).await;

    // "pluv" is a prefix of "pluvial".
    let hits = ts.service.search_emails("pluv", Some(&eph.id), 100).await.unwrap();
    assert_eq!(hits.len(), 1);
    assert!(hits[0].subject.as_deref().unwrap().contains("Pluvial"));
}

#[tokio::test(flavor = "multi_thread")]
async fn multi_word_query_is_implicit_and() {
    let ts = TestService::boot().await;
    let eph = ts.create_ephemeral(60).await;
    send_with(&eph.host, eph.port, "Order shipped", "Your package is on the way").await;
    send_with(&eph.host, eph.port, "Order received", "Thanks for your order").await;
    wait_for_count(&ts, &eph.id, 2).await;

    // Both messages contain "order"; only one contains "shipped".
    let hits = ts
        .service
        .search_emails("order shipped", Some(&eph.id), 100)
        .await
        .unwrap();
    assert_eq!(hits.len(), 1);
    assert_eq!(hits[0].subject.as_deref(), Some("Order shipped"));
}

#[tokio::test(flavor = "multi_thread")]
async fn delete_removes_from_fts_index() {
    let ts = TestService::boot().await;
    let eph = ts.create_ephemeral(60).await;
    send_with(&eph.host, eph.port, "Searchable", "findme keyword").await;
    wait_for_count(&ts, &eph.id, 1).await;

    let before = ts
        .service
        .search_emails("findme", Some(&eph.id), 100)
        .await
        .unwrap();
    assert_eq!(before.len(), 1);
    let id = before[0].id.clone();
    ts.service.delete_email(&id).await.unwrap();

    let after = ts
        .service
        .search_emails("findme", Some(&eph.id), 100)
        .await
        .unwrap();
    assert!(after.is_empty(), "FTS index still has deleted row");
}

#[tokio::test(flavor = "multi_thread")]
async fn clear_mailbox_purges_fts() {
    let ts = TestService::boot().await;
    let eph = ts.create_ephemeral(60).await;
    for i in 0..3 {
        send_with(&eph.host, eph.port, "Bulk", &format!("body {i}")).await;
    }
    wait_for_count(&ts, &eph.id, 3).await;
    assert_eq!(
        ts.service.search_emails("bulk", Some(&eph.id), 100).await.unwrap().len(),
        3
    );
    ts.service.clear_mailbox(&eph.id).await.unwrap();
    assert!(ts
        .service
        .search_emails("bulk", Some(&eph.id), 100)
        .await
        .unwrap()
        .is_empty());
}

#[tokio::test(flavor = "multi_thread")]
async fn empty_query_returns_empty() {
    let ts = TestService::boot().await;
    let eph = ts.create_ephemeral(60).await;
    send_with(&eph.host, eph.port, "Anything", "body").await;
    wait_for_count(&ts, &eph.id, 1).await;

    let hits = ts.service.search_emails("", Some(&eph.id), 100).await.unwrap();
    assert!(hits.is_empty(), "empty query must not return everything");
    // Same for whitespace-only.
    let hits = ts
        .service
        .search_emails("   ", Some(&eph.id), 100)
        .await
        .unwrap();
    assert!(hits.is_empty());
}

#[tokio::test(flavor = "multi_thread")]
async fn search_across_mailboxes_without_filter() {
    let ts = TestService::boot().await;
    let eph_a = ts.create_ephemeral(60).await;
    let eph_b = ts.create_ephemeral(60).await;
    // Use a single distinctive token (unicode61 tokenizer splits on
    // hyphens, so we keep it whole).
    send_with(&eph_a.host, eph_a.port, "Mailbox A zoomzoomzoom", "body a").await;
    send_with(&eph_b.host, eph_b.port, "Mailbox B zoomzoomzoom subject", "body b").await;
    wait_for_count(&ts, &eph_a.id, 1).await;
    wait_for_count(&ts, &eph_b.id, 1).await;

    let hits = ts.service.search_emails("zoomzoomzoom", None, 100).await.unwrap();
    assert_eq!(hits.len(), 2, "got {hits:?}");
}
