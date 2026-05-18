//! `postcrate` — headless SMTP capture daemon.
//!
//! Usage:
//!     postcrate run --smtp 1025 --http 1080 --data-dir /tmp/pc
//!     postcrate version
//!     postcrate doctor --data-dir /tmp/pc

use std::path::PathBuf;
use std::sync::Arc;

use anyhow::{Context, Result};
use clap::{Parser, Subcommand};
use postcrate_core::{CoreConfig, LogSink, MailboxKind, Service, SettingsPatch};
use tracing_subscriber::EnvFilter;

#[derive(Parser, Debug)]
#[command(name = "postcrate", version, about = "SMTP capture daemon with a local HTTP API")]
struct Cli {
    #[command(subcommand)]
    cmd: Cmd,
}

#[derive(Subcommand, Debug)]
enum Cmd {
    /// Run the daemon.
    Run {
        #[arg(long, default_value_t = 1025, env = "POSTCRATE_SMTP_PORT")]
        smtp: u16,

        #[arg(long, default_value_t = 1080, env = "POSTCRATE_HTTP_PORT")]
        http: u16,

        #[arg(long, default_value = "127.0.0.1", env = "POSTCRATE_BIND")]
        bind: String,

        #[arg(long, env = "POSTCRATE_DATA_DIR")]
        data_dir: Option<PathBuf>,

        #[arg(long, default_value_t = 50 * 1024 * 1024)]
        max_size: u64,

        /// Ephemeral port range, e.g. `1100-1199`.
        #[arg(long, default_value = "1100-1199")]
        ephemeral_range: String,

        #[arg(long, short, action = clap::ArgAction::Count)]
        verbose: u8,

        #[arg(long, short)]
        quiet: bool,
    },

    /// Print version.
    Version,

    /// Pre-flight check.
    Doctor {
        #[arg(long, env = "POSTCRATE_DATA_DIR")]
        data_dir: Option<PathBuf>,
        #[arg(long, default_value_t = 1025)]
        smtp: u16,
        #[arg(long, default_value_t = 1080)]
        http: u16,
        #[arg(long, default_value = "127.0.0.1")]
        bind: String,
    },
}

#[tokio::main(flavor = "multi_thread")]
async fn main() -> Result<()> {
    let cli = Cli::parse();
    match cli.cmd {
        Cmd::Version => {
            println!("postcrate {}", env!("CARGO_PKG_VERSION"));
            Ok(())
        }
        Cmd::Doctor {
            data_dir,
            smtp,
            http,
            bind,
        } => doctor(data_dir, smtp, http, bind).await,
        Cmd::Run {
            smtp,
            http,
            bind,
            data_dir,
            max_size,
            ephemeral_range,
            verbose,
            quiet,
        } => {
            init_tracing(verbose, quiet);
            run_daemon(smtp, http, bind, data_dir, max_size, ephemeral_range).await
        }
    }
}

fn init_tracing(verbose: u8, quiet: bool) {
    let default = if quiet {
        "warn"
    } else {
        match verbose {
            0 => "info,postcrate=info",
            1 => "debug,postcrate=debug",
            _ => "trace,postcrate=trace",
        }
    };
    let filter = EnvFilter::try_from_default_env().unwrap_or_else(|_| EnvFilter::new(default));
    tracing_subscriber::fmt().with_env_filter(filter).init();
}

