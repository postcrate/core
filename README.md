# postcrate-core

[![CI](https://img.shields.io/github/actions/workflow/status/postcrate/postcrate-core/ci.yml?branch=main)](https://github.com/postcrate/postcrate-core/actions)
[![Crates.io](https://img.shields.io/crates/v/postcrate-core.svg)](https://crates.io/crates/postcrate-core)
[![Docs.rs](https://img.shields.io/docsrs/postcrate-core)](https://docs.rs/postcrate-core)
[![License: MIT](https://img.shields.io/badge/license-MIT-blue.svg)](./LICENSE)

A Tokio-native SMTP capture engine for local development, integration tests, and CI.

`postcrate-core` is the engine behind [Postcrate](https://postcrate.dev) — a self-contained Rust library that listens for SMTP, parses incoming mail, stores it in SQLite, and exposes everything over a small HTTP API. It has no dependency on Tauri, on a UI, or on any third-party mail service. You can embed it in your own binary, run it as a daemon, or drop it into a CI job.

```rust
use std::sync::Arc;
use postcrate_core::{CoreConfig, LogSink, Service};

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    let cfg = CoreConfig::for_data_dir("/tmp/postcrate")?;
    let service = Service::build(cfg, Arc::new(LogSink)).await?;
    service.start_all().await?;

    // The service is now listening on the configured SMTP and HTTP ports.
    tokio::signal::ctrl_c().await?;
    service.stop_all().await?;
    Ok(())
}
```

## Features

- **SMTP server** — `EHLO/HELO`, `MAIL FROM` / `RCPT TO`, `DATA` with dot-stuffing and RFC 5321 line limits, `RSET`, `NOOP`, `QUIT`, `VRFY`, `HELP`. Advertises `PIPELINING`, `SIZE`, `8BITMIME`, `SMTPUTF8`, `ENHANCEDSTATUSCODES`, `AUTH PLAIN LOGIN`. Optional `STARTTLS` and implicit-TLS (RFC 8314) under the `tls` feature.
- **Multi-mailbox lifecycle** — primary, shared, and ephemeral mailboxes with TTL-based auto-expiry. Per-mailbox port allocation and live config (chaos, bounce rules, tags, pins).
- **MIME parsing** — built on [`mail-parser`](https://crates.io/crates/mail-parser). UTF-8 headers, RFC 2047 encoded-words, multipart trees, inline images via `cid:` references, attachments with filename* encoding.
- **SQLite persistence** — `sqlx` with WAL, foreign keys on, FTS5 for full-text search across subject/sender/recipients/body.
- **HTTP API** — small Axum router under `/api/v1`. JSON in/out, optional bearer-token auth, optional HTTPS, server-sent events for real-time UI hooks.
- **Chaos mode** — deterministic-seeded SMTP failure injection (4xx, 5xx, mid-DATA drops, response jitter).
- **Bounce rules** — glob-pattern address matching that responds with custom codes at RCPT TO.
- **Webhooks & forwarding** — POST to a downstream URL on every captured email, or relay it to a real SMTP server with an optional recipient allow-list.
- **Scenario checks** — link extraction, spam heuristics, SPF/DKIM/DMARC inspection, RFC 2369 / 8058 `List-Unsubscribe` validation, accessibility lint, 7-client rendering profiles with fidelity badges.
- **Mailtrap-compatible alias** — a small subset of the Mailtrap REST surface is exposed at `/api/accounts/…` so existing test suites can drop in without code changes.

## Workspace layout

| Crate | Purpose |
|---|---|
| `postcrate-core`   | The library. Public surface is the [`Service`] type. |
| `postcrate-server` | Headless daemon binary (`postcrate run ...`). |
| `postcrate-ci`     | Fast-start CI variant; prints an env line on ready. |

## Install

As a library:

```toml
[dependencies]
postcrate-core = "0.1"
```

As a daemon:

```sh
cargo install postcrate-server          # the `postcrate` binary
postcrate run --smtp 1025 --http 1080
```

As a CI helper:

```sh
cargo install postcrate-ci
eval "$(postcrate-ci --data-dir /tmp/pc)"
# now $POSTCRATE_SMTP_PORT, $POSTCRATE_API_URL are set for the rest of the job
```

## Quick start

```sh
cargo run -p postcrate-server -- run --smtp 1025 --http 1080
swaks --to a@b --server 127.0.0.1:1025 --body "hello"
curl http://127.0.0.1:1080/api/v1/messages
```

The TLS path is gated on the `tls` feature:

```sh
cargo run -p postcrate-server --features postcrate-core/tls -- run --smtp 1025 --http 1080
```

## Documentation

- API reference — [docs.rs/postcrate-core](https://docs.rs/postcrate-core)
- Architecture, HTTP API, SMTP extensions, embedding guide — [postcrate/postcrate-docs](https://github.com/postcrate/postcrate-docs)

## Minimum supported Rust version

1.78 (set via `rust-toolchain.toml`).

## License

MIT — see [`LICENSE`](./LICENSE).
