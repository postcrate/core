//! STARTTLS handshake smoke test (T3.2).
//!
//! Only meaningful when the `tls` feature is on; otherwise the engine
//! deliberately doesn't advertise STARTTLS and this binary's tests
//! are empty.

#![cfg(feature = "tls")]

mod common;

use std::sync::Arc;

use common::TestService;
use rcgen::generate_simple_self_signed;
use tempfile::TempDir;
use tokio::io::AsyncWriteExt;
use tokio::net::TcpStream;
use tokio_rustls::rustls::pki_types::ServerName;

fn write_self_signed(dir: &TempDir) -> (std::path::PathBuf, std::path::PathBuf) {
    let cert = generate_simple_self_signed(vec!["localhost".into()]).unwrap();
    let cert_path = dir.path().join("cert.pem");
    let key_path = dir.path().join("key.pem");
    std::fs::write(&cert_path, cert.cert.pem()).unwrap();
    std::fs::write(&key_path, cert.key_pair.serialize_pem()).unwrap();
    (cert_path, key_path)
}

#[tokio::test(flavor = "multi_thread")]
async fn ehlo_advertises_starttls_when_tls_configured() {
    let dir = TempDir::new().unwrap();
    let (cert_path, key_path) = write_self_signed(&dir);

    let ts = TestService::boot_with(|cfg| {
        cfg.tls.enabled = true;
        cfg.tls.cert_path = Some(cert_path.clone());
        cfg.tls.key_path = Some(key_path.clone());
    })
    .await;
    let eph = ts.create_ephemeral(60).await;

    let mut c = common::SmtpClient::connect(&eph.host, eph.port).await.unwrap();
    let _ = c.read_reply().await.unwrap();
    c.send("EHLO test").await.unwrap();
    let reply = c.read_reply().await.unwrap();
    let joined = reply.join("\n");
    assert!(joined.contains("STARTTLS"), "STARTTLS not advertised; got {joined}");
}

#[tokio::test(flavor = "multi_thread")]
async fn full_starttls_handshake_and_post_tls_ehlo() {
    use tokio_rustls::TlsConnector;

    let dir = TempDir::new().unwrap();
    let (cert_path, key_path) = write_self_signed(&dir);

    let ts = TestService::boot_with(|cfg| {
        cfg.tls.enabled = true;
        cfg.tls.cert_path = Some(cert_path);
        cfg.tls.key_path = Some(key_path);
    })
    .await;
    let eph = ts.create_ephemeral(60).await;

    // Raw TCP: greet, EHLO, STARTTLS.
    let sock = TcpStream::connect((eph.host.as_str(), eph.port)).await.unwrap();
    let mut sock = tokio::io::BufStream::new(sock);

    // Banner.
    let banner = read_smtp_reply(&mut sock).await;
    assert!(banner.starts_with("220"), "got {banner:?}");
    // EHLO.
    sock.write_all(b"EHLO test\r\n").await.unwrap();
    sock.flush().await.unwrap();
    let ehlo = read_smtp_reply(&mut sock).await;
    assert!(ehlo.contains("STARTTLS"), "STARTTLS not in EHLO: {ehlo}");
    // STARTTLS.
    sock.write_all(b"STARTTLS\r\n").await.unwrap();
    sock.flush().await.unwrap();
    let tls_ready = read_smtp_reply(&mut sock).await;
    assert!(tls_ready.starts_with("220"), "got {tls_ready:?}");

    // Unwrap the BufStream so the inner TcpStream can be handed to rustls.
    let sock = sock.into_inner();

    // Wrap with rustls — trust-all because the cert is self-signed.
    let connector = TlsConnector::from(Arc::new(trust_all_client_config()));
    let server_name = ServerName::try_from("localhost").unwrap();
    let tls = connector.connect(server_name, sock).await.expect("TLS handshake");
    let mut tls = tokio::io::BufStream::new(tls);

    // The server greets again on the new (TLS) session — read and
    // discard the 220.
    let post_banner = read_smtp_reply(&mut tls).await;
    assert!(post_banner.starts_with("220"), "got {post_banner:?}");

    // Send EHLO inside TLS — the server should *not* advertise
    // STARTTLS (RFC 3207 §4).
    tls.write_all(b"EHLO secure\r\n").await.unwrap();
    tls.flush().await.unwrap();
    let reply = read_smtp_reply(&mut tls).await;
    assert!(reply.starts_with("250"), "got {reply:?}");
    assert!(!reply.contains("STARTTLS"), "STARTTLS still advertised inside TLS: {reply}");
}

/// Read one full SMTP reply (single-line or multi-line) from a buffered
/// stream. SMTP marks the last line of a multi-line reply by putting a
/// space at column 4 instead of `-`.
async fn read_smtp_reply<S>(s: &mut tokio::io::BufStream<S>) -> String
where
    S: tokio::io::AsyncRead + tokio::io::AsyncWrite + Unpin,
{
    use tokio::io::AsyncBufReadExt;
    let mut out = String::new();
    loop {
        let mut line = String::new();
        s.read_line(&mut line).await.unwrap();
        let is_final = line.len() < 4 || line.as_bytes()[3] == b' ';
        out.push_str(&line);
        if is_final {
            return out;
        }
    }
}

