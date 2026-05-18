//! The single public façade. The built-in HTTP routes, downstream
//! command shims, and CLI subcommands all speak only to this type.

use std::sync::Arc;
use std::time::Duration;

use chrono::Utc;
use sqlx::SqlitePool;
use tokio::sync::mpsc;
use tokio_util::sync::CancellationToken;

use tokio::sync::broadcast;

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
use crate::events::{ChannelSink, ComposedSink, CoreEvent, EventSink, ServerStatus};
use crate::http;
use crate::mailbox::kinds::MailboxKind;
use crate::mailbox::lifecycle::{self, ExpiryMsg};
use crate::mailbox::service::MailboxService;
use crate::pipeline::{ingest, retention};
use crate::smtp::session::CapturedEnvelope;

pub struct Service {
    inner: Arc<Inner>,
}

#[derive(Debug)]
struct ScanResult {
    matched: Option<EmailDetail>,
    seen: Vec<EmailSummary>,
}

pub(crate) struct Inner {
    pub config: CoreConfig,
    pub pool: SqlitePool,
    pub mailboxes: Arc<MailboxService>,
    pub sink: Arc<dyn EventSink>,
    /// In-process fan-out for `Service::subscribe`. Wrapped under the
    /// user-provided sink via `ComposedSink` so every emission reaches
    /// both the embedder's sink and any in-process `subscribe()`
    /// consumers (CLI tail, SSE endpoint, `wait_for_email`).
    pub events: ChannelSink,
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

        // Build a composed sink: the user's sink + our internal channel
        // sink. Every `emit` reaches both. Subscribers (CLI tail, SSE,
        // wait_for_email) read from `events`.
        let events = ChannelSink::new(1024);
        let composed: Arc<dyn EventSink> = Arc::new(ComposedSink::new(vec![
            sink.clone(),
            Arc::new(events.clone()),
        ]));

        let mailboxes = Arc::new(MailboxService::new(
            pool.clone(),
            cfg.clone(),
            ingest_tx,
            expiry_tx,
            composed.clone(),
        ));

