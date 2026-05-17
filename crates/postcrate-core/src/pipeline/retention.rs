//! Retention: per-mailbox cap (inline) + global age cap + audit pruning.
//!
//! Runs inside the ingest worker so there's only ever one writer. The
//! periodic age-based purge spawns its own task but uses a coarse
//! interval (default every hour) so there's no realistic write contention.

use std::path::Path;
use std::time::Duration;

use chrono::Utc;
use sqlx::SqlitePool;
use tokio::task::JoinHandle;
use tokio_util::sync::CancellationToken;

use crate::db::{audit, emails, settings};
use crate::error::Result;

/// Trim `mailbox_id` to at most `keep_max` newest emails. Returns the
/// number of rows removed. Also deletes the matching raw-blob files.
pub async fn cap_per_mailbox(
    pool: &SqlitePool,
    mailbox_id: &str,
    keep_max: i64,
    _raw_dir: &Path,
) -> Result<u64> {
    if keep_max <= 0 {
        return Ok(0);
    }
    let victims = emails::trim_mailbox(pool, mailbox_id, keep_max).await?;
    if victims.is_empty() {
        return Ok(0);
    }
    let ids: Vec<String> = victims.iter().map(|(id, _)| id.clone()).collect();
    emails::delete_by_ids(pool, &ids).await?;
    for (_, raw_path) in &victims {
        let _ = tokio::fs::remove_file(raw_path).await;
    }
    Ok(victims.len() as u64)
}

/// Periodic age-based purge + audit pruning. Spawned once at boot.
pub fn spawn_periodic(
    pool: SqlitePool,
    cancel: CancellationToken,
    interval: Duration,
) -> JoinHandle<()> {
    tokio::spawn(async move {
        let mut tick = tokio::time::interval(interval);
        tick.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);

        loop {
            tokio::select! {
                _ = cancel.cancelled() => return,
                _ = tick.tick() => {
                    if let Err(e) = run_once(&pool).await {
                        tracing::warn!(target: "postcrate::retention",
                            error = %e, "retention sweep failed");
                    }
                }
            }
        }
    })
}

async fn run_once(pool: &SqlitePool) -> Result<()> {
    let s = settings::load_all(pool).await?;

    // Age-based email purge.
    if s.inbox.auto_clear_after_days > 0 {
        let cutoff = Utc::now().timestamp_millis()
            - (i64::from(s.inbox.auto_clear_after_days) * 86_400_000);
        let victims = emails::list_older_than(pool, cutoff).await?;
        if !victims.is_empty() {
            let ids: Vec<String> = victims.iter().map(|(id, _, _)| id.clone()).collect();
            emails::delete_by_ids(pool, &ids).await?;
            for (_, _, raw_path) in &victims {
                let _ = tokio::fs::remove_file(raw_path).await;
            }
            tracing::info!(target: "postcrate::retention",
                count = victims.len(), "age-purged emails");
        }
    }

    // Audit log pruning.
    if s.advanced.audit_retain_days > 0 {
        let pruned = audit::prune_older_than(pool, s.advanced.audit_retain_days).await?;
        if pruned > 0 {
            tracing::info!(target: "postcrate::retention", count = pruned, "pruned audit rows");
        }
    }

    Ok(())
}
