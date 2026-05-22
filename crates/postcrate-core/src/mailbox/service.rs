//! The mailbox service: owns all running SMTP listeners and exposes
//! create/start/stop/recreate/ephemeral operations to the rest of the
//! engine.

use std::collections::HashSet;
use std::net::SocketAddr;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::time::Duration;

use dashmap::DashMap;
use parking_lot::{Mutex, RwLock};
use sqlx::SqlitePool;
use tokio::sync::mpsc;
use tokio::time::timeout;

use crate::config::CoreConfig;
use crate::db::audit::{self as db_audit, AuditAppend};
use crate::db::{bounce_rules, chaos_configs, emails, mailboxes as db_mb};
use crate::error::{Error, Result};
use crate::events::{CoreEvent, EventSink, MailboxStateChange};
use crate::mailbox::kinds::MailboxKind;
use crate::mailbox::lifecycle::ExpiryMsg;
use crate::mailbox::ports::PortAllocator;
use crate::smtp::bounce::BounceEvaluator;
use crate::smtp::chaos::ChaosInjector;
use crate::smtp::extensions::EhloAdvert;
use crate::smtp::listener::{self, ListenerHandle, ListenerSpec};
use crate::smtp::session::CapturedEnvelope;
use crate::smtp::tls::{self, TlsAcceptor};

pub struct MailboxService {
    pool: SqlitePool,
    config: CoreConfig,
    listeners: DashMap<String, ListenerHandle>,
    bounce_evals: DashMap<String, Arc<RwLock<BounceEvaluator>>>,
    port_alloc: Mutex<PortAllocator>,
    ingest_tx: mpsc::Sender<CapturedEnvelope>,
    expiry_tx: mpsc::UnboundedSender<ExpiryMsg>,
    sink: Arc<dyn EventSink>,
    /// Shared STARTTLS acceptor, built once at construction time. `None`
    /// when TLS is disabled or the binary was built without `--features tls`.
    tls_acceptor: Option<Arc<TlsAcceptor>>,
    /// Reflects `AdvancedPrefs.preserve_smtp_transcript`. Shared with
    /// every running listener so a pref flip takes effect on the very
    /// next session without restarting any listener.
    preserve_transcript: Arc<AtomicBool>,
}

impl std::fmt::Debug for MailboxService {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("MailboxService")
            .field("listener_count", &self.listeners.len())
            .finish()
    }
}

impl MailboxService {
    pub fn new(
        pool: SqlitePool,
        config: CoreConfig,
        ingest_tx: mpsc::Sender<CapturedEnvelope>,
        expiry_tx: mpsc::UnboundedSender<ExpiryMsg>,
        sink: Arc<dyn EventSink>,
    ) -> Self {
        let (lo, hi) = config.ephemeral_port_range;
        let tls_acceptor = match tls::maybe_acceptor(&config.tls) {
            Ok(opt) => opt,
            Err(e) => {
                // A misconfigured TLS is a startup error in spirit, but
                // we don't want to take the whole service down — log,
                // surface STARTTLS as unavailable, and move on.
                tracing::error!(target: "postcrate::tls", error = %e,
                    "failed to load TLS acceptor; STARTTLS will not be offered");
                None
            }
        };
        Self {
            pool,
            config,
            listeners: DashMap::new(),
            bounce_evals: DashMap::new(),
            port_alloc: Mutex::new(PortAllocator::new(lo, hi)),
            ingest_tx,
            expiry_tx,
            sink,
            tls_acceptor,
            preserve_transcript: Arc::new(AtomicBool::new(false)),
        }
    }

    /// Flip the live transcript-capture flag shared with every running
    /// SMTP listener. Cheap atomic write; the next accepted session
    /// reads the new value and decides whether to allocate a sink.
    pub fn set_preserve_transcript(&self, enabled: bool) {
        self.preserve_transcript.store(enabled, Ordering::Relaxed);
    }