async fn run_daemon(
    smtp: u16,
    http: u16,
    bind: String,
    data_dir: Option<PathBuf>,
    max_size: u64,
    ephemeral_range: String,
) -> Result<()> {
    let data_dir = match data_dir {
        Some(d) => d,
        None => CoreConfig::default_data_dir().context("resolve data dir")?,
    };
    let mut cfg = CoreConfig::for_data_dir(&data_dir)?;
    cfg.default_smtp_port = smtp;
    cfg.http_port = http;
    cfg.max_message_bytes = max_size;
    cfg.bind_host = if bind == "0.0.0.0" {
        postcrate_core::config::BindHost::AllInterfaces
    } else {
        postcrate_core::config::BindHost::Loopback
    };
    let (lo, hi) = parse_range(&ephemeral_range)?;
    cfg.ephemeral_port_range = (lo, hi);

    let svc = Service::build(cfg, Arc::new(LogSink)).await?;

    // Seed: persist the requested ports + bind so subsequent operations
    // (e.g. ephemeral creation, web UI updates) see them as the source
    // of truth.
    seed_settings(&svc, smtp, http, bind == "0.0.0.0").await?;

    // Boot DB-resident mailboxes first; create the default afterwards so
    // we don't race the boot loop trying to bind the same port twice.
    svc.start_all().await?;
    ensure_default_mailbox(&svc, smtp).await?;
    tracing::info!(
        "postcrate running — smtp :{} http :{} data {}",
        smtp,
        http,
        data_dir.display()
    );

    tokio::signal::ctrl_c().await?;
    tracing::info!("shutdown signal — stopping");
    svc.stop_all().await?;
    Ok(())
}

async fn seed_settings(svc: &Service, smtp: u16, http: u16, expose: bool) -> Result<()> {
    let s = svc.get_settings().await?;
    let mut net = s.network.clone();
    net.smtp_port = smtp;
    net.http_api_port = http;
    net.expose_on_lan = expose;
    if net.smtp_port != s.network.smtp_port
        || net.http_api_port != s.network.http_api_port
        || net.expose_on_lan != s.network.expose_on_lan
    {
        svc.update_settings(SettingsPatch::Network(net)).await?;
    }
    Ok(())
}

async fn ensure_default_mailbox(svc: &Service, smtp_port: u16) -> Result<()> {
    let mailboxes = svc.list_mailboxes(Some("default")).await?;
    if mailboxes.iter().any(|m| matches!(m.kind, MailboxKind::Primary)) {
        return Ok(());
    }
    match svc
        .create_mailbox(postcrate_core::CreateMailboxInput {
            project_id: "default".into(),
            name: "default".into(),
            kind: MailboxKind::Primary,
            port: Some(smtp_port),
            ttl_seconds: None,
            implicit_tls: false,
        })
        .await
    {
        Ok(_) => Ok(()),
        Err(postcrate_core::Error::PortInUse(_) | postcrate_core::Error::DuplicateMailbox(_)) => {
            // Either a race with an existing row or the port is bound
            // by something else. The doctor command exists exactly for
            // this — don't fail boot.
            Ok(())
        }
        Err(e) => Err(e.into()),
    }
}

fn parse_range(s: &str) -> Result<(u16, u16)> {
    let (lo, hi) = s
        .split_once('-')
        .ok_or_else(|| anyhow::anyhow!("range must look like LOW-HIGH"))?;
    Ok((lo.parse()?, hi.parse()?))
}

async fn doctor(
    data_dir: Option<PathBuf>,
    smtp: u16,
    http: u16,
    bind: String,
) -> Result<()> {
    use tokio::net::TcpListener;
    let mut failures = 0u32;

    let data_dir = match data_dir {
        Some(d) => d,
        None => CoreConfig::default_data_dir()?,
    };
    println!("• data dir: {}", data_dir.display());
    if tokio::fs::create_dir_all(&data_dir).await.is_err() {
        println!("  ✗ cannot create");
        failures += 1;
    } else {
        let probe = data_dir.join(".probe");
        match tokio::fs::write(&probe, b"x").await {
            Ok(()) => {
                let _ = tokio::fs::remove_file(&probe).await;
                println!("  ✓ writable");
            }
            Err(e) => {
                println!("  ✗ not writable: {e}");
                failures += 1;
            }
        }
    }

    for (label, port) in [("smtp", smtp), ("http", http)] {
        let addr = format!("{bind}:{port}");
        match TcpListener::bind(&addr).await {
            Ok(_) => println!("• {label} bind {addr}: ✓ free"),
            Err(e) => {
                println!("• {label} bind {addr}: ✗ {e}");
                failures += 1;
            }
        }
    }

    if failures > 0 {
        std::process::exit(1);
    }
    Ok(())
}
