//! The one public façade. All HTTP routes, all Tauri command shims,
//! all CLI subcommands speak only to this type.

use std::sync::Arc;
use std::time::Duration;

use chrono::Utc;
use sqlx::SqlitePool;
use tokio::sync::mpsc;
use tokio_util::sync::CancellationToken;

use crate::config::CoreConfig;
use crate::db::audit::{AuditAppend, AuditEntry};
use crate::db::bounce_rules::BounceRule;
use crate::db::chaos_configs::ChaosConfig;
use crate::db::emails::{EmailDetail, EmailSummary};
use crate::db::mailboxes::{
    CreateEphemeralInput, CreateMailboxInput, EphemeralHandle, Mailbox, UpdateMailboxInput,
};
use crate::db::settings::{BackendSettings, SettingsPatch};
use crate::db::{audit as db_audit, bounce_rules, chaos_configs, emails as db_emails,
                mailboxes as db_mb, pool as db_pool, settings as db_settings};
use crate::error::Result;
use crate::events::{CoreEvent, EventSink, ServerStatus};
use crate::http;
use crate::mailbox::kinds::MailboxKind;
use crate::mailbox::lifecycle::{self, ExpiryMsg};
use crate::mailbox::service::MailboxService;
use crate::pipeline::{ingest, retention};
use crate::smtp::session::CapturedEnvelope;

pub struct Service {
    inner: Arc<Inner>,
}

pub(crate) struct Inner {
    pub config: CoreConfig,
    pub pool: SqlitePool,
    pub mailboxes: Arc<MailboxService>,
    pub sink: Arc<dyn EventSink>,
    pub cancel: CancellationToken,
    http_handle: parking_lot::Mutex<Option<http::HttpServerHandle>>,
    started: parking_lot::Mutex<bool>,
    /// Hold these so they're cancelled with the service.
    _ingest_task: tokio::task::JoinHandle<()>,
    _retention_task: tokio::task::JoinHandle<()>,
    _ttl_task: tokio::task::JoinHandle<()>,
}

impl std::fmt::Debug for Service {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Service").finish()
    }
}

impl Service {
    /// Build the engine: open the DB, migrate, spawn workers, prepare
    /// listeners. Doesn't bind any sockets — call [`Service::start_all`].
    pub async fn build(cfg: CoreConfig, sink: Arc<dyn EventSink>) -> Result<Service> {
        cfg.ensure_dirs().await?;
        let pool = db_pool::open(&cfg.db_path).await?;
        crate::db::migrate::run(&pool).await?;

        let cancel = CancellationToken::new();

        let (ingest_tx, ingest_rx) = mpsc::channel::<CapturedEnvelope>(cfg.ingest_channel_capacity);
        let (expiry_tx, expiry_rx) = mpsc::unbounded_channel::<ExpiryMsg>();

        let mailboxes = Arc::new(MailboxService::new(
            pool.clone(),
            cfg.clone(),
            ingest_tx,
            expiry_tx,
            sink.clone(),
        ));

        let raw_dir = cfg.raw_dir();
        let att_dir = cfg.att_dir();
        let ingest_task = ingest::spawn(
            pool.clone(),
            sink.clone(),
            ingest_rx,
            raw_dir,
            att_dir,
            cancel.clone(),
        );

        let retention_task = retention::spawn_periodic(
            pool.clone(),
            cancel.clone(),
            Duration::from_secs(3600),
        );

        let initial_expiries = db_mb::list_expiring(&pool).await?;
        let ttl_task = lifecycle::spawn(
            mailboxes.clone(),
            expiry_rx,
            cancel.clone(),
            initial_expiries,
        );

        Ok(Service {
            inner: Arc::new(Inner {
                config: cfg,
                pool,
                mailboxes,
                sink,
                cancel,
                http_handle: parking_lot::Mutex::new(None),
                started: parking_lot::Mutex::new(false),
                _ingest_task: ingest_task,
                _retention_task: retention_task,
                _ttl_task: ttl_task,
            }),
        })
    }