    /// Boot-time: sweep expired ephemerals on disk, then start a listener
    /// for every remaining mailbox.
    pub async fn boot(&self) -> Result<()> {
        // Sweep + orphan blob cleanup.
        let swept = db_mb::sweep_expired_ephemerals(&self.pool).await?;
        for id in &swept {
            tracing::info!(target: "postcrate::mailbox", mailbox = %id, "swept expired ephemeral on boot");
        }
        // Orphan raw blobs: any file in raw_dir not referenced by a row.
        // SMTP transcripts live alongside the raw email as
        // `<raw>.smtp.log` — treat them as referenced when their email
        // is, so they don't get swept up here and resurrected as
        // orphans on next boot.
        let referenced: HashSet<String> = emails::list_all_raw_paths(&self.pool)
            .await?
            .into_iter()
            .collect();
        if let Ok(mut rd) = tokio::fs::read_dir(self.config.raw_dir()).await {
            while let Ok(Some(entry)) = rd.next_entry().await {
                let p = entry.path();
                if !p.is_file() {
                    continue;
                }
                let as_str = p.to_string_lossy().to_string();
                let parent = as_str.strip_suffix(".smtp.log").map(str::to_string);
                let is_known_transcript = parent
                    .as_ref()
                    .is_some_and(|p| referenced.contains(p));
                if is_known_transcript {
                    continue;
                }
                if !referenced.contains(&as_str)
                    && !referenced.iter().any(|r| p.ends_with(r))
                {
                    let _ = tokio::fs::remove_file(&p).await;
                }
            }
        }

        // Start listeners. Skip anything already running so this is safe
        // to call multiple times. Also skip rows the user explicitly
        // stopped (`paused`) and rows that failed last time — both
        // require user action (Start) to come back online.
        let rows = db_mb::list_active_for_boot(&self.pool).await?;
        for row in rows {
            if row.failed || row.paused || self.listeners.contains_key(&row.id) {
                continue;
            }
            if let Err(e) = self.start_listener_for(&row.id, row.port).await {
                tracing::warn!(target: "postcrate::mailbox",
                    error = %e, mailbox = %row.id, "boot start failed");
                let _ = db_mb::mark_failed(&self.pool, &row.id, Some(&e.to_string())).await;
            }
        }

        Ok(())
    }

    /// Bring a stopped or failed listener back online. Idempotent on
    /// already-running mailboxes (returns Ok without rebinding).
    /// Clears any prior `failed` state on success; surfaces bind
    /// errors to the caller so Tauri can revert the optimistic update.
    pub async fn start(&self, id: &str) -> Result<()> {
        if self.listeners.contains_key(id) {
            return Ok(());
        }
        let row = db_mb::get(&self.pool, id).await?;
        // Best-effort: a stale `failed=1` from a previous boot must
        // not block the retry. mark_failed runs again on error if the
        // new bind also fails.
        let _ = db_mb::clear_failed(&self.pool, id).await;
        match self.start_listener_for(id, row.port).await {
            Ok(()) => Ok(()),
            Err(e) => {
                let _ = db_mb::mark_failed(&self.pool, id, Some(&e.to_string())).await;
                Err(e)
            }
        }
    }

    /// Tear down a running listener. Idempotent on already-stopped
    /// mailboxes — returns Ok without emitting a second `Stopped`.
    /// The user-intent `paused` flag is managed one layer up in
    /// `Service::stop_mailbox`; this is the pure runtime tear-down.
    pub async fn stop(&self, id: &str) -> Result<()> {
        if !self.listeners.contains_key(id) {
            return Ok(());
        }
        self.stop_listener(id).await;
        Ok(())
    }

    pub async fn stop_all(&self) {
        let ids: Vec<String> = self.listeners.iter().map(|e| e.key().clone()).collect();
        for id in ids {
            self.stop_listener(&id).await;
        }
    }

    pub fn running_count(&self) -> u32 {
        self.listeners.len() as u32
    }

    pub async fn create(
        &self,
        project_id: &str,
        name: &str,
        kind: MailboxKind,
        port: Option<u16>,
        ttl_seconds: Option<u64>,
        implicit_tls: bool,
    ) -> Result<db_mb::Mailbox> {
        let port = match (port, kind) {
            (Some(p), _) => p,
            (None, MailboxKind::Ephemeral) => self.allocate_ephemeral_port().await?,
            (None, _) => {
                return Err(Error::Invalid(
                    "port required for non-ephemeral mailbox".into(),
                ))
            }
        };

        let row = db_mb::insert(
            &self.pool,
            project_id,
            name,
            port,
            kind,
            ttl_seconds,
            implicit_tls,
        )
        .await?;
        if matches!(kind, MailboxKind::Ephemeral) {
            if let Some(exp) = row.expires_at {
                let _ = self.expiry_tx.send(ExpiryMsg::Add {
                    mailbox_id: row.id.clone(),
                    expires_at: exp,
                });
            }
        }
        if let Err(e) = self.start_listener_for(&row.id, port).await {
            // Roll back the row if we can't bind.
            let _ = db_mb::mark_failed(&self.pool, &row.id, Some(&e.to_string())).await;
            return Err(e);
        }

        let mb = row.with_count(0);
        self.sink.emit(CoreEvent::MailboxStateChanged {
            mailbox_id: mb.id.clone(),
            change: MailboxStateChange::Created,
        });
        Ok(mb)
    }

