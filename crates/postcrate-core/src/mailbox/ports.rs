//! Port reservation for ephemeral mailboxes.
//!
//! We scan a configured range, skip ports the database already claims,
//! and probe-bind to detect external collisions. We never use OS-assigned
//! port 0 — callers (test code, CI) need a deterministic port before
//! the listener starts so they can wire env vars.

use std::collections::HashSet;
use std::net::{IpAddr, SocketAddr};

use tokio::net::TcpListener;

use crate::error::{Error, Result};

#[derive(Debug, Clone)]
pub struct PortAllocator {
    range_lo: u16,
    range_hi: u16,
    /// Ports we've handed out this process but the DB may not reflect yet.
    reserved: HashSet<u16>,
}

impl PortAllocator {
    pub fn new(lo: u16, hi: u16) -> Self {
        let (lo, hi) = if lo <= hi { (lo, hi) } else { (hi, lo) };
        Self {
            range_lo: lo,
            range_hi: hi,
            reserved: HashSet::new(),
        }
    }

    /// Try to find a free port in our range.
    pub async fn reserve(
        &mut self,
        bind_host: IpAddr,
        db_ports: &HashSet<u16>,
    ) -> Result<u16> {
        for p in self.range_lo..=self.range_hi {
            if db_ports.contains(&p) || self.reserved.contains(&p) {
                continue;
            }
            if probe_bind(bind_host, p).await.is_ok() {
                self.reserved.insert(p);
                return Ok(p);
            }
        }
        Err(Error::PortRangeExhausted)
    }

    /// Mark a port free again (called when the mailbox dies).
    pub fn release(&mut self, port: u16) {
        self.reserved.remove(&port);
    }

    /// Record a port as reserved without probing. Used to commit a
    /// reservation that was made via a temporary snapshot.
    pub fn mark_reserved(&mut self, port: u16) {
        self.reserved.insert(port);
    }
}

async fn probe_bind(host: IpAddr, port: u16) -> Result<()> {
    let addr = SocketAddr::new(host, port);
    let l = TcpListener::bind(addr).await?;
    drop(l);
    Ok(())
}

/// Walk upward from `start` (or 1025, whichever is larger) looking for
/// a port that is *both* free in this DB (`taken`) and not currently
/// bound by anything else on the host (probe-tested with a real
/// `TcpListener::bind`). Returns the first hit.
///
/// Does NOT reserve the port — the caller may proceed to use it
/// however they like. If a racing `create_mailbox` claims it first,
/// the actual create will fail with `Error::PortInUse(port)` and the
/// UI revalidates. That's the right behavior: suggestions are
/// advisory, the create is authoritative.
///
/// Errors with `Error::PortRangeExhausted` if nothing in `[start, 65535]`
/// is free — extremely unlikely on a normal box, but possible on a
/// loaded build agent or in a tight test sandbox.
pub async fn find_free_port(
    start: u16,
    bind_host: IpAddr,
    taken: &HashSet<u16>,
) -> Result<u16> {
    // Refuse to suggest reserved/privileged ports (<1024). The OS
    // would also reject the bind, but failing fast with the right
    // message is friendlier than a misleading PermissionDenied.
    let mut p = start.max(1025);
    while p < u16::MAX {
        if !taken.contains(&p) && probe_bind(bind_host, p).await.is_ok() {
            return Ok(p);
        }
        p = p.saturating_add(1);
    }
    Err(Error::PortRangeExhausted)
}