    /// Start every persisted mailbox's listener + the HTTP API.
    /// Idempotent.
    pub async fn start_all(&self) -> Result<()> {
        {
            let mut s = self.inner.started.lock();
            if *s {
                return Ok(());
            }
            *s = true;
        }

        self.inner.mailboxes.boot().await?;
        let http = http::start(self.clone_handle()).await?;
        *self.inner.http_handle.lock() = Some(http);

        self.emit_status();
        Ok(())
    }

    pub async fn stop_all(&self) -> Result<()> {
        if let Some(http) = self.inner.http_handle.lock().take() {
            http.shutdown.cancel();
            let _ = http.task.await;
        }
        self.inner.mailboxes.stop_all().await;
        *self.inner.started.lock() = false;
        self.emit_status();
        Ok(())
    }

    pub fn status(&self) -> ServerStatus {
        ServerStatus {
            running_mailboxes: self.inner.mailboxes.running_count(),
            http_running: self.inner.http_handle.lock().is_some(),
            errors: Vec::new(),
        }
    }

    /// The HTTP API's bound socket address, if the server is running.
    pub fn http_addr(&self) -> Option<std::net::SocketAddr> {
        self.inner.http_handle.lock().as_ref().map(|h| h.addr)
    }

    /// The bound SMTP socket address for a given mailbox listener.
    pub fn mailbox_addr(&self, mailbox_id: &str) -> Option<std::net::SocketAddr> {
        self.inner.mailboxes.listener_addr(mailbox_id)
    }

    fn emit_status(&self) {
        self.inner
            .sink
            .emit(CoreEvent::ServerStatusChanged { status: self.status() });
    }

    pub(crate) fn clone_handle(&self) -> ServiceHandle {
        ServiceHandle {
            inner: self.inner.clone(),
        }
    }

    pub fn handle(&self) -> ServiceHandle {
        self.clone_handle()
    }

    pub fn config(&self) -> &CoreConfig {
        &self.inner.config
    }

    // ---- Mailboxes ----

    pub async fn list_mailboxes(&self, project_id: Option<&str>) -> Result<Vec<Mailbox>> {
        db_mb::list(&self.inner.pool, project_id).await
    }

    pub async fn get_mailbox(&self, id: &str) -> Result<Mailbox> {
        let row = db_mb::get(&self.inner.pool, id).await?;
        let count = db_mb::count_emails(&self.inner.pool, id).await?;
        Ok(row.with_count(count))
    }

    pub async fn create_mailbox(&self, input: CreateMailboxInput) -> Result<Mailbox> {
        let mb = self
            .inner
            .mailboxes
            .create(
                &input.project_id,
                &input.name,
                input.kind,
                input.port,
                input.ttl_seconds,
            )
            .await?;
        self.audit("user", "mailbox.create", Some("mailbox"), Some(&mb.id), None)
            .await;
        Ok(mb)
    }

    pub async fn update_mailbox(
        &self,
        id: &str,
        patch: UpdateMailboxInput,
    ) -> Result<Mailbox> {
        let mb = self.inner.mailboxes.update(id, &patch).await?;
        self.audit("user", "mailbox.update", Some("mailbox"), Some(id), None)
            .await;
        Ok(mb)
    }

    pub async fn delete_mailbox(&self, id: &str) -> Result<()> {
        self.inner.mailboxes.delete(id).await?;
        self.audit("user", "mailbox.delete", Some("mailbox"), Some(id), None)
            .await;
        Ok(())
    }

