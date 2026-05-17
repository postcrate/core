# postcrate-core

The mail engine that powers [Postcrate](https://postcrate.dev). A standalone, Tokio-native SMTP capture server with a local HTTP API, multi-mailbox lifecycle, chaos simulation, and SQLite persistence. Ships as a library, a headless binary, and a CI variant — no UI dependency.

## Crates

| Crate | Purpose |
|-------|---------|
| `postcrate-core`   | Library. `Service` façade exposing every operation. |
| `postcrate-server` | Mailpit-style headless daemon (`postcrate run ...`). |
| `postcrate-ci`     | Fast-start CI variant; prints env line on ready. |

## Quick start (headless server)

```sh
cargo run -p postcrate-server -- run --smtp 1025 --http 1080
swaks --to a@b --server 127.0.0.1:1025 --body "hello"
curl http://127.0.0.1:1080/api/v1/messages
```

## Embedding

```rust
use std::sync::Arc;
use postcrate_core::{Service, CoreConfig, LogSink};

# async fn _example() -> anyhow::Result<()> {
let cfg = CoreConfig::for_data_dir("/tmp/pc")?;
let service = Service::build(cfg, Arc::new(LogSink)).await?;
service.start_all().await?;
# Ok(()) }
```

See `docs/EMBEDDING.md` for the full surface.

## License

MIT.