    pub async fn update(
        &self,
        id: &str,
        patch: &db_mb::UpdateMailboxInput,
    ) -> Result<db_mb::Mailbox> {
        let old = db_mb::get(&self.pool, id).await?;
        // Snapshot runtime state *before* the DB write so a port-change
        // rebind only happens when the listener was actually running.
        // For paused mailboxes we'd otherwise resurrect the listener
        // here (stop is a no-op, start succeeds) while the DB still
        // says paused=true — a quiet state lie. For failed mailboxes
        // the user's recovery path is Start (which probes + clears
        // `failed`), not Edit; Edit only mutates the persisted record.
        let was_running = self.listeners.contains_key(id);
        let updated = db_mb::update(&self.pool, id, patch).await?;
        if updated.port != old.port && was_running {
            self.stop_listener(id).await;
            if let Err(e) = self.start_listener_for(id, updated.port).await {
                let _ = db_mb::mark_failed(&self.pool, id, Some(&e.to_string())).await;
                return Err(e);
            }
        }
        let count = db_mb::count_emails(&self.pool, id).await?;
        let mb = updated.with_count(count);
        self.sink.emit(CoreEvent::MailboxStateChanged {
            mailbox_id: mb.id.clone(),
            change: MailboxStateChange::Updated,
        });
        Ok(mb)
    }

    pub async fn delete(&self, id: &str) -> Result<()> {
        self.stop_listener(id).await;
        self.bounce_evals.remove(id);
        db_mb::delete(&self.pool, id).await?;
        self.sink.emit(CoreEvent::MailboxStateChanged {
            mailbox_id: id.to_string(),
            change: MailboxStateChange::Deleted,
        });
        Ok(())
    }

    /// Called by the lifecycle task at TTL.
    pub(crate) async fn expire(&self, id: &str) -> Result<()> {
        // Verify the mailbox still exists and is genuinely expired.
        match db_mb::get(&self.pool, id).await {
            Ok(row) => {
                let now = chrono::Utc::now().timestamp_millis();
                if row.expires_at.is_none_or(|e| e > now) {
                    return Ok(());
                }
                self.stop_listener(id).await;
                db_mb::delete(&self.pool, id).await?;
                self.sink.emit(CoreEvent::MailboxStateChanged {
                    mailbox_id: id.to_string(),
                    change: MailboxStateChange::Expired,
                });
                Ok(())
            }
            Err(Error::MailboxNotFound(_)) => Ok(()),
            Err(e) => Err(e),
        }
    }

    pub async fn refresh_bounce(&self, mailbox_id: &str) -> Result<()> {
        let rules = bounce_rules::list_enabled(&self.pool, mailbox_id).await?;
        if let Some(eval) = self.bounce_evals.get(mailbox_id) {
            eval.write().replace(rules);
        }
        Ok(())
    }

    pub async fn refresh_chaos(&self, mailbox_id: &str) -> Result<()> {
        // Chaos cfg lives per-session; recreate the listener so new
        // connections pick up the new config. Existing sessions keep
        // the old behavior — that's an intentional simplification.
        if self.listeners.contains_key(mailbox_id) {
            let row = db_mb::get(&self.pool, mailbox_id).await?;
            self.stop_listener(mailbox_id).await;
            self.start_listener_for(mailbox_id, row.port).await?;
        }
        Ok(())
    }

    pub fn listener_addr(&self, mailbox_id: &str) -> Option<SocketAddr> {
        self.listeners.get(mailbox_id).map(|h| h.addr)
    }

    /// Clone the ingest sender so callers can push synthetic envelopes
    /// (recording replay, test fixtures) through the same single-writer
    /// pipeline that the real SMTP path uses.
    pub fn ingest_tx(&self) -> mpsc::Sender<CapturedEnvelope> {
        self.ingest_tx.clone()
    }

    /// Pick a fresh ephemeral port. We pull the in-use set under a lock,
    /// release it, do the async probe outside the lock, then re-lock just
    /// to record the reservation.
    async fn allocate_ephemeral_port(&self) -> Result<u16> {
        let db_ports: HashSet<u16> = db_mb::list_all_ports(&self.pool)
            .await?
            .into_iter()
            .collect();
        let mut snapshot = self.port_alloc.lock().clone();
        let port = snapshot
            .reserve(self.config.bind_host.as_ip(), &db_ports)
            .await?;
        self.port_alloc.lock().mark_reserved(port);
        Ok(port)
    }

    // ---- internal ----