    pub async fn create_ephemeral(
        &self,
        input: CreateEphemeralInput,
    ) -> Result<EphemeralHandle> {
        let name = input.name.unwrap_or_else(|| format!("eph-{}", short_id()));
        let mb = self
            .inner
            .mailboxes
            .create(
                &input.project_id,
                &name,
                MailboxKind::Ephemeral,
                None,
                Some(input.ttl_seconds),
            )
            .await?;
        let addr = self.inner.mailboxes.listener_addr(&mb.id);
        let host = addr
            .map(|a| a.ip().to_string())
            .unwrap_or_else(|| self.inner.config.bind_host.as_ip().to_string());
        let port = addr.map_or(mb.port, |a| a.port());
        let expires_at = mb.expires_at.unwrap_or_else(|| {
            Utc::now().timestamp_millis() + (input.ttl_seconds as i64 * 1000)
        });
        self.audit(
            "user",
            "mailbox.ephemeral.create",
            Some("mailbox"),
            Some(&mb.id),
            None,
        )
        .await;
        Ok(EphemeralHandle {
            id: mb.id,
            host,
            port,
            expires_at,
        })
    }

    // ---- Emails ----

    pub async fn list_emails(
        &self,
        mailbox_id: &str,
        limit: u32,
        offset: u32,
    ) -> Result<Vec<EmailSummary>> {
        db_emails::list(&self.inner.pool, mailbox_id, limit, offset).await
    }

    pub async fn get_email(&self, id: &str) -> Result<EmailDetail> {
        db_emails::get_detail(&self.inner.pool, id).await
    }

    pub async fn get_email_raw(&self, id: &str) -> Result<Vec<u8>> {
        let path = db_emails::get_raw_path(&self.inner.pool, id).await?;
        Ok(tokio::fs::read(&path).await?)
    }

    pub async fn delete_email(&self, id: &str) -> Result<()> {
        let raw_path = db_emails::delete(&self.inner.pool, id).await?;
        let _ = tokio::fs::remove_file(&raw_path).await;
        self.audit("user", "email.delete", Some("email"), Some(id), None).await;
        Ok(())
    }

    pub async fn clear_mailbox(&self, mailbox_id: &str) -> Result<u64> {
        let (n, paths) = db_emails::clear_mailbox(&self.inner.pool, mailbox_id).await?;
        for p in &paths {
            let _ = tokio::fs::remove_file(p).await;
        }
        self.audit(
            "user",
            "mailbox.clear",
            Some("mailbox"),
            Some(mailbox_id),
            Some(serde_json::json!({"deleted": n})),
        )
        .await;
        Ok(n)
    }

    pub async fn search_emails(
        &self,
        q: &str,
        mailbox_id: Option<&str>,
        limit: u32,
    ) -> Result<Vec<EmailSummary>> {
        db_emails::search(&self.inner.pool, q, mailbox_id, limit).await
    }

    pub async fn mark_read(&self, id: &str, read: bool) -> Result<()> {
        db_emails::mark_read(&self.inner.pool, id, read).await
    }

    // ---- Attachments ----

    pub async fn get_attachment_blob(
        &self,
        attachment_id: &str,
    ) -> Result<(Vec<u8>, Option<String>, Option<String>)> {
        let (path, name, ct) =
            crate::db::attachments::get_blob_path(&self.inner.pool, attachment_id).await?;
        let bytes = tokio::fs::read(&path).await?;
        Ok((bytes, name, ct))
    }

    // ---- Chaos ----

    pub async fn get_chaos(&self, mailbox_id: &str) -> Result<ChaosConfig> {
        // Surface NotFound for an unknown mailbox.
        let _ = db_mb::get(&self.inner.pool, mailbox_id).await?;
        chaos_configs::get(&self.inner.pool, mailbox_id).await
    }

    pub async fn set_chaos(&self, mailbox_id: &str, cfg: ChaosConfig) -> Result<()> {
        let _ = db_mb::get(&self.inner.pool, mailbox_id).await?;
        chaos_configs::upsert(&self.inner.pool, mailbox_id, &cfg).await?;
        self.inner.mailboxes.refresh_chaos(mailbox_id).await?;
        self.audit(
            "user",
            "chaos.update",
            Some("mailbox"),
            Some(mailbox_id),
            Some(serde_json::to_value(&cfg)?),
        )
        .await;
        Ok(())
    }