#[tokio::test(flavor = "multi_thread")]
async fn implicit_tls_listener_wraps_immediately() {
    use std::sync::Arc;
    use tokio::io::{AsyncReadExt, AsyncWriteExt};
    use tokio_rustls::TlsConnector;

    let dir = TempDir::new().unwrap();
    let (cert_path, key_path) = write_self_signed(&dir);

    let ts = TestService::boot_with(|cfg| {
        cfg.tls.enabled = true;
        cfg.tls.cert_path = Some(cert_path);
        cfg.tls.key_path = Some(key_path);
    })
    .await;

    // Pick a free port and create a mailbox with implicit_tls = true.
    let port = {
        let l = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let p = l.local_addr().unwrap().port();
        drop(l);
        p
    };
    let mb = ts
        .service
        .create_mailbox(postcrate_core::CreateMailboxInput {
            project_id: "test".into(),
            name: format!("implicit-tls-{port}"),
            kind: postcrate_core::MailboxKind::Primary,
            port: Some(port),
            ttl_seconds: None,
            implicit_tls: true,
        })
        .await
        .expect("create mailbox");
    let addr = ts.service.mailbox_addr(&mb.id).expect("listener addr");

    // Connect raw TCP and wrap with rustls — the server expects a TLS
    // ClientHello as the very first thing on the wire (no plaintext
    // banner).
    let sock = tokio::net::TcpStream::connect(addr).await.unwrap();
    let connector = TlsConnector::from(Arc::new(trust_all_client_config()));
    let server_name = tokio_rustls::rustls::pki_types::ServerName::try_from("localhost").unwrap();
    let mut tls = connector.connect(server_name, sock).await.expect("TLS handshake");

    // First bytes inside TLS should be the 220 banner.
    let mut buf = vec![0u8; 256];
    let n = tls.read(&mut buf).await.unwrap();
    let banner = std::str::from_utf8(&buf[..n]).unwrap();
    assert!(banner.starts_with("220"), "got {banner:?}");

    // EHLO inside TLS — STARTTLS must NOT be advertised.
    tls.write_all(b"EHLO test\r\n").await.unwrap();
    let mut buf = vec![0u8; 4096];
    // Drain until we see a final `250 ` line.
    let mut acc = String::new();
    loop {
        let n = tls.read(&mut buf).await.unwrap();
        acc.push_str(std::str::from_utf8(&buf[..n]).unwrap());
        if acc.lines().any(|l| l.len() >= 4 && &l[..4] == "250 ") {
            break;
        }
    }
    assert!(!acc.contains("STARTTLS"), "STARTTLS leaked: {acc}");
    assert!(acc.contains("AUTH PLAIN LOGIN"));
}

fn trust_all_client_config() -> tokio_rustls::rustls::ClientConfig {
    use tokio_rustls::rustls::client::danger::{ServerCertVerified, ServerCertVerifier};
    use tokio_rustls::rustls::pki_types::{CertificateDer, ServerName, UnixTime};
    use tokio_rustls::rustls::{ClientConfig, DigitallySignedStruct, Error, SignatureScheme};

    #[derive(Debug)]
    struct TrustAll;
    impl ServerCertVerifier for TrustAll {
        fn verify_server_cert(
            &self,
            _end_entity: &CertificateDer<'_>,
            _intermediates: &[CertificateDer<'_>],
            _server_name: &ServerName<'_>,
            _ocsp_response: &[u8],
            _now: UnixTime,
        ) -> std::result::Result<ServerCertVerified, Error> {
            Ok(ServerCertVerified::assertion())
        }
        fn verify_tls12_signature(
            &self,
            _message: &[u8],
            _cert: &CertificateDer<'_>,
            _dss: &DigitallySignedStruct,
        ) -> std::result::Result<tokio_rustls::rustls::client::danger::HandshakeSignatureValid, Error>
        {
            Ok(tokio_rustls::rustls::client::danger::HandshakeSignatureValid::assertion())
        }
        fn verify_tls13_signature(
            &self,
            _message: &[u8],
            _cert: &CertificateDer<'_>,
            _dss: &DigitallySignedStruct,
        ) -> std::result::Result<tokio_rustls::rustls::client::danger::HandshakeSignatureValid, Error>
        {
            Ok(tokio_rustls::rustls::client::danger::HandshakeSignatureValid::assertion())
        }
        fn supported_verify_schemes(&self) -> Vec<SignatureScheme> {
            vec![
                SignatureScheme::RSA_PKCS1_SHA256,
                SignatureScheme::RSA_PKCS1_SHA384,
                SignatureScheme::RSA_PKCS1_SHA512,
                SignatureScheme::ECDSA_NISTP256_SHA256,
                SignatureScheme::ECDSA_NISTP384_SHA384,
                SignatureScheme::ED25519,
            ]
        }
    }

    ClientConfig::builder()
        .dangerous()
        .with_custom_certificate_verifier(Arc::new(TrustAll))
        .with_no_client_auth()
}
