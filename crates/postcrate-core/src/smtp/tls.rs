//! TLS scaffolding placeholder.
//!
//! Today this module only exists so the session loop can be generic over
//! `Io: AsyncRead + AsyncWrite + Unpin`. When the TLS phase lands, we'll:
//!
//! 1. Add `tokio-rustls` + `rustls-pemfile` to `Cargo.toml` behind the
//!    existing `tls` feature flag.
//! 2. Implement an `upgrade_to_tls(stream, cert, key) -> impl AsyncRead +
//!    AsyncWrite + Unpin` helper here.
//! 3. Flip `EhloAdvert::starttls_enabled = true` and add a STARTTLS
//!    handler in `session.rs` that calls `upgrade_to_tls` before the
//!    next `EHLO` round.
//!
//! Nothing else in the engine has to change.

#[derive(Debug, Clone, Default)]
pub struct TlsConfig {
    pub enabled: bool,
    pub cert_path: Option<std::path::PathBuf>,
    pub key_path: Option<std::path::PathBuf>,
}
