//! Bearer-token middleware on the HTTP API.

mod common;

use std::time::Duration;

use common::TestService;
use postcrate_core::{NetworkPrefs, SettingsPatch};

async fn enable_auth(ts: &TestService, token: &str) {
    let mut net = ts.service.get_settings().await.unwrap().network;
    net.api_auth_token = Some(token.into());
    ts.service.update_settings(SettingsPatch::Network(net)).await.unwrap();
    // The middleware is registered at boot; we need to restart the
    // HTTP server for it to pick up the new token. Easiest path:
    // stop + start.
    ts.service.stop_all().await.unwrap();
    ts.service.start_all().await.unwrap();
}

fn client() -> reqwest::Client {
    reqwest::Client::builder()
        .timeout(Duration::from_secs(5))
        .build()
        .unwrap()
}

#[tokio::test(flavor = "multi_thread")]
async fn no_token_means_open_api() {
    let ts = TestService::boot().await;
    let resp = client()
        .get(format!("{}/api/v1/mailboxes", ts.http_url))
        .send()
        .await
        .unwrap();
    assert!(resp.status().is_success(), "open API rejected: {}", resp.status());
}

#[tokio::test(flavor = "multi_thread")]
async fn missing_token_returns_401_when_required() {
    let ts = TestService::boot().await;
    enable_auth(&ts, "sekrit").await;
    // After restart the addr may have changed. Re-derive http_url via
    // the service.
    let addr = ts.service.http_addr().expect("addr");
    let url = format!("http://{addr}/api/v1/mailboxes");
    let resp = client().get(&url).send().await.unwrap();
    assert_eq!(resp.status().as_u16(), 401, "expected 401");
}

#[tokio::test(flavor = "multi_thread")]
async fn correct_token_passes() {
    let ts = TestService::boot().await;
    enable_auth(&ts, "sekrit-token").await;
    let addr = ts.service.http_addr().expect("addr");
    let url = format!("http://{addr}/api/v1/mailboxes");
    let resp = client()
        .get(&url)
        .header(reqwest::header::AUTHORIZATION, "Bearer sekrit-token")
        .send()
        .await
        .unwrap();
    assert!(resp.status().is_success(), "got {}", resp.status());
}

#[tokio::test(flavor = "multi_thread")]
async fn wrong_token_returns_401() {
    let ts = TestService::boot().await;
    enable_auth(&ts, "sekrit").await;
    let addr = ts.service.http_addr().expect("addr");
    let url = format!("http://{addr}/api/v1/mailboxes");
    let resp = client()
        .get(&url)
        .header(reqwest::header::AUTHORIZATION, "Bearer wrong")
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status().as_u16(), 401);
}

#[tokio::test(flavor = "multi_thread")]
async fn healthz_is_always_open() {
    let ts = TestService::boot().await;
    enable_auth(&ts, "sekrit").await;
    let addr = ts.service.http_addr().expect("addr");
    let resp = client()
        .get(format!("http://{addr}/healthz"))
        .send()
        .await
        .unwrap();
    assert!(resp.status().is_success(), "healthz blocked: {}", resp.status());
}

// Suppress unused import in this file when the test below is filtered out.
#[allow(dead_code)]
const _: Option<NetworkPrefs> = None;
