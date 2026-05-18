//! Smoke test for `GET /api/v1/events` (SSE). Asserts:
//!   1. Content-Type is `text/event-stream`.
//!   2. Sending an email produces a `newEmail` event on the stream.

mod common;

use std::time::Duration;

use common::{quick_send, TestService};
use futures_util::StreamExt;

#[tokio::test(flavor = "multi_thread")]
async fn sse_emits_new_email_event() {
    let ts = TestService::boot().await;
    let eph = ts.create_ephemeral(60).await;

    let url = format!("{}/api/v1/events", ts.http_url);
    let client = reqwest::Client::builder()
        .timeout(Duration::from_secs(10))
        .build()
        .unwrap();
    let resp = client.get(&url).send().await.expect("connect SSE");
    assert!(resp.status().is_success(), "status {}", resp.status());
    let ct = resp
        .headers()
        .get(reqwest::header::CONTENT_TYPE)
        .and_then(|v| v.to_str().ok())
        .unwrap_or("");
    assert!(
        ct.starts_with("text/event-stream"),
        "wrong content-type: {ct}"
    );

    // Give the server task a tick to subscribe before we send.
    tokio::time::sleep(Duration::from_millis(50)).await;

    let host = eph.host.clone();
    let port = eph.port;
    tokio::spawn(async move {
        let _ = quick_send(
            &host,
            port,
            "alice@example.com",
            "bob@example.com",
            "SSE test",
            "body",
        )
        .await;
    });

    // Consume chunks until we see `event: newEmail`.
    let mut stream = resp.bytes_stream();
    let mut buffer = String::new();
    let deadline = tokio::time::Instant::now() + Duration::from_secs(5);
    while tokio::time::Instant::now() < deadline {
        let chunk = tokio::time::timeout(Duration::from_millis(500), stream.next()).await;
        match chunk {
            Ok(Some(Ok(bytes))) => {
                buffer.push_str(&String::from_utf8_lossy(&bytes));
                if buffer.contains("event: newEmail") {
                    return; // pass
                }
            }
            Ok(Some(Err(e))) => panic!("stream error: {e}"),
            Ok(None) => panic!("stream ended early; buffer={buffer}"),
            Err(_) => {} // tick — keep waiting
        }
    }
    panic!("no newEmail event within 5s; got:\n{buffer}");
}