    async fn stop_listener(&self, id: &str) {
        if let Some((_, handle)) = self.listeners.remove(id) {
            handle.cancel.cancel();
            let _ = timeout(Duration::from_secs(2), handle.task).await;
            self.port_alloc.lock().release(handle.addr.port());
            self.sink.emit(CoreEvent::MailboxStateChanged {
                mailbox_id: id.to_string(),
                change: MailboxStateChange::Stopped,
            });
        }
    }

    async fn start_listener_for(&self, id: &str, port: u16) -> Result<()> {
        let chaos_cfg = chaos_configs::get(&self.pool, id).await?;
        let bounce_rules_list = bounce_rules::list_enabled(&self.pool, id).await?;
        let row = db_mb::get(&self.pool, id).await?;
        // Implicit TLS requires a live acceptor — fall back to plaintext
        // otherwise rather than refusing to boot a mailbox.
        let implicit_tls = row.implicit_tls && self.tls_acceptor.is_some();

        let bind = SocketAddr::new(self.config.bind_host.as_ip(), port);
        let advert = EhloAdvert {
            hostname: self.config.ehlo_hostname.clone(),
            max_size: self.config.max_message_bytes,
            // STARTTLS is only meaningful on a plaintext listener; an
            // implicit-TLS listener already has an encrypted session
            // by the time the client speaks SMTP.
            starttls_enabled: self.tls_acceptor.is_some() && !implicit_tls,
            // AUTH is advertised by default for client compatibility.
            // Local capture servers don't actually need authentication,
            // but many sender libraries refuse to submit unless AUTH is
            // offered, so we advertise it and accept any credentials.
            auth_enabled: true,
        };

        // Reuse the existing bounce-evaluator handle if we have one;
        // otherwise create a fresh one. This is what lets live rule
        // updates take effect without restarting the listener.
        let bounce = self
            .bounce_evals
            .entry(id.to_string())
            .or_insert_with(|| Arc::new(RwLock::new(BounceEvaluator::default())))
            .clone();
        bounce.write().replace(bounce_rules_list);

        let spec = ListenerSpec {
            mailbox_id: id.to_string(),
            bind,
            ehlo_advert: advert,
            max_line: self.config.smtp_max_line_bytes,
            max_bytes: self.config.max_message_bytes,
            spill_at: self.config.data_spill_bytes,
            incoming_dir: self.config.incoming_dir(),
            chaos: ChaosInjector::new(chaos_cfg, port as u64),
            bounce,
            ingest_tx: self.ingest_tx.clone(),
            tls_acceptor: self.tls_acceptor.clone(),
            implicit_tls,
            preserve_transcript: self.preserve_transcript.clone(),
        };

        match listener::start(spec).await {
            Ok(handle) => {
                self.listeners.insert(id.to_string(), handle);
                self.sink.emit(CoreEvent::MailboxStateChanged {
                    mailbox_id: id.to_string(),
                    change: MailboxStateChange::Started,
                });
                let _ = db_mb::clear_failed(&self.pool, id).await;
                Ok(())
            }
            Err(e) => {
                let kind = if matches!(&e, Error::Io(io_err)
                    if io_err.kind() == std::io::ErrorKind::AddrInUse)
                {
                    Error::PortInUse(port)
                } else {
                    e
                };
                self.audit_failed(id, &kind.to_string()).await;
                self.sink.emit(CoreEvent::MailboxStateChanged {
                    mailbox_id: id.to_string(),
                    change: MailboxStateChange::Failed {
                        error: kind.to_string(),
                    },
                });
                Err(kind)
            }
        }
    }

    /// Append a `mailbox.failed` audit row whenever a listener can't
    /// bind. Actor is "system" (not "user") so the UI can tell it
    /// apart from a user-initiated start. Failures here are
    /// best-effort: if the audit table itself can't be written, log
    /// it but never block the originating error from propagating.
    async fn audit_failed(&self, id: &str, error: &str) {
        let entry = db_audit::append(
            &self.pool,
            AuditAppend {
                actor: "system".to_string(),
                action: "mailbox.failed".to_string(),
                target_kind: Some("mailbox".to_string()),
                target_id: Some(id.to_string()),
                metadata: Some(serde_json::json!({ "error": error })),
            },
        )
        .await;
        match entry {
            Ok(entry) => {
                self.sink.emit(CoreEvent::AuditAppended { entry });
            }
            Err(err) => {
                tracing::warn!(target: "postcrate::mailbox",
                    error = %err, mailbox = %id,
                    "couldn't write mailbox.failed audit row");
            }
        }
    }
}
