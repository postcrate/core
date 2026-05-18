//! Smoke test for `Service::release_email`. We point the relay at
//! another ephemeral mailbox on the same engine, release an email,
//! and assert the relay target captured a fresh copy.

mod common;

use std::time::Duration;

use common::{quick_send, TestService};
use postcrate_core::RelayConfig;

#[tokio::test(flavor = "multi_thread")]
async fn release_routes_to_relay_mailbox() {
    let ts = TestService::boot().await;

    let captured_inbox = ts.create_ephemeral(60).await;
    let relay_inbox = ts.create_ephemeral(60).await;

    // Land an email in `captured_inbox`.
    quick_send(
        &captured_inbox.host,
        captured_inbox.port,
        "alice@example.com",
        "bob@example.com",
        "to be released",
        "this is the original body",
    )
    .await
    .unwrap();

    // Wait for ingest.
    let id = loop {
        let s = ts.service.list_emails(&captured_inbox.id, 10, 0).await.unwrap();
        if let Some(s0) = s.first() {
            break s0.id.clone();
        }
        tokio::time::sleep(Duration::from_millis(20)).await;
    };

    // Release it through `relay_inbox`'s listener.
    let relay = RelayConfig {
        host: relay_inbox.host.clone(),
        port: relay_inbox.port,
        timeout_seconds: Some(5),
    };
    ts.service
        .release_email(&id, "downstream@real.example", &relay)
        .await
        .expect("release");

    // Assert the relay mailbox captured a new email.
    let deadline = std::time::Instant::now() + Duration::from_secs(5);
    loop {
        let s = ts.service.list_emails(&relay_inbox.id, 10, 0).await.unwrap();
        if let Some(s0) = s.first() {
            assert_eq!(s0.to, vec!["downstream@real.example".to_string()]);
            assert_eq!(s0.from, "alice@example.com");
            assert_eq!(s0.subject.as_deref(), Some("to be released"));
            return;
        }
        if std::time::Instant::now() > deadline {
            panic!("relay mailbox never received the released email");
        }
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
}
