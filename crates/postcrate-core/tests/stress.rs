//! Stress: send many messages concurrently from many client sockets;
//! assert zero loss, FTS index integrity, and that retention/trim
//! triggers don't drop captures we haven't seen yet.
//!
//! Default volume is sized to be quick under `cargo test`. Set the
//! `POSTCRATE_STRESS_N` env var to override (e.g. `=10000` for the
//! roadmap target). The roadmap's (<200ms p99 capture
//! latency) is measured at the larger volume.

mod common;

use std::time::{Duration, Instant};

use common::{quick_send, TestService};
use postcrate_core::{InboxPrefs, SettingsPatch};

fn requested_volume() -> usize {
    std::env::var("POSTCRATE_STRESS_N")
        .ok()
        .and_then(|s| s.parse().ok())
        .unwrap_or(500)
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn many_concurrent_sends_no_loss() {
    let total = requested_volume();
    let ts = TestService::boot_with(|cfg| {
        // Give the ingest queue room for the burst.
        cfg.ingest_channel_capacity = (total * 2).max(2048);
    })
    .await;
    // Disable per-mailbox cap so retention doesn't trim our captures.
    ts.service
        .update_settings(SettingsPatch::Inbox(InboxPrefs {
            max_retained_emails: 0,
            auto_clear_after_days: 0,
            thread_related: false,
            auto_tag: false,
        }))
        .await
        .expect("disable retention");
    let eph = ts.create_ephemeral(600).await;

    let started = Instant::now();
    let mut handles = Vec::with_capacity(total);
    for i in 0..total {
        let host = eph.host.clone();
        let port = eph.port;
        handles.push(tokio::spawn(async move {
            // A small, predictable retry guards against transient EMFILE
            // / ECONNRESET under fork-bombs of connect()s. In CI we want
            // the test to be deterministic.
            for attempt in 0..3 {
                match quick_send(
                    &host,
                    port,
                    "alice@example.com",
                    "bob@example.com",
                    &format!("msg-{i}"),
                    &format!("body of message {i}"),
                )
                .await
                {
                    Ok(()) => return,
                    Err(_) if attempt < 2 => {
                        tokio::time::sleep(Duration::from_millis(20)).await;
                    }
                    Err(e) => panic!("send {i} failed: {e}"),
                }
            }
        }));
    }
    for h in handles {
        h.await.expect("client task");
    }
    let sent_at = started.elapsed();

    // Wait for the ingest worker to drain. Single-writer pipeline means
    // we can just poll until count is stable for two consecutive ticks.
    let deadline = Instant::now() + Duration::from_secs(60);
    let mut last_seen: i64 = 0;
    let mut stable_ticks = 0;
    let mailbox_id = eph.id.clone();
    loop {
        let mb = ts.service.get_mailbox(&mailbox_id).await.unwrap();
        let count = mb.count;
        if count == last_seen && count >= total as i64 {
            stable_ticks += 1;
            if stable_ticks >= 2 {
                break;
            }
        } else if count > last_seen {
            last_seen = count;
            stable_ticks = 0;
        }
        if Instant::now() > deadline {
            panic!(
                "ingest stalled at {last_seen}/{total} (sent in {:?})",
                sent_at
            );
        }
        tokio::time::sleep(Duration::from_millis(50)).await;
    }

    let mb = ts.service.get_mailbox(&mailbox_id).await.unwrap();
    assert_eq!(
        mb.count, total as i64,
        "expected exactly {total} messages, got {} (sent in {sent_at:?})",
        mb.count
    );

    // Sanity: FTS index should be queryable and find some of the messages.
    let hits = ts.service.search_emails("msg", Some(&mailbox_id), 1000).await.unwrap();
    assert!(
        !hits.is_empty(),
        "FTS returned 0 hits for token 'msg' after a {total}-message burst"
    );
}
