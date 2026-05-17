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
