//! Static configuration for a `Service`. Resolved once at construction.
//! Anything the user can change at runtime lives in the `settings` table,
//! not here.

use std::net::IpAddr;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use crate::error::{Error, Result};

/// Bind selection at startup. Runtime `exposeOnLan` toggles override this
/// at restart time (the running listeners aren't moved).
#[derive(Debug, Clone, Copy, Serialize, Deserialize)]
pub enum BindHost {
    /// `127.0.0.1` — Postcrate's default and only safe choice.
    Loopback,
    /// `0.0.0.0` — opt-in only. Logs a warning at startup.
    AllInterfaces,
}

impl BindHost {
    pub fn as_ip(self) -> IpAddr {
        match self {
            BindHost::Loopback => IpAddr::from([127, 0, 0, 1]),
            BindHost::AllInterfaces => IpAddr::from([0, 0, 0, 0]),
        }
    }
}

/// Configuration for the [`crate::Service`].
#[derive(Debug, Clone)]
pub struct CoreConfig {
    pub data_dir: PathBuf,
    pub db_path: PathBuf,
    pub blobs_dir: PathBuf,
    pub default_smtp_port: u16,
    pub http_port: u16,
    pub bind_host: BindHost,
    pub max_message_bytes: u64,
    pub ephemeral_port_range: (u16, u16),
    /// EHLO hostname advertised to clients. Defaults to `postcrate.local`.
    pub ehlo_hostname: String,
    /// SMTP receive line length (RFC 5321 §4.5.3.1.6 is 1000 incl. CRLF).
    pub smtp_max_line_bytes: usize,
    /// Threshold above which DATA streams to a tempfile.
    pub data_spill_bytes: usize,
    /// Bounded queue size between SMTP sessions and the ingest worker.
    pub ingest_channel_capacity: usize,
}

impl CoreConfig {
    /// Convenience constructor.
    pub fn for_data_dir(data_dir: impl Into<PathBuf>) -> Result<Self> {
        let data_dir = data_dir.into();
        let db_path = data_dir.join("postcrate.sqlite");
        let blobs_dir = data_dir.join("blobs");
        Ok(Self {
            data_dir,
            db_path,
            blobs_dir,
            default_smtp_port: 1025,
            http_port: 1080,
            bind_host: BindHost::Loopback,
            max_message_bytes: 50 * 1024 * 1024,
            ephemeral_port_range: (1100, 1199),
            ehlo_hostname: "postcrate.local".to_string(),
            smtp_max_line_bytes: 1000,
            data_spill_bytes: 256 * 1024,
            ingest_channel_capacity: 1024,
        })
    }

    /// Resolve the platform-appropriate default data directory.
    pub fn default_data_dir() -> Result<PathBuf> {
        // We deliberately avoid pulling in `directories` to keep the dep
        // surface minimal — the binaries take an explicit `--data-dir`,
        // and embedders pass their own.
        if let Ok(home) = std::env::var("HOME") {
            #[cfg(target_os = "macos")]
            {
                let p = Path::new(&home).join("Library/Application Support/Postcrate");
                return Ok(p);
            }
            #[cfg(target_os = "linux")]
            {
                if let Ok(xdg) = std::env::var("XDG_DATA_HOME") {
                    return Ok(Path::new(&xdg).join("postcrate"));
                }
                return Ok(Path::new(&home).join(".local/share/postcrate"));
            }
            #[cfg(not(any(target_os = "macos", target_os = "linux")))]
            {
                return Ok(Path::new(&home).join(".postcrate"));
            }
        }
        if let Ok(appdata) = std::env::var("APPDATA") {
            return Ok(Path::new(&appdata).join("Postcrate"));
        }
        Err(Error::Internal(
            "could not resolve a default data directory; pass one explicitly".into(),
        ))
    }

    pub(crate) fn raw_dir(&self) -> PathBuf {
        self.blobs_dir.join("raw")
    }

    pub(crate) fn incoming_dir(&self) -> PathBuf {
        self.blobs_dir.join("raw").join("incoming")
    }

    pub(crate) fn att_dir(&self) -> PathBuf {
        self.blobs_dir.join("att")
    }

    pub(crate) async fn ensure_dirs(&self) -> Result<()> {
        for dir in [
            &self.data_dir,
            &self.blobs_dir,
            &self.raw_dir(),
            &self.incoming_dir(),
            &self.att_dir(),
        ] {
            tokio::fs::create_dir_all(dir).await?;
        }
        Ok(())
    }
}
