//! `postcrate-ci` — fast-start headless variant for CI pipelines.
//!
//! Differences from `postcrate`:
//!   * Uses an OS-provided temp dir by default (always fresh).
//!   * Emits `POSTCRATE_*` env lines to stdout once ready so a shell
//!     wrapper can `eval $(postcrate-ci ...)`.
//!   * Exits cleanly on SIGTERM/SIGINT only — no daemonization helpers.

use std::path::PathBuf;
use std::sync::Arc;

use anyhow::Result;
use clap::Parser;
use postcrate_core::{
    config::BindHost, CoreConfig, CreateMailboxInput, LogSink, MailboxKind, Service, SettingsPatch,
};
use tracing_subscriber::EnvFilter;

#[derive(Parser, Debug)]
#[command(name = "postcrate-ci", version, about = "Fast-start CI variant of postcrate")]
struct Cli {
    #[arg(long, default_value_t = 1025)]
    smtp: u16,
    #[arg(long, default_value_t = 1080)]
    http: u16,
    #[arg(long, default_value = "127.0.0.1")]
    bind: String,
    #[arg(long)]
    data_dir: Option<PathBuf>,
}

#[tokio::main(flavor = "multi_thread", worker_threads = 2)]
async fn main() -> Result<()> {
    let filter = EnvFilter::try_from_default_env().unwrap_or_else(|_| EnvFilter::new("warn"));
    tracing_subscriber::fmt().with_env_filter(filter).init();

    let cli = Cli::parse();

    let data_dir = cli.data_dir.unwrap_or_else(|| {
        std::env::temp_dir().join(format!("postcrate-ci-{}", std::process::id()))
    });
    let mut cfg = CoreConfig::for_data_dir(&data_dir)?;
    cfg.bind_host = if cli.bind == "0.0.0.0" {
        BindHost::AllInterfaces
    } else {
        BindHost::Loopback
    };
    cfg.http_port = cli.http;
    cfg.default_smtp_port = cli.smtp;

    let svc = Service::build(cfg.clone(), Arc::new(LogSink)).await?;
    let mut net = svc.get_settings().await?.network;
    net.smtp_port = cli.smtp;
    net.http_api_port = cli.http;
    net.expose_on_lan = matches!(cfg.bind_host, BindHost::AllInterfaces);
    svc.update_settings(SettingsPatch::Network(net)).await?;

    let _ = svc
        .create_mailbox(CreateMailboxInput {
            project_id: "ci".into(),
            name: "ci".into(),
            kind: MailboxKind::Primary,
            port: Some(cli.smtp),
            ttl_seconds: None,
            implicit_tls: false,
        })
        .await;
    svc.start_all().await?;

    let bind = cfg.bind_host.as_ip();
    println!("POSTCRATE_SMTP_HOST={bind}");
    println!("POSTCRATE_SMTP_PORT={}", cli.smtp);
    println!("POSTCRATE_API_URL=http://{bind}:{}", cli.http);
    println!("POSTCRATE_DATA_DIR={}", data_dir.display());
    use std::io::Write;
    let _ = std::io::stdout().flush();

    tokio::signal::ctrl_c().await?;
    svc.stop_all().await?;
    Ok(())
}
