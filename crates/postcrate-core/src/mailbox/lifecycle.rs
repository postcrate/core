//! TTL scheduler. One tokio task with a min-heap of `(expires_at,
//! mailbox_id)` entries. Wakes on the earliest expiry or when a new
//! ephemeral is pushed.

use std::cmp::Reverse;
use std::collections::BinaryHeap;
use std::sync::Arc;

use tokio::sync::{mpsc, Notify};
use tokio::task::JoinHandle;
use tokio_util::sync::CancellationToken;

use crate::mailbox::service::MailboxService;

#[derive(Debug)]
pub enum ExpiryMsg {
    /// Add or replace an entry. (Replacement is achieved by pushing a
    /// new entry; expired-but-stale entries are filtered at pop time
    /// against the DB row.)
    Add { mailbox_id: String, expires_at: i64 },
    /// Remove an entry. Lazy — handled at pop time.
    Remove { mailbox_id: String },
}

pub fn spawn(
    service: Arc<MailboxService>,
    mut rx: mpsc::UnboundedReceiver<ExpiryMsg>,
    cancel: CancellationToken,
    initial: Vec<(String, i64)>,
) -> JoinHandle<()> {
    tokio::spawn(async move {
        let mut heap: BinaryHeap<Reverse<(i64, String)>> = BinaryHeap::new();
        for (id, at) in initial {
            heap.push(Reverse((at, id)));
        }
        let notify = Arc::new(Notify::new());

        loop {
            // Decide how long to sleep.
            let next = heap.peek().map(|Reverse((at, _))| *at);
            let now = chrono::Utc::now().timestamp_millis();

            tokio::select! {
                _ = cancel.cancelled() => return,
                msg = rx.recv() => match msg {
                    Some(ExpiryMsg::Add { mailbox_id, expires_at }) => {
                        heap.push(Reverse((expires_at, mailbox_id)));
                        notify.notify_waiters();
                    }
                    Some(ExpiryMsg::Remove { .. }) => {
                        // Filtering happens at pop time.
                    }
                    None => return,
                },
                _ = sleep_until_or_forever(next, now) => {
                    while let Some(Reverse((at, _))) = heap.peek() {
                        let now = chrono::Utc::now().timestamp_millis();
                        if *at > now {
                            break;
                        }
                        let Reverse((_, id)) = heap.pop().expect("non-empty");
                        if let Err(e) = service.expire(&id).await {
                            tracing::warn!(target: "postcrate::mailbox",
                                error = %e, mailbox = %id, "ttl expire failed");
                        }
                    }
                }
            }
        }
    })
}

async fn sleep_until_or_forever(next: Option<i64>, now: i64) {
    match next {
        Some(at) => {
            let dur = (at - now).max(0) as u64;
            tokio::time::sleep(std::time::Duration::from_millis(dur)).await;
        }
        None => std::future::pending::<()>().await,
    }
}
