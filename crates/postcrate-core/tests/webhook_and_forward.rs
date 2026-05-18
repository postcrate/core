//! Outbound webhook + auto-forwarding integration tests.

mod common;

use std::net::SocketAddr;
use std::sync::Arc;
use std::time::Duration;

use axum::extract::State;
use axum::routing::post;
use axum::{Json, Router};
use common::{quick_send, TestService};
use parking_lot::Mutex;
use postcrate_core::{CreateForwardingRule, CreateWebhook, RelayConfig};
use tokio::sync::Notify;

#[derive(Clone, Default)]
struct CollectingState {
    payloads: Arc<Mutex<Vec<serde_json::Value>>>,
    notify: Arc<Notify>,
}

async fn collect_handler(
    State(state): State<CollectingState>,
    Json(body): Json<serde_json::Value>,
) -> &'static str {
    state.payloads.lock().push(body);
    state.notify.notify_one();
    "ok"
}

async fn spawn_mock_webhook() -> (SocketAddr, CollectingState) {
    let state = CollectingState::default();
    let app = Router::new()
        .route("/hook", post(collect_handler))
        .with_state(state.clone());
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    tokio::spawn(async move {
        axum::serve(listener, app).await.unwrap();
    });
    (addr, state)
}

#[tokio::test(flavor = "multi_thread")]
async fn webhook_fires_on_new_email() {
    let (mock_addr, state) = spawn_mock_webhook().await;
    let ts = TestService::boot().await;
    let eph = ts.create_ephemeral(60).await;

    ts.service
        .create_webhook(CreateWebhook {
            mailbox_id: Some(eph.id.clone()),
            url: format!("http://{mock_addr}/hook"),
            auth_header: Some("Bearer test-token".into()),
            enabled: Some(true),
        })
        .await
        .unwrap();

    quick_send(
        &eph.host,
        eph.port,
        "alice@example.com",
        "bob@example.com",
        "webhook test",
        "body",
    )
    .await
    .unwrap();

    // Wait for the mock server to receive the POST.
    tokio::time::timeout(Duration::from_secs(5), state.notify.notified())
        .await
        .expect("webhook never fired");

    let payloads = state.payloads.lock().clone();
    assert_eq!(payloads.len(), 1);
    let p = &payloads[0];
    assert_eq!(p.get("event").and_then(|v| v.as_str()), Some("new_email"));
    assert_eq!(p.get("mailboxId").and_then(|v| v.as_str()), Some(eph.id.as_str()));
    assert_eq!(
        p.pointer("/email/subject").and_then(|v| v.as_str()),
        Some("webhook test")
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn forwarding_rule_relays_to_target_mailbox() {
    let ts = TestService::boot().await;
    let source = ts.create_ephemeral(60).await;
    let target_mb = ts.create_ephemeral(60).await;

    ts.service
        .create_forwarding_rule(CreateForwardingRule {
            mailbox_id: Some(source.id.clone()),
            target_addresses: vec!["downstream@real.example".into()],
            relay: RelayConfig {
                host: target_mb.host.clone(),
                port: target_mb.port,
                timeout_seconds: Some(5),
                allowed_recipients: None,
            },
            enabled: Some(true),
        })
        .await
        .unwrap();

    quick_send(
        &source.host,
        source.port,
        "alice@example.com",
        "bob@example.com",
        "auto-forward me",
        "body",
    )
    .await
    .unwrap();

    // The forwarded message should land in the target mailbox.
    let deadline = std::time::Instant::now() + Duration::from_secs(5);
    loop {
        let s = ts.service.list_emails(&target_mb.id, 10, 0).await.unwrap();
        if let Some(s0) = s.first() {
            assert_eq!(s0.subject.as_deref(), Some("auto-forward me"));
            assert_eq!(s0.to, vec!["downstream@real.example".to_string()]);
            return;
        }
        if std::time::Instant::now() > deadline {
            panic!("forwarding rule never fired");
        }
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
}
