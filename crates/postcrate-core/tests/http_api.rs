//! HTTP API smoke tests. Drives the `/api/v1/...` surface end-to-end,
//! including the new `/audit` endpoints from A1.8.

mod common;

use std::time::Duration;

use common::{quick_send, TestService};
use serde_json::Value;

fn client() -> reqwest::Client {
    reqwest::Client::builder()
        .timeout(Duration::from_secs(5))
        .build()
        .unwrap()
}

#[tokio::test(flavor = "multi_thread")]
async fn healthz_responds() {
    let ts = TestService::boot().await;
    let resp = client()
        .get(format!("{}/healthz", ts.http_url))
        .send()
        .await
        .unwrap();
    assert!(resp.status().is_success(), "status {}", resp.status());
}

#[tokio::test(flavor = "multi_thread")]
async fn mailbox_lifecycle_via_http() {
    let ts = TestService::boot().await;
    let url = &ts.http_url;
    let c = client();

    // List — initially empty for `test` project.
    let mailboxes: Value = c
        .get(format!("{url}/api/v1/mailboxes?projectId=test"))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert!(mailboxes.as_array().map_or(false, |a| a.is_empty()));

    // Create ephemeral.
    let eph: Value = c
        .post(format!("{url}/api/v1/mailboxes/ephemeral"))
        .json(&serde_json::json!({
            "projectId": "test",
            "ttlSeconds": 60
        }))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    let port = eph.get("port").and_then(|v| v.as_u64()).expect("port") as u16;
    let host = eph
        .get("host")
        .and_then(|v| v.as_str())
        .expect("host")
        .to_string();
    let id = eph.get("id").and_then(|v| v.as_str()).expect("id").to_string();

    // Send one mail.
    quick_send(&host, port, "a@b", "c@d", "via http", "body").await.unwrap();

    // Poll the messages endpoint.
    let start = std::time::Instant::now();
    let summary = loop {
        let resp: Value = c
            .get(format!("{url}/api/v1/messages?mailboxId={id}"))
            .send()
            .await
            .unwrap()
            .json()
            .await
            .unwrap();
        let arr = resp.as_array().cloned().unwrap_or_default();
        if !arr.is_empty() {
            break arr[0].clone();
        }
        if start.elapsed() > Duration::from_secs(5) {
            panic!("no messages within 5s");
        }
        tokio::time::sleep(Duration::from_millis(20)).await;
    };
    let msg_id = summary.get("id").and_then(|v| v.as_str()).unwrap().to_string();

    // Get detail.
    let detail: Value = c
        .get(format!("{url}/api/v1/messages/{msg_id}"))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert_eq!(detail.get("subject").and_then(|v| v.as_str()), Some("via http"));

    // Get raw.
    let raw_resp = c
        .get(format!("{url}/api/v1/messages/{msg_id}/raw"))
        .send()
        .await
        .unwrap();
    assert!(raw_resp.status().is_success());
    let ct = raw_resp
        .headers()
        .get(reqwest::header::CONTENT_TYPE)
        .and_then(|v| v.to_str().ok())
        .unwrap_or("");
    assert_eq!(ct, "message/rfc822");

    // Mark read.
    let r: Value = c
        .post(format!("{url}/api/v1/messages/{msg_id}/read"))
        .json(&serde_json::json!({"read": true}))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert_eq!(r.get("read"), Some(&Value::Bool(true)));

    // Clear inbox.
    let cleared: Value = c
        .delete(format!("{url}/api/v1/mailboxes/{id}/messages"))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert!(cleared.get("deleted").and_then(|v| v.as_u64()).unwrap() >= 1);
}

#[tokio::test(flavor = "multi_thread")]
async fn audit_list_and_clear() {
    let ts = TestService::boot().await;
    let url = &ts.http_url;
    let c = client();

    // Trigger a few audit-emitting actions: ephemeral create, clear.
    let eph: Value = c
        .post(format!("{url}/api/v1/mailboxes/ephemeral"))
        .json(&serde_json::json!({"projectId": "test", "ttlSeconds": 60}))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    let id = eph.get("id").and_then(|v| v.as_str()).unwrap().to_string();
    c.delete(format!("{url}/api/v1/mailboxes/{id}/messages"))
        .send()
        .await
        .unwrap();

    // List audit.
    let audit: Value = c
        .get(format!("{url}/api/v1/audit"))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    let arr = audit.as_array().expect("array");
    assert!(arr.len() >= 2, "expected at least 2 audit entries, got {}", arr.len());
    let actions: Vec<&str> = arr
        .iter()
        .filter_map(|e| e.get("action").and_then(|v| v.as_str()))
        .collect();
    assert!(actions.iter().any(|a| *a == "mailbox.ephemeral.create"));
    assert!(actions.iter().any(|a| *a == "mailbox.clear"));

    // Clear audit.
    let cleared: Value = c
        .delete(format!("{url}/api/v1/audit"))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert!(cleared.get("deleted").and_then(|v| v.as_u64()).unwrap() >= 2);

    // Subsequent list is empty.
    let audit2: Value = c
        .get(format!("{url}/api/v1/audit"))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert!(audit2.as_array().map_or(false, |a| a.is_empty()));
}

#[tokio::test(flavor = "multi_thread")]
async fn audit_older_than_days_is_a_no_op_for_recent() {
    let ts = TestService::boot().await;
    let url = &ts.http_url;
    let c = client();

    let _eph: Value = c
        .post(format!("{url}/api/v1/mailboxes/ephemeral"))
        .json(&serde_json::json!({"projectId": "test", "ttlSeconds": 60}))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();

    // Recent entries should NOT be pruned by `olderThanDays=30`.
    let cleared: Value = c
        .delete(format!("{url}/api/v1/audit?olderThanDays=30"))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert_eq!(cleared.get("deleted"), Some(&Value::from(0u64)));

    let audit: Value = c
        .get(format!("{url}/api/v1/audit"))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert!(!audit.as_array().unwrap().is_empty());
}
