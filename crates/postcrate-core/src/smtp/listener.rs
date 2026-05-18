//! TCP accept loop for one mailbox.

use std::net::SocketAddr;
use std::sync::Arc;

use tokio::net::TcpListener;
use tokio::sync::mpsc;
use tokio::task::JoinHandle;
use tokio_util::sync::CancellationToken;

use crate::error::Result;
use crate::smtp::bounce::BounceEvaluator;
use crate::smtp::chaos::ChaosInjector;
use crate::smtp::extensions::EhloAdvert;
use crate::smtp::session::{run_session, CapturedEnvelope, SessionCtx, SessionOutcome};
use crate::smtp::tls::TlsAcceptor;

#[derive(Debug)]
pub struct ListenerHandle {
    pub mailbox_id: String,
    pub addr: SocketAddr,
    pub cancel: CancellationToken,
    pub task: JoinHandle<()>,
}

pub struct ListenerSpec {
    pub mailbox_id: String,
    pub bind: SocketAddr,
    pub ehlo_advert: EhloAdvert,
    pub max_line: usize,
    pub max_bytes: u64,
    pub spill_at: usize,
    pub incoming_dir: std::path::PathBuf,
    pub chaos: ChaosInjector,
    pub bounce: Arc<parking_lot::RwLock<BounceEvaluator>>,
    pub ingest_tx: mpsc::Sender<CapturedEnvelope>,
    /// Optional STARTTLS acceptor. When `Some`, the listener will offer
    /// STARTTLS in EHLO and upgrade sessions that request it. When
    /// `None`, STARTTLS is rejected with `454 TLS not available`.
    pub tls_acceptor: Option<Arc<TlsAcceptor>>,
}

pub async fn start(spec: ListenerSpec) -> Result<ListenerHandle> {
    let listener = TcpListener::bind(spec.bind).await?;
    let addr = listener.local_addr()?;
    let cancel = CancellationToken::new();
    let mailbox_id = spec.mailbox_id.clone();
    let cancel_child = cancel.clone();

    let task = tokio::spawn(async move {
        accept_loop(listener, cancel_child, spec).await;
    });

    Ok(ListenerHandle {
        mailbox_id,
        addr,
        cancel,
        task,
    })
}

async fn accept_loop(
    listener: TcpListener,
    cancel: CancellationToken,
    spec: ListenerSpec,
) {
    let ehlo = Arc::new(spec.ehlo_advert);
    let incoming_dir = Arc::new(spec.incoming_dir);

    loop {
        tokio::select! {
            _ = cancel.cancelled() => {
                tracing::info!(target: "postcrate::smtp", mailbox = %spec.mailbox_id, "listener cancelled");
                return;
            }
            res = listener.accept() => match res {
                Ok((stream, peer)) => {
                    tracing::debug!(target: "postcrate::smtp", mailbox = %spec.mailbox_id, %peer, "accepted");
                    let ctx = SessionCtx {
                        mailbox_id: spec.mailbox_id.clone(),
                        ehlo_advert: (*ehlo).clone(),
                        max_line: spec.max_line,
                        max_bytes: spec.max_bytes,
                        spill_at: spec.spill_at,
                        incoming_dir: (*incoming_dir).clone(),
                        chaos: spec.chaos.clone(),
                        bounce: spec.bounce.clone(),
                        ingest_tx: spec.ingest_tx.clone(),
                        tls_active: false,
                    };
                    let acceptor = spec.tls_acceptor.clone();
                    tokio::spawn(handle_connection(stream, ctx, acceptor));
                }
                Err(e) => {
                    tracing::warn!(target: "postcrate::smtp", error = %e, "accept failed");
                    // Brief backoff to avoid a tight loop on persistent errors.
                    tokio::time::sleep(std::time::Duration::from_millis(50)).await;
                }
            }
        }
    }
}

async fn handle_connection(
    stream: tokio::net::TcpStream,
    ctx: SessionCtx,
    tls_acceptor: Option<Arc<TlsAcceptor>>,
) {
    let outcome = match run_session(stream, ctx.clone()).await {
        Ok(o) => o,
        Err(e) => {
            tracing::warn!(target: "postcrate::smtp", error = %e, "session error");
            return;
        }
    };

    let stream = match outcome {
        SessionOutcome::Closed => return,
        SessionOutcome::UpgradeTls(stream) => stream,
    };

    #[cfg(feature = "tls")]
    {
        let Some(acceptor) = tls_acceptor else {
            // We advertised STARTTLS but lost the acceptor — abnormal.
            tracing::warn!(target: "postcrate::smtp", "upgrade requested with no acceptor configured");
            return;
        };
        let tls_stream = match acceptor.accept(stream).await {
            Ok(s) => s,
            Err(e) => {
                tracing::warn!(target: "postcrate::smtp", error = %e, "TLS handshake failed");
                return;
            }
        };
        let mut tls_ctx = ctx;
        tls_ctx.tls_active = true;
        if let Err(e) = run_session(tls_stream, tls_ctx).await {
            tracing::warn!(target: "postcrate::smtp", error = %e, "session error (TLS)");
        }
    }

    #[cfg(not(feature = "tls"))]
    {
        // When the feature is off we never advertise STARTTLS, so the
        // session never returns UpgradeTls. The branch is here only to
        // make types line up.
        let _ = (stream, tls_acceptor);
    }
}
