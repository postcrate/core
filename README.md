# postcrate-core

[![Crates.io](https://img.shields.io/crates/v/postcrate-core.svg)](https://crates.io/crates/postcrate-core)
[![Docs.rs](https://img.shields.io/docsrs/postcrate-core)](https://docs.rs/postcrate-core)
[![License: MIT](https://img.shields.io/badge/license-MIT-blue.svg)](./LICENSE)

A Tokio-native SMTP capture engine for local development, integration tests, and CI.

`postcrate-core` is the engine that powers the [Postcrate](https://github.com/postcrate/postcrate) desktop app — a self-contained Rust library that listens for SMTP, parses incoming mail, stores it in SQLite, and exposes everything over a small HTTP API. No Tauri, no UI, no third-party service. Embed it in your own binary, run it as a daemon, or drop it into a CI job.

## Status

Pre-1.0. The public API may change between `0.x` minor versions until `1.0` lands. Already used in production by the Postcrate desktop app.

## Install

```toml
[dependencies]
postcrate-core = "0.1"
```

## Quick start

```rust
use std::sync::Arc;
use postcrate_core::{CoreConfig, LogSink, Service};

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    let cfg = CoreConfig::for_data_dir("/tmp/postcrate")?;
    let service = Service::build(cfg, Arc::new(LogSink)).await?;
    service.start_all().await?;

    // SMTP listeners are accepting mail; the HTTP API is serving under /api/v1.
    tokio::signal::ctrl_c().await?;
    service.stop_all().await?;
    Ok(())
}
```

Subscribe to live events by passing a custom `EventSink` implementation in place of `LogSink`.

## Features

- **SMTP server** — `EHLO/HELO`, `MAIL FROM` / `RCPT TO`, `DATA` with dot-stuffing and RFC 5321 line limits, `RSET`, `NOOP`, `QUIT`, `VRFY`, `HELP`. Advertises `PIPELINING`, `SIZE`, `8BITMIME`, `SMTPUTF8`, `ENHANCEDSTATUSCODES`, `AUTH PLAIN LOGIN`. Optional `STARTTLS` and implicit-TLS (RFC 8314) under the `tls` feature.
- **Multi-mailbox lifecycle** — primary, shared, and ephemeral mailboxes with TTL-based auto-expiry. Per-mailbox port allocation and live config (chaos, bounce rules, tags, pins).
- **MIME parsing** — built on [`mail-parser`](https://crates.io/crates/mail-parser). UTF-8 headers, RFC 2047 encoded-words, multipart trees, inline images via `cid:` references, attachments with `filename*` encoding.
- **SQLite persistence** — `sqlx` with WAL, foreign keys on, FTS5 full-text search across subject, sender, recipients, and body.
- **HTTP API** — small Axum router under `/api/v1`. JSON in/out, optional bearer-token auth, optional HTTPS, server-sent events for real-time UI hooks.
- **Chaos mode** — deterministic-seeded SMTP failure injection (4xx, 5xx, mid-`DATA` drops, response jitter).
- **Bounce rules** — glob-pattern address matching that responds with custom SMTP codes at `RCPT TO`.
- **Webhooks & forwarding** — POST to a downstream URL on every captured email, or relay it to a real SMTP server with an optional recipient allow-list.
- **Scenario checks** — link extraction, spam heuristics, SPF/DKIM/DMARC inspection, RFC 2369 / 8058 `List-Unsubscribe` validation, accessibility lint, 7-client rendering profiles with fidelity badges.
- **Mailtrap-compatible alias** — a subset of the Mailtrap REST surface is exposed at `/api/accounts/…` so existing test suites can drop in without code changes.

## Crate features

| Feature  | Effect                                                                                     |
| -------- | ------------------------------------------------------------------------------------------ |
| `tls`    | Enables `STARTTLS` and implicit-TLS listeners (RFC 8314). Off by default.                  |
| `specta` | Derives `specta::Type` on every public DTO for TypeScript binding generation. No runtime effect. |

Enable both:

```toml
postcrate-core = { version = "0.1", features = ["tls", "specta"] }
```

## Workspace layout

This crate ships in a Cargo workspace alongside two unpublished binaries:

| Crate              | What it is                                                              |
| ------------------ | ----------------------------------------------------------------------- |
| `postcrate-core`   | This library. Public surface is the `Service` type.                     |
| `postcrate-server` | Headless daemon binary (`postcrate run …`). Build from source.          |
| `postcrate-ci`     | Fast-start CI variant; prints an env line on ready. Build from source.  |

To run the daemon from source:

```sh
git clone git@github.com:postcrate/core.git
cd core
cargo run -p postcrate-server -- run --smtp 1025 --http 1080
```

The TLS path is gated on the `tls` feature:

```sh
cargo run -p postcrate-server --features postcrate-core/tls -- run --smtp 1025 --http 1080
```

## Minimum supported Rust version

1.78 (pinned via `rust-toolchain.toml`).

## License

MIT — see [`LICENSE`](./LICENSE).
