//! STARTTLS support.
//!
//! The session state machine is generic over the I/O type, so the
//! upgrade itself happens here and in the listener — the session just
//! signals "I want to upgrade" by returning `SessionOutcome::UpgradeTls`.
//!
//! Behind the `tls` feature flag. Without the feature: `STARTTLS` is
//! never advertised in EHLO, the session replies `454 TLS not
//! available` if a client asks anyway, and this module compiles down
//! to a thin stub.

use std::path::PathBuf;
use std::sync::Arc;

use crate::error::{Error, Result};

/// Boot-time TLS configuration. Stored on [`CoreConfig`].
#[derive(Debug, Clone, Default)]
pub struct TlsConfig {
    /// Master switch. If false (the default), STARTTLS is never offered.
    pub enabled: bool,
    /// PEM-encoded certificate chain.
    pub cert_path: Option<PathBuf>,
    /// PEM-encoded private key (PKCS#8 or RSA).
    pub key_path: Option<PathBuf>,
}

#[cfg(feature = "tls")]
pub type TlsAcceptor = tokio_rustls::TlsAcceptor;

#[cfg(not(feature = "tls"))]
pub type TlsAcceptor = ();

/// Load PEM cert + key into a reusable [`TlsAcceptor`].
///
/// When the `tls` feature is off this always errors with
/// `NotImplemented` — callers should check `cfg(feature = "tls")` or
/// rely on the listener layer to skip this entirely.
pub fn load_acceptor(
    cert_path: &std::path::Path,
    key_path: &std::path::Path,
) -> Result<Arc<TlsAcceptor>> {
    #[cfg(feature = "tls")]
    {
        real::load_acceptor(cert_path, key_path)
    }
    #[cfg(not(feature = "tls"))]
    {
        let _ = (cert_path, key_path);
        Err(Error::NotImplemented(
            "TLS support not compiled in — rebuild with --features tls",
        ))
    }
}

/// Convenience: build an acceptor from a [`TlsConfig`] only when both
/// paths are present and the feature is on. Returns `Ok(None)` when
/// TLS is disabled or unconfigured.
pub fn maybe_acceptor(cfg: &TlsConfig) -> Result<Option<Arc<TlsAcceptor>>> {
    if !cfg.enabled {
        return Ok(None);
    }
    #[cfg(feature = "tls")]
    {
        let cert = cfg
            .cert_path
            .as_ref()
            .ok_or_else(|| Error::Invalid("tls.cert_path required when tls.enabled".into()))?;
        let key = cfg
            .key_path
            .as_ref()
            .ok_or_else(|| Error::Invalid("tls.key_path required when tls.enabled".into()))?;
        let acceptor = real::load_acceptor(cert, key)?;
        Ok(Some(acceptor))
    }
    #[cfg(not(feature = "tls"))]
    {
        // The user asked for TLS but the binary doesn't have it. Surface
        // the misconfiguration loudly — silently falling back to plain
        // would be a security footgun.
        Err(Error::NotImplemented(
            "tls.enabled=true but binary was built without --features tls",
        ))
    }
}

#[cfg(feature = "tls")]
mod real {
    use std::fs::File;
    use std::io::BufReader;
    use std::path::Path;
    use std::sync::Arc;

    use rustls_pemfile::{certs, pkcs8_private_keys, rsa_private_keys};
    use tokio_rustls::rustls::pki_types::{CertificateDer, PrivateKeyDer};
    use tokio_rustls::rustls::ServerConfig;
    use tokio_rustls::TlsAcceptor;

    use crate::error::{Error, Result};

    pub(super) fn load_acceptor(cert_path: &Path, key_path: &Path) -> Result<Arc<TlsAcceptor>> {
        let cert_chain = read_certs(cert_path)?;
        if cert_chain.is_empty() {
            return Err(Error::Invalid(format!(
                "no certificates found in {}",
                cert_path.display()
            )));
        }
        let key = read_key(key_path)?;

        let config = ServerConfig::builder()
            .with_no_client_auth()
            .with_single_cert(cert_chain, key)
            .map_err(|e| Error::Internal(format!("tls server config: {e}")))?;

        Ok(Arc::new(TlsAcceptor::from(Arc::new(config))))
    }

    fn read_certs(path: &Path) -> Result<Vec<CertificateDer<'static>>> {
        let file = File::open(path).map_err(|e| {
            Error::Internal(format!("open cert {}: {}", path.display(), e))
        })?;
        let mut reader = BufReader::new(file);
        certs(&mut reader)
            .collect::<std::result::Result<_, _>>()
            .map_err(|e| Error::Internal(format!("parse cert {}: {}", path.display(), e)))
    }

    fn read_key(path: &Path) -> Result<PrivateKeyDer<'static>> {
        // Try PKCS#8 first; fall back to RSA. We reopen the file between
        // attempts because rustls-pemfile consumes the reader.
        {
            let file = File::open(path).map_err(|e| {
                Error::Internal(format!("open key {}: {}", path.display(), e))
            })?;
            let mut reader = BufReader::new(file);
            let keys: Vec<_> = pkcs8_private_keys(&mut reader)
                .collect::<std::result::Result<_, _>>()
                .map_err(|e| Error::Internal(format!("parse pkcs8 key {}: {}", path.display(), e)))?;
            if let Some(k) = keys.into_iter().next() {
                return Ok(PrivateKeyDer::Pkcs8(k));
            }
        }
        let file = File::open(path).map_err(|e| {
            Error::Internal(format!("open key {}: {}", path.display(), e))
        })?;
        let mut reader = BufReader::new(file);
        let keys: Vec<_> = rsa_private_keys(&mut reader)
            .collect::<std::result::Result<_, _>>()
            .map_err(|e| Error::Internal(format!("parse rsa key {}: {}", path.display(), e)))?;
        let k = keys
            .into_iter()
            .next()
            .ok_or_else(|| Error::Invalid(format!("no PKCS#8 or RSA key in {}", path.display())))?;
        Ok(PrivateKeyDer::Pkcs1(k))
    }
}
