//! Embed `postcrate_core::Service` in your own Rust project.
//!
//!     cargo run --example embed
//!
//! On startup we boot a Service against a temp dir, create a primary
//! mailbox on port 1099, and print the HTTP URL. Press Ctrl-C to exit.

use std::sync::Arc;

use postcrate_core::{
    CoreConfig, CreateMailboxInput, LogSink, MailboxKind, Service,
};

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    tracing_subscriber::fmt::init();

    let tmp = std::env::temp_dir().join("postcrate-embed-example");
    let mut cfg = CoreConfig::for_data_dir(&tmp)?;
    cfg.http_port = 18080;
    cfg.default_smtp_port = 11099;

    let svc = Service::build(cfg, Arc::new(LogSink)).await?;
    svc.start_all().await?;

    let _ = svc
        .create_mailbox(CreateMailboxInput {
            project_id: "embed".into(),
            name: "demo".into(),
            kind: MailboxKind::Primary,
            port: Some(11099),
            ttl_seconds: None,
            implicit_tls: false,
        })
        .await;

    println!("SMTP: 127.0.0.1:11099");
    println!("HTTP: http://127.0.0.1:18080");
    println!("Data: {}", tmp.display());
    println!("Ctrl-C to exit.");

    tokio::signal::ctrl_c().await?;
    svc.stop_all().await?;
    Ok(())
}