        let raw_dir = cfg.raw_dir();
        let att_dir = cfg.att_dir();
        let ingest_task = ingest::spawn(
            pool.clone(),
            composed.clone(),
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
                sink: composed,
                events,
                cancel,
                http_handle: parking_lot::Mutex::new(None),
                started: parking_lot::Mutex::new(false),
                _ingest_task: ingest_task,
                _retention_task: retention_task,
                _ttl_task: ttl_task,
            }),
        })
    }

    /// Subscribe to engine events. Each call returns a fresh
    /// `broadcast::Receiver`; consumers that lag behind by more than
    /// the channel capacity (currently 1024) will receive `Lagged`
    /// errors and must reconnect. This is the canonical way for the
    /// CLI `tail`, the SSE endpoint, the `wait_for_email` primitive,
    /// and external consumers to observe events.
    pub fn subscribe(&self) -> broadcast::Receiver<CoreEvent> {
        self.inner.events.subscribe()
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
        // Scope the lock so the MutexGuard is dropped before we await
        // — otherwise this future is `!Send` and can't be spawned from
        // a multi-thread runtime (e.g. Tauri's app shutdown hook).
        let http = self.inner.http_handle.lock().take();
        if let Some(http) = http {
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
                input.implicit_tls,
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
                false,
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

    /// Clear all non-pinned emails from a mailbox. Pinned emails (set
    /// via [`Self::set_pinned`]) survive. Use
    /// [`Self::purge_mailbox`] to wipe everything including pinned.
    pub async fn clear_mailbox(&self, mailbox_id: &str) -> Result<u64> {
        let (n, paths) = db_emails::clear_mailbox(&self.inner.pool, mailbox_id, true).await?;
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

    /// Wipe every email in a mailbox — pinned ones included. Use
    /// only for explicit "purge" actions (rare).
    pub async fn purge_mailbox(&self, mailbox_id: &str) -> Result<u64> {
        let (n, paths) = db_emails::clear_mailbox(&self.inner.pool, mailbox_id, false).await?;
        for p in &paths {
            let _ = tokio::fs::remove_file(p).await;
        }
        self.audit(
            "user",
            "mailbox.purge",
            Some("mailbox"),
            Some(mailbox_id),
            Some(serde_json::json!({"deleted": n})),
        )
        .await;
        Ok(n)
    }

    pub async fn set_pinned(&self, id: &str, pinned: bool) -> Result<()> {
        db_emails::set_pinned(&self.inner.pool, id, pinned).await?;
        self.audit(
            "user",
            if pinned { "email.pin" } else { "email.unpin" },
            Some("email"),
            Some(id),
            None,
        )
        .await;
        Ok(())
    }

    pub async fn set_starred(&self, id: &str, starred: bool) -> Result<()> {
        db_emails::set_starred(&self.inner.pool, id, starred).await?;
        self.audit(
            "user",
            if starred { "email.star" } else { "email.unstar" },
            Some("email"),
            Some(id),
            None,
        )
        .await;
        Ok(())
    }

    pub async fn set_note(&self, id: &str, note: Option<&str>) -> Result<()> {
        db_emails::set_note(&self.inner.pool, id, note).await?;
        self.audit("user", "email.note", Some("email"), Some(id), None).await;
        Ok(())
    }

    /// Set or clear the tag on an email. Plus-addressing
    /// (`user+tag@host`) sets this automatically at ingest; this
    /// method lets users override or clear it manually.
    pub async fn set_tag(&self, id: &str, tag: Option<&str>) -> Result<()> {
        db_emails::set_tag(&self.inner.pool, id, tag).await?;
        self.audit("user", "email.tag", Some("email"), Some(id), None).await;
        Ok(())
    }

    /// Forward a captured email to a real address via an external SMTP
    /// relay. The original raw bytes are sent unchanged; the envelope
    /// `MAIL FROM` defaults to the captured sender and the envelope
    /// recipient is the new `to`.
    ///
    /// Audit-logged (PROD.md §9.3): this is the only public-Service
    /// method that produces outbound network traffic, so users need a
    /// clear trail of when releases happen.
    pub async fn release_email(
        &self,
        id: &str,
        to: &str,
        relay: &crate::RelayConfig,
    ) -> Result<()> {
        let detail = self.get_email(id).await?;
        let raw = self.get_email_raw(id).await?;
        let from = if detail.from.is_empty() {
            "postcrate@localhost".to_string()
        } else {
            detail.from.clone()
        };
        crate::smtp::relay::relay_message(relay, &from, &[to.to_string()], &raw).await?;
        self.audit(
            "user",
            "email.release",
            Some("email"),
            Some(id),
            Some(serde_json::json!({
                "to": to,
                "relay": format!("{}:{}", relay.host, relay.port),
            })),
        )
        .await;
        Ok(())
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

    // ---- Scenarios ----

    /// Score a captured email's spam-likelihood.
    /// Local heuristics only; no network.
    pub async fn analyze_spam(
        &self,
        id: &str,
    ) -> Result<crate::scenarios::spam::SpamReport> {
        let parsed = self.parsed_email(id).await?;
        Ok(crate::scenarios::spam::score(&parsed))
    }

    /// Extract + classify every link in a captured email
    ///. Does not HEAD-check links.
    pub async fn analyze_links(
        &self,
        id: &str,
    ) -> Result<crate::scenarios::links::LinkReport> {
        let parsed = self.parsed_email(id).await?;
        Ok(crate::scenarios::links::extract(&parsed))
    }

    /// Inspect SPF / DKIM / DMARC headers and predict pass/fail
    ///. Header inspection only.
    pub async fn analyze_auth(
        &self,
        id: &str,
    ) -> Result<crate::scenarios::auth::AuthReport> {
        let parsed = self.parsed_email(id).await?;
        Ok(crate::scenarios::auth::analyze(&parsed))
    }

    /// Validate the `List-Unsubscribe` / `List-Unsubscribe-Post`
    /// headers per RFC 2369 + RFC 8058.
    pub async fn analyze_list_unsub(
        &self,
        id: &str,
    ) -> Result<crate::scenarios::list_unsub::UnsubReport> {
        let parsed = self.parsed_email(id).await?;
        Ok(crate::scenarios::list_unsub::analyze(&parsed))
    }

    /// Helper: re-parse a captured email's raw bytes from disk.
    /// We don't cache the full `Parsed` in SQLite (only its JSON
    /// projection), so scenarios that need attachments or full
    /// headers re-parse on demand.
    async fn parsed_email(&self, id: &str) -> Result<crate::mail::parse::Parsed> {
        let raw = self.get_email_raw(id).await?;
        Ok(crate::mail::parse::parse(&raw))
    }

    // ---- Rendering ----

    /// Render the email's HTML body through a client profile
    ///. Returns the transformed HTML + a list of
    /// transforms that ran.
    pub async fn render_preview(
        &self,
        id: &str,
        profile: crate::rendering::profile::Profile,
    ) -> Result<crate::rendering::profile::RenderedPreview> {
        let detail = self.get_email(id).await?;
        let html = detail.html_body.unwrap_or_default();
        Ok(crate::rendering::profile::apply(&html, profile))
    }

    /// Lint the email's HTML for known client incompatibilities.
    pub async fn lint_html(&self, id: &str) -> Result<crate::rendering::lint::LintReport> {
        let detail = self.get_email(id).await?;
        let html = detail.html_body.unwrap_or_default();
        Ok(crate::rendering::lint::lint(&html))
    }

    /// Accessibility check on the email's HTML.
    pub async fn audit_a11y(&self, id: &str) -> Result<crate::rendering::a11y::A11yReport> {
        let detail = self.get_email(id).await?;
        let html = detail.html_body.unwrap_or_default();
        Ok(crate::rendering::a11y::audit(&html))
    }

    // ---- Recordings ----

    /// Snapshot every email in a mailbox into a portable
    /// `.postcrate` recording. The result serializes to
    /// JSON via serde; the caller is responsible for persisting it.
    pub async fn export_recording(
        &self,
        mailbox_id: &str,
        label: Option<String>,
    ) -> Result<crate::recording::Recording> {
        // Existence check + 404 propagation.
        let _ = db_mb::get(&self.inner.pool, mailbox_id).await?;
        let summaries = db_emails::list(&self.inner.pool, mailbox_id, u32::MAX, 0).await?;
        let mut messages = Vec::with_capacity(summaries.len());
        // Walk in chronological order so replay observes the same
        // received-at ordering as the original capture.
        let mut summaries = summaries;
        summaries.sort_by_key(|s| s.received_at);
        for s in summaries {
            let raw = self.get_email_raw(&s.id).await?;
            let detail = self.get_email(&s.id).await?;
            messages.push(crate::recording::RecordedMessage {
                envelope: crate::recording::RecordedEnvelope {
                    mail_from: detail.from.clone(),
                    rcpt_to: detail.to.clone(),
                    received_at: detail.received_at,
                    ext_smtputf8: detail.ext_smtputf8,
                    ext_8bitmime: detail.ext_8bitmime,
                    subject: detail.subject.clone(),
                },
                raw_b64: crate::recording::encode_raw(&raw),
            });
        }
        Ok(crate::recording::Recording {
            version: crate::recording::RECORDING_VERSION,
            exported_at: chrono::Utc::now().timestamp_millis(),
            label,
            messages,
        })
    }

    /// Replay a recording's messages straight into a mailbox by
    /// pushing them through the ingest worker. SMTP listeners,
    /// chaos, and bounce rules are bypassed — this is for fixture
    /// restoration, not for re-running a scenario.
    /// Use [`Self::replay_email`] for a single SMTP-driven re-send.
    pub async fn replay_recording(
        &self,
        mailbox_id: &str,
        recording: &crate::recording::Recording,
    ) -> Result<u64> {
        recording.validate()?;
        let _ = db_mb::get(&self.inner.pool, mailbox_id).await?;
        let mailbox_id_owned = mailbox_id.to_string();
        let ingest_tx = self.inner.mailboxes.ingest_tx();
        let incoming_dir = self.inner.config.incoming_dir();
        tokio::fs::create_dir_all(&incoming_dir).await?;

        let mut count: u64 = 0;
        for msg in &recording.messages {
            let raw = crate::recording::decode_raw(msg)?;
            // Spill the bytes to a temp file so the ingest worker
            // picks up an OnDisk source (matches the real DATA path
            // for messages > spill threshold; behavior is identical
            // for smaller payloads).
            let tmp = incoming_dir.join(format!("{}.tmp", uuid::Uuid::new_v4()));
            tokio::fs::write(&tmp, &raw).await?;
            let size = raw.len() as u64;
            let env = crate::smtp::session::CapturedEnvelope {
                mailbox_id: mailbox_id_owned.clone(),
                received_at: msg.envelope.received_at,
                mail_from: msg.envelope.mail_from.clone(),
                rcpt_to: msg.envelope.rcpt_to.clone(),
                raw: crate::smtp::data_reader::CapturedSource::OnDisk(tmp, size),
                ext_smtputf8: msg.envelope.ext_smtputf8,
                ext_8bitmime: msg.envelope.ext_8bitmime,
            };
            ingest_tx
                .send(env)
                .await
                .map_err(|e| crate::error::Error::Internal(format!("ingest closed: {e}")))?;
            count += 1;
        }
        self.audit(
            "user",
            "recording.replay",
            Some("mailbox"),
            Some(mailbox_id),
            Some(serde_json::json!({"count": count})),
        )
        .await;
        Ok(count)
    }

    /// Re-inject one captured email's raw bytes into a (possibly
    /// different) mailbox via the local SMTP listener — exercises
    /// chaos + bounce rules + parsing the way a real send would.
    pub async fn replay_email(&self, id: &str, target_mailbox_id: &str) -> Result<()> {
        let detail = self.get_email(id).await?;
        let raw = self.get_email_raw(id).await?;
        let addr = self
            .inner
            .mailboxes
            .listener_addr(target_mailbox_id)
            .ok_or_else(|| crate::error::Error::MailboxNotFound(target_mailbox_id.into()))?;
        let from = if detail.from.is_empty() {
            "postcrate@localhost".to_string()
        } else {
            detail.from.clone()
        };
        let rcpts = if detail.to.is_empty() {
            vec!["postcrate@localhost".to_string()]
        } else {
            detail.to.clone()
        };
        crate::smtp::relay::relay_message(
            &crate::RelayConfig {
                host: addr.ip().to_string(),
                port: addr.port(),
                timeout_seconds: Some(10),
                allowed_recipients: None,
            },
            &from,
            &rcpts,
            &raw,
        )
        .await?;
        self.audit(
            "user",
            "email.replay",
            Some("email"),
            Some(id),
            Some(serde_json::json!({"targetMailbox": target_mailbox_id})),
        )
        .await;
        Ok(())
    }

    // ---- Wait / Match ----

    /// Block up to `timeout` for an email that satisfies `predicate`.
    ///
    /// Sequence:
    ///   1. Subscribe to the event stream first (so we don't miss an
    ///      email that arrives between scan + subscribe).
    ///   2. Do a one-shot scan of recent emails in case it already
    ///      arrived before the call.
    ///   3. Otherwise consume the broadcast until timeout.
    ///
    /// The returned [`crate::matcher::WaitOutcome`] always carries the
    /// list of emails seen during the wait, so callers can distinguish
    /// "no email at all" from "email arrived but didn't match".
    pub async fn wait_for_email(
        &self,
        predicate: crate::matcher::EmailPredicate,
        timeout: std::time::Duration,
    ) -> Result<crate::matcher::WaitOutcome> {
        use crate::events::CoreEvent;
        use tokio::sync::broadcast::error::RecvError;
        use tokio::time::Instant;

        let mut rx = self.subscribe();
        let mut seen: Vec<EmailSummary> = Vec::new();

        // Initial scan — most recent 100 emails in scope.
        let initial = self.scan_for_match(&predicate, 100).await?;
        if let Some(d) = initial.matched {
            return Ok(crate::matcher::WaitOutcome {
                matched: Some(d),
                seen_during_wait: initial.seen,
            });
        }
        seen.extend(initial.seen);

        let deadline = Instant::now() + timeout;
        loop {
            let remaining = deadline.saturating_duration_since(Instant::now());
            if remaining.is_zero() {
                return Ok(crate::matcher::WaitOutcome {
                    matched: None,
                    seen_during_wait: seen,
                });
            }
            match tokio::time::timeout(remaining, rx.recv()).await {
                Err(_) => {
                    return Ok(crate::matcher::WaitOutcome {
                        matched: None,
                        seen_during_wait: seen,
                    });
                }
                Ok(Err(RecvError::Closed)) => {
                    return Ok(crate::matcher::WaitOutcome {
                        matched: None,
                        seen_during_wait: seen,
                    });
                }
                Ok(Err(RecvError::Lagged(_))) => {
                    // Catch up via a full scan and keep looping.
                    let catch = self.scan_for_match(&predicate, 100).await?;
                    if catch.matched.is_some() {
                        return Ok(crate::matcher::WaitOutcome {
                            matched: catch.matched,
                            seen_during_wait: seen,
                        });
                    }
                    continue;
                }
                Ok(Ok(CoreEvent::NewEmail { mailbox_id, email })) => {
                    if predicate.mailbox_id.as_ref().is_some_and(|m| m != &mailbox_id) {
                        continue;
                    }
                    if predicate.matches_summary(&email) {
                        let detail = self.get_email(&email.id).await?;
                        if predicate.check(&detail).matched {
                            return Ok(crate::matcher::WaitOutcome {
                                matched: Some(detail),
                                seen_during_wait: seen,
                            });
                        }
                    }
                    seen.push(email);
                }
                Ok(Ok(_)) => continue,
            }
        }
    }

    /// Check a specific email against a predicate. The full
    /// [`crate::matcher::MatchResult`] is returned (including any
    /// mismatches) so callers can produce a structured diff.
    pub async fn assert_email_matches(
        &self,
        id: &str,
        predicate: &crate::matcher::EmailPredicate,
    ) -> Result<crate::matcher::MatchResult> {
        let detail = self.get_email(id).await?;
        Ok(predicate.check(&detail))
    }

    /// Implementation detail of [`Self::wait_for_email`]: scan up to
    /// `limit` most-recent emails (across all mailboxes, or filtered
    /// by `predicate.mailbox_id`) and return either the first match
    /// or the list of all candidates seen.
    async fn scan_for_match(
        &self,
        predicate: &crate::matcher::EmailPredicate,
        limit: u32,
    ) -> Result<ScanResult> {
        let summaries = match &predicate.mailbox_id {
            Some(mb) => db_emails::list(&self.inner.pool, mb, limit, 0).await?,
            None => db_emails::list_recent_across(&self.inner.pool, limit).await?,
        };
        let mut seen = Vec::new();
        for s in summaries {
            if !predicate.matches_summary(&s) {
                seen.push(s);
                continue;
            }
            let detail = self.get_email(&s.id).await?;
            if predicate.check(&detail).matched {
                return Ok(ScanResult { matched: Some(detail), seen });
            }
            seen.push(s);
        }
        Ok(ScanResult { matched: None, seen })
    }

    // ---- Webhooks ----

    pub async fn list_webhooks(&self) -> Result<Vec<crate::db::webhooks::Webhook>> {
        crate::db::webhooks::list(&self.inner.pool).await
    }

    pub async fn create_webhook(
        &self,
        input: crate::db::webhooks::CreateWebhook,
    ) -> Result<crate::db::webhooks::Webhook> {
        let hook = crate::db::webhooks::insert(&self.inner.pool, input).await?;
        self.audit(
            "user",
            "webhook.create",
            Some("webhook"),
            Some(&hook.id),
            None,
        )
        .await;
        Ok(hook)
    }

    pub async fn delete_webhook(&self, id: &str) -> Result<()> {
        crate::db::webhooks::delete(&self.inner.pool, id).await?;
        self.audit("user", "webhook.delete", Some("webhook"), Some(id), None).await;
        Ok(())
    }

    // ---- Forwarding ----

    pub async fn list_forwarding_rules(
        &self,
    ) -> Result<Vec<crate::db::forwarding::ForwardingRule>> {
        crate::db::forwarding::list(&self.inner.pool).await
    }

    pub async fn create_forwarding_rule(
        &self,
        input: crate::db::forwarding::CreateForwardingRule,
    ) -> Result<crate::db::forwarding::ForwardingRule> {
        let rule = crate::db::forwarding::insert(&self.inner.pool, input).await?;
        self.audit(
            "user",
            "forwarding.create",
            Some("forwarding_rule"),
            Some(&rule.id),
            None,
        )
        .await;
        Ok(rule)
    }

    pub async fn delete_forwarding_rule(&self, id: &str) -> Result<()> {
        crate::db::forwarding::delete(&self.inner.pool, id).await?;
        self.audit(
            "user",
            "forwarding.delete",
            Some("forwarding_rule"),
            Some(id),
            None,
        )
        .await;
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
