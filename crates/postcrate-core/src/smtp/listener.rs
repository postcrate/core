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
use crate::smtp::session::{run_session, CapturedEnvelope, SessionCtx};

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
                    };
                    tokio::spawn(async move {
                        if let Err(e) = run_session(stream, ctx).await {
                            tracing::warn!(target: "postcrate::smtp", error = %e, "session error");
                        }
                    });
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