    // ---- Bounces ----

    pub async fn list_bounce_rules(&self, mailbox_id: &str) -> Result<Vec<BounceRule>> {
        let _ = db_mb::get(&self.inner.pool, mailbox_id).await?;
        bounce_rules::list(&self.inner.pool, mailbox_id).await
    }

    pub async fn upsert_bounce_rule(&self, rule: BounceRule) -> Result<BounceRule> {
        let _ = db_mb::get(&self.inner.pool, &rule.mailbox_id).await?;
        let saved = bounce_rules::upsert(&self.inner.pool, rule).await?;
        self.inner.mailboxes.refresh_bounce(&saved.mailbox_id).await?;
        self.audit(
            "user",
            "bounce.upsert",
            Some("mailbox"),
            Some(&saved.mailbox_id),
            Some(serde_json::to_value(&saved)?),
        )
        .await;
        Ok(saved)
    }

    pub async fn delete_bounce_rule(&self, id: &str) -> Result<()> {
        bounce_rules::delete(&self.inner.pool, id).await?;
        self.audit("user", "bounce.delete", Some("bounce_rule"), Some(id), None).await;
        Ok(())
    }

    // ---- Settings ----

    pub async fn get_settings(&self) -> Result<BackendSettings> {
        db_settings::load_all(&self.inner.pool).await
    }

    pub async fn update_settings(&self, patch: SettingsPatch) -> Result<()> {
        let section = patch.section();
        db_settings::apply_patch(&self.inner.pool, &patch).await?;
        self.inner.sink.emit(CoreEvent::SettingsChanged { section });
        Ok(())
    }

    // ---- Audit ----

    pub async fn list_audit(&self, limit: u32, offset: u32) -> Result<Vec<AuditEntry>> {
        db_audit::list(&self.inner.pool, limit, offset).await
    }

    pub async fn clear_audit(&self, older_than_days: Option<u32>) -> Result<u64> {
        match older_than_days {
            Some(days) => db_audit::prune_older_than(&self.inner.pool, days).await,
            None => db_audit::clear_all(&self.inner.pool).await,
        }
    }

    // ---- internal ----

    async fn audit(
        &self,
        actor: &str,
        action: &str,
        target_kind: Option<&str>,
        target_id: Option<&str>,
        metadata: Option<serde_json::Value>,
    ) {
        let res = db_audit::append(
            &self.inner.pool,
            AuditAppend {
                actor: actor.to_string(),
                action: action.to_string(),
                target_kind: target_kind.map(str::to_string),
                target_id: target_id.map(str::to_string),
                metadata,
            },
        )
        .await;
        if let Ok(entry) = res {
            self.inner
                .sink
                .emit(CoreEvent::AuditAppended { entry });
        }
    }
}

/// Cheap-to-clone view into a [`Service`]. The HTTP layer uses this.
#[derive(Clone)]
pub struct ServiceHandle {
    pub(crate) inner: Arc<Inner>,
}

impl ServiceHandle {
    pub fn pool(&self) -> &SqlitePool {
        &self.inner.pool
    }

    pub fn mailboxes(&self) -> &MailboxService {
        &self.inner.mailboxes
    }

    pub fn config(&self) -> &CoreConfig {
        &self.inner.config
    }

    pub fn sink(&self) -> &Arc<dyn EventSink> {
        &self.inner.sink
    }

    pub fn as_service(&self) -> Service {
        Service {
            inner: self.inner.clone(),
        }
    }
}

impl std::fmt::Debug for ServiceHandle {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ServiceHandle").finish()
    }
}

fn short_id() -> String {
    use rand::distributions::{Alphanumeric, DistString};
    Alphanumeric.sample_string(&mut rand::thread_rng(), 6).to_lowercase()
}
