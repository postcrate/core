# Embedding `postcrate-core` as a library

The whole engine is one type — `Service`. Build it, optionally hook into its event stream, and you have an SMTP server you can drive from your own code.

## Cargo

```toml
[dependencies]
postcrate-core = { git = "https://github.com/postcrate/postcrate-core" }   # or path = "../postcrate-core/crates/postcrate-core"
tokio = { version = "1", features = ["macros", "rt-multi-thread", "signal"] }
```

## Minimum viable embed

```rust
use std::sync::Arc;
use postcrate_core::{CoreConfig, LogSink, Service};

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    let cfg = CoreConfig::for_data_dir("/tmp/postcrate")?;
    let service = Service::build(cfg, Arc::new(LogSink)).await?;
    service.start_all().await?;

    // … do work …

    service.stop_all().await?;
    Ok(())
}
```

## Custom event sink (your own UI)

```rust
use std::sync::Arc;
use postcrate_core::{CoreEvent, EventSink};

struct MySink;

impl EventSink for MySink {
    fn emit(&self, event: CoreEvent) {
        match event {
            CoreEvent::NewEmail { mailbox_id, email } => {
                // push to your websocket / Tauri AppHandle / etc.
            }
            _ => {}
        }
    }
}

let service = Service::build(cfg, Arc::new(MySink)).await?;
```

## Per-test ephemeral mailbox (for matcher packages)

```rust
use postcrate_core::{CreateEphemeralInput, EphemeralHandle};

let h: EphemeralHandle = service
    .create_ephemeral(CreateEphemeralInput {
        project_id: "tests".into(),
        name: None,
        ttl_seconds: 60,
    })
    .await?;

println!("send to {}:{}", h.host, h.port);

let mailbox_id = h.id;
let messages = service.list_emails(&mailbox_id, 100, 0).await?;
```

## Chaos / bounces

```rust
use postcrate_core::ChaosConfig;
service
    .set_chaos(
        &mailbox_id,
        ChaosConfig {
            enabled: true,
            reject_5xx_prob: 1.0,
            seed: Some(42),
            ..Default::default()
        },
    )
    .await?;
```

```rust
use postcrate_core::{BounceKind, BounceRule};

let rule = BounceRule {
    id: String::new(),
    mailbox_id: mailbox_id.clone(),
    address_pattern: "bounce@*".into(),
    bounce_kind: BounceKind::Hard,
    smtp_code: 550,
    smtp_message: "User unknown".into(),
    enabled: true,
    created_at: 0,
};
service.upsert_bounce_rule(rule).await?;
```

## Search

```rust
let results = service.search_emails("verify", Some(&mailbox_id), 50).await?;
```

## Lifecycle notes

- `Service::build` opens the SQLite pool, runs migrations, and spawns three background tasks (ingest worker, retention sweeper, TTL scheduler). They live as long as the `Service`.
- `Service::start_all` binds every persisted mailbox's listener + the HTTP API. Idempotent.
- `Service::stop_all` shuts both down. Background tasks (`ingest`, `retention`, `ttl`) are cancelled when the `Service` is dropped — its inner `CancellationToken` is held in the same `Arc`.

## What's NOT in scope

The library does not embed an MCP server, a CLI, or a UI. For those:

- `postcrate-server` — headless daemon binary.
- `postcrate-ci` — fast-start CI variant.
- The (downstream) Tauri repo — desktop UI that consumes this crate via path dependency.
