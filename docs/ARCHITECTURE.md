# Architecture

`postcrate-core` is structured as a single library crate with one public façade — `Service` — and a small set of internal subsystems that the façade orchestrates. Two binary crates (`postcrate-server`, `postcrate-ci`) wrap that façade with different process-lifetime semantics.

```
                            ┌────────────────────────┐
                            │      Service           │  ◀── only public type
                            │   (service.rs)         │      that mutates state
                            └────────┬───────────────┘
                                     │
                ┌────────────────────┼──────────────────────┐
                ▼                    ▼                      ▼
       MailboxService         Pipeline ingest worker   HTTP routes (Axum)
       (mailbox/service)      (pipeline/ingest)         (http/routes/…)
                │                    │                      │
                ▼                    ▼                      │
     SMTP listeners (1+)        SQLite via sqlx ◀───────────┘
      (smtp/listener)              (db/…)
                │
                ▼
         Per-connection
         SMTP session
       (smtp/session.rs)
                │
                ▼
      mpsc::channel ───────────► pipeline ingest worker
   (CapturedEnvelope)            (single writer)
```

## Data flow on capture

1. **Accept** — `mailbox::service::MailboxService` owns a `DashMap<MailboxId, ListenerHandle>`. Each entry runs an accept loop in `smtp/listener.rs`.
2. **Session** — every accepted connection spawns a Tokio task running `smtp/session.rs::run_session`. The session is generic over `Io: AsyncRead + AsyncWrite + Unpin` so STARTTLS can later swap the stream.
3. **Chaos / bounce** — three chaos hook points per command (`pre-banner`, `pre-response delay`, `post-parse rejection roll`). Bounce rules are consulted at RCPT TO via `smtp/bounce.rs::BounceEvaluator`, which lives in an `Arc<RwLock<…>>` owned by `MailboxService` so updates apply live.
4. **DATA** — `smtp/data_reader.rs` reads CRLF-terminated lines, un-dot-stuffs, enforces RFC 5321's 1000-octet line limit, spills to a tempfile once a configurable threshold (default 256 KiB) is crossed.
5. **Handoff** — on a clean `.\r\n` the session pushes a `CapturedEnvelope` onto a bounded `mpsc::channel(1024)` and replies `250`. The channel is the only synchronization point between the session tasks and the writer.
6. **Ingest** — `pipeline/ingest.rs` drains the channel in a single task: parses via `mail-parser`, writes attachment blobs, opens one SQL transaction that inserts `emails` + FTS row + `attachments`, then emits `CoreEvent::NewEmail` via the configured `EventSink`. The single-writer model matches SQLite's actual concurrency.
7. **Retention** — after each insert, `pipeline/retention.rs::cap_per_mailbox` enforces the `inbox.maxRetainedEmails` cap. A separate hourly task handles `auto_clear_after_days` and `audit_retain_days`.

## State ownership

| State | Type | Reason |
|---|---|---|
| Listener handles | `DashMap<MailboxId, ListenerHandle>` | Lock-free per-key; we never iterate the map under a held lock. |
| Bounce evaluators | `DashMap<MailboxId, Arc<RwLock<BounceEvaluator>>>` | Live-updatable from `Service::upsert_bounce_rule` without restarting the listener. |
| Port allocator | `parking_lot::Mutex<PortAllocator>` | Short critical section; never held across an `.await`. |
| HTTP server handle | `parking_lot::Mutex<Option<HttpServerHandle>>` | Toggled at `start_all` / `stop_all`. |

## Decoupling

- `postcrate-core` has **zero dependency on Tauri** or any UI framework.
- All events go through the `EventSink` trait (`events.rs`). The crate ships three sinks: `LogSink` (tracing), `ChannelSink` (broadcast for tests/CLI), and `ComposedSink` (fan-out).
- A Tauri shell implements `EventSink` for an `AppHandle` and wires `#[tauri::command]` shims over `Service` methods. That glue is in the downstream Tauri repo, not here.

## TLS path (deferred)

`smtp/tls.rs` is a stub. When TLS is enabled:

1. Add `tokio-rustls` + `rustls-pemfile` behind the `tls` feature in `Cargo.toml`.
2. Implement `upgrade_to_tls(stream, cert, key)` here.
3. Flip `EhloAdvert.starttls_enabled = true`.
4. Add a STARTTLS handler in `session.rs` that calls `upgrade_to_tls` before the next EHLO round.

The session loop being generic over `Io` means no other module changes.
