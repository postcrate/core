//! TCP accept loop for one mailbox.

use std::net::SocketAddr;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;

use parking_lot::Mutex;
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
    /// Implicit-TLS (port-465 style). When true, the listener wraps
    /// every accepted socket with `tls_acceptor` *before* the SMTP
    /// banner. Requires `tls_acceptor.is_some()` and `--features tls`.
    /// STARTTLS is not advertised inside an implicit-TLS session
    /// (RFC 8314 §3.3).
    pub implicit_tls: bool,
    /// Live flag for SMTP-transcript capture. Cloned from the mailbox
    /// service; the accept loop reads it per-connection so a pref flip
    /// takes effect on the very next session.
    pub preserve_transcript: Arc<AtomicBool>,
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
    // For implicit TLS we suppress the STARTTLS advert (RFC 8314)
    // since the session is already encrypted.
    let mut ehlo_for_session = spec.ehlo_advert.clone();
    if spec.implicit_tls {
        ehlo_for_session.starttls_enabled = false;
    }
    let ehlo = Arc::new(ehlo_for_session);
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
                    // Allocate a transcript sink only when the pref is
                    // on at *this* accept. Sessions started before the
                    // pref was flipped continue without capture; new
                    // sessions immediately reflect the new value.
                    let transcript = spec
                        .preserve_transcript
                        .load(Ordering::Relaxed)
                        .then(|| Arc::new(Mutex::new(Vec::new())));

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
                        // Implicit TLS: the very first byte over the wire
                        // is a TLS ClientHello. After the wrap, the session
                        // runs with tls_active=true from the start.
                        tls_active: spec.implicit_tls,
                        transcript,
                    };
                    let acceptor = spec.tls_acceptor.clone();
                    let implicit = spec.implicit_tls;
                    tokio::spawn(handle_connection(stream, ctx, acceptor, implicit));
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
    implicit_tls: bool,
) {
    // Implicit TLS path: wrap immediately, then run the session on the
    // TLS stream. STARTTLS isn't reachable because we suppress its
    // EHLO advert in `accept_loop`.
    #[cfg(feature = "tls")]
    if implicit_tls {
        let Some(acceptor) = tls_acceptor.clone() else {
            tracing::warn!(target: "postcrate::smtp", "implicit_tls set but no acceptor configured");
            return;
        };
        let tls_stream = match acceptor.accept(stream).await {
            Ok(s) => s,
            Err(e) => {
                tracing::warn!(target: "postcrate::smtp", error = %e, "implicit TLS handshake failed");
                return;
            }
        };
        if let Err(e) = run_session(tls_stream, ctx).await {
            tracing::warn!(target: "postcrate::smtp", error = %e, "session error (implicit TLS)");
        }
        return;
    }

    #[cfg(not(feature = "tls"))]
    {
        let _ = implicit_tls;
    }

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
        let _ = (stream, tls_acceptor);
    }
}
