//! The mailbox service: owns all running SMTP listeners and exposes
//! create/start/stop/recreate/ephemeral operations to the rest of the
//! engine.

use std::collections::HashSet;
use std::net::SocketAddr;
use std::sync::Arc;
use std::time::Duration;

use dashmap::DashMap;
use parking_lot::{Mutex, RwLock};
use sqlx::SqlitePool;
use tokio::sync::mpsc;
use tokio::time::timeout;

use crate::config::CoreConfig;
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
        }
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
                if !referenced.contains(&as_str)
                    && !referenced.iter().any(|r| p.ends_with(r))
                {
                    let _ = tokio::fs::remove_file(&p).await;
                }
            }
        }

        // Start listeners. Skip anything already running so this is safe
        // to call multiple times.
        let rows = db_mb::list_active_for_boot(&self.pool).await?;
        for row in rows {
            if row.failed || self.listeners.contains_key(&row.id) {
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

        let row = db_mb::insert(&self.pool, project_id, name, port, kind, ttl_seconds).await?;
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
        let updated = db_mb::update(&self.pool, id, patch).await?;
        if updated.port != old.port {
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

        let bind = SocketAddr::new(self.config.bind_host.as_ip(), port);
        let advert = EhloAdvert {
            hostname: self.config.ehlo_hostname.clone(),
            max_size: self.config.max_message_bytes,
            starttls_enabled: self.tls_acceptor.is_some(),
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

}
