//! Shared helpers for the integration test suite.
//!
//! Each integration test boots its own `Service` against a fresh temp
//! directory so they can run in parallel without stomping on each
//! other. SMTP listeners pick from a wide ephemeral range to avoid
//! collisions between concurrent test binaries.

#![allow(dead_code)]

use std::path::PathBuf;
use std::sync::atomic::{AtomicU16, Ordering};
use std::sync::Arc;
use std::time::Duration;

use postcrate_core::{
    BindHost, CoreConfig, CreateEphemeralInput, CreateMailboxInput, EphemeralHandle, LogSink,
    MailboxKind, Service,
};
use tempfile::TempDir;
use tokio::io::{AsyncBufReadExt, AsyncReadExt, AsyncWriteExt, BufReader};
use tokio::net::TcpStream;

/// A booted service + its temp data dir. Both are kept alive for the
/// duration of the test; dropping `TempDir` cleans up the data dir.
pub struct TestService {
    pub service: Service,
    pub _data_dir: TempDir,
    pub http_url: String,
}

impl TestService {
    /// Boot with the default config but on an OS-assigned HTTP port and
    /// a wide ephemeral SMTP range so we don't collide with other test
    /// binaries running in parallel.
    pub async fn boot() -> Self {
        Self::boot_with(|_| {}).await
    }

    /// Boot, allowing the caller to mutate the `CoreConfig` first.
    pub async fn boot_with<F: FnOnce(&mut CoreConfig)>(tweak: F) -> Self {
        // We deliberately don't init tracing_subscriber here — multiple
        // tests in one binary would fight over the global default.
        // Callers that want logs can pipe RUST_LOG and set it up
        // themselves.
        let data_dir = TempDir::new().expect("create temp dir");
        let mut cfg = CoreConfig::for_data_dir(data_dir.path()).expect("config");
        cfg.http_port = 0;
        cfg.bind_host = BindHost::Loopback;
        cfg.ephemeral_port_range = next_port_range();
        // No primary mailbox by default — tests opt into one explicitly.
        cfg.default_smtp_port = 0;
        tweak(&mut cfg);

        let service = Service::build(cfg, Arc::new(LogSink)).await.expect("build");
        service.start_all().await.expect("start_all");

        let addr = wait_for(Duration::from_secs(5), || service.http_addr())
            .await
            .expect("http addr");
        let http_url = format!("http://{addr}");

        Self {
            service,
            _data_dir: data_dir,
            http_url,
        }
    }

    /// Create a fresh ephemeral mailbox; returns its handle (host + port).
    pub async fn create_ephemeral(&self, ttl_seconds: u64) -> EphemeralHandle {
        self.service
            .create_ephemeral(CreateEphemeralInput {
                project_id: "test".into(),
                name: None,
                ttl_seconds,
            })
            .await
            .expect("ephemeral")
    }

    /// Create a primary mailbox on a specific port. Caller should
    /// already have probed that the port is free.
    pub async fn create_primary(&self, port: u16) -> postcrate_core::Mailbox {
        self.service
            .create_mailbox(CreateMailboxInput {
                project_id: "test".into(),
                name: format!("primary-{port}"),
                kind: MailboxKind::Primary,
                port: Some(port),
                ttl_seconds: None,
            })
            .await
            .expect("create primary")
    }
}

/// Hand out a unique 50-port slice per test, scoped by PID so multiple
/// `cargo test` binaries running in parallel don't collide.
///
/// Layout: `30000 + (pid % 35) * 1000 + (test_idx % 1000)`. Gives ~35
/// distinct PID buckets × 20 tests/binary before wraparound, well
/// inside the practical scale of our test suite.
fn next_port_range() -> (u16, u16) {
    static OFFSET: AtomicU16 = AtomicU16::new(0);
    const SLICE: u16 = 50;
    let pid_bucket = (std::process::id() % 35) as u16 * 1000;
    let test_offset = OFFSET.fetch_add(SLICE, Ordering::Relaxed) % 1000;
    let base = 30_000u16
        .checked_add(pid_bucket)
        .and_then(|x| x.checked_add(test_offset))
        .unwrap_or(30_000);
    (base, base + SLICE - 1)
}

/// Spin until `f` returns `Some(_)` or the deadline passes.
pub async fn wait_for<T, F: Fn() -> Option<T>>(deadline: Duration, f: F) -> Option<T> {
    let start = std::time::Instant::now();
    while start.elapsed() < deadline {
        if let Some(v) = f() {
            return Some(v);
        }
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
    None
}

// ---- Minimal raw-TCP SMTP client -----------------------------------

/// A tiny RFC 5321 client wrapping a TCP stream. Lets tests drive the
/// wire directly so we can assert against specific reply codes/lines.
pub struct SmtpClient {
    pub reader: BufReader<tokio::io::ReadHalf<TcpStream>>,
    pub writer: tokio::io::WriteHalf<TcpStream>,
}

impl SmtpClient {
    pub async fn connect(host: &str, port: u16) -> std::io::Result<Self> {
        let stream = TcpStream::connect((host, port)).await?;
        let (r, w) = tokio::io::split(stream);
        Ok(Self {
            reader: BufReader::new(r),
            writer: w,
        })
    }

    /// Read the next CRLF-terminated line (trailing \r\n stripped).
    pub async fn read_line(&mut self) -> std::io::Result<String> {
        let mut line = String::new();
        let n = self.reader.read_line(&mut line).await?;
        if n == 0 {
            return Err(std::io::Error::new(
                std::io::ErrorKind::UnexpectedEof,
                "server closed",
            ));
        }
        while line.ends_with('\n') || line.ends_with('\r') {
            line.pop();
        }
        Ok(line)
    }

    /// Drain a multi-line reply: keep reading until a line whose 4th
    /// character is space (the SMTP "final line" marker).
    pub async fn read_reply(&mut self) -> std::io::Result<Vec<String>> {
        let mut lines = Vec::new();
        loop {
            let line = self.read_line().await?;
            let is_final = line.len() < 4 || line.as_bytes()[3] == b' ';
            lines.push(line);
            if is_final {
                return Ok(lines);
            }
        }
    }

    pub async fn send(&mut self, line: &str) -> std::io::Result<()> {
        self.writer.write_all(line.as_bytes()).await?;
        if !line.ends_with("\r\n") {
            self.writer.write_all(b"\r\n").await?;
        }
        self.writer.flush().await?;
        Ok(())
    }

    /// Send a full mail. Caller has already passed EHLO + MAIL FROM +
    /// RCPT TO; this drives the DATA phase end-to-end.
    pub async fn send_data(&mut self, raw_message: &[u8]) -> std::io::Result<Vec<String>> {
        self.send("DATA").await?;
        let reply = self.read_reply().await?;
        assert!(reply[0].starts_with("354"), "expected 354 got {:?}", reply);
        // Dot-stuff and append terminator.
        let stuffed = dot_stuff(raw_message);
        self.writer.write_all(&stuffed).await?;
        if !stuffed.ends_with(b"\r\n") {
            self.writer.write_all(b"\r\n").await?;
        }
        self.writer.write_all(b".\r\n").await?;
        self.writer.flush().await?;
        self.read_reply().await
    }

    pub async fn quit(mut self) -> std::io::Result<()> {
        self.send("QUIT").await?;
        let _ = self.read_reply().await;
        Ok(())
    }
}

/// Append `.` to any line in `body` that begins with `.` (RFC 5321
/// §4.5.2). Used by SmtpClient::send_data.
pub fn dot_stuff(body: &[u8]) -> Vec<u8> {
    let mut out = Vec::with_capacity(body.len());
    let mut at_line_start = true;
    for &b in body {
        if at_line_start && b == b'.' {
            out.push(b'.');
        }
        out.push(b);
        at_line_start = b == b'\n';
    }
    out
}

/// Bare-bones "send one message and disconnect" helper. Builds a tiny
/// well-formed RFC 5322 message inline; suitable for happy-path tests
/// where the exact payload doesn't matter.
pub async fn quick_send(
    host: &str,
    port: u16,
    from: &str,
    to: &str,
    subject: &str,
    body: &str,
) -> std::io::Result<()> {
    let mut c = SmtpClient::connect(host, port).await?;
    let _ = c.read_reply().await?; // banner
    c.send("EHLO test").await?;
    let _ = c.read_reply().await?;
    c.send(&format!("MAIL FROM:<{from}>")).await?;
    let _ = c.read_reply().await?;
    c.send(&format!("RCPT TO:<{to}>")).await?;
    let _ = c.read_reply().await?;
    let raw = format!(
        "From: {from}\r\nTo: {to}\r\nSubject: {subject}\r\nDate: Mon, 1 Jan 2024 00:00:00 +0000\r\n\r\n{body}"
    );
    let _ = c.send_data(raw.as_bytes()).await?;
    c.quit().await
}

/// Locate the fixtures directory relative to the crate root, regardless
/// of CWD when `cargo test` runs.
pub fn fixtures_dir() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("tests")
        .join("fixtures")
}

/// Read a fixture file as bytes.
pub fn read_fixture(rel: &str) -> Vec<u8> {
    let path = fixtures_dir().join(rel);
    std::fs::read(&path).unwrap_or_else(|e| panic!("read {}: {}", path.display(), e))
}

/// Read a fixture file as UTF-8 string.
pub fn read_fixture_str(rel: &str) -> String {
    let bytes = read_fixture(rel);
    String::from_utf8(bytes).expect("utf-8 fixture")
}

// ---- Transcript-based conformance harness --------------------------

/// Drive a `.txt` transcript fixture against a fresh SMTP session.
///
/// Format:
///   `# comment`              — ignored
///   `C: <line>`              — send `<line>\r\n`
///   `S: <prefix>`            — read one reply line, assert it starts with `<prefix>`
///   `C-DATA: <line>`         — body line during DATA (no dot-stuffing applied)
///   `C-DATA-END`             — send `.` terminator
///
/// Whitespace between the prefix and the value is significant — the
/// transcript should reflect the actual wire bytes.
pub async fn run_transcript(host: &str, port: u16, transcript: &str) {
    let mut c = SmtpClient::connect(host, port).await.expect("connect");

    for (lineno, raw) in transcript.lines().enumerate() {
        let lineno = lineno + 1;
        let line = raw.trim_end();
        if line.is_empty() || line.starts_with('#') {
            continue;
        }
        if let Some(rest) = line.strip_prefix("C: ") {
            c.send(rest)
                .await
                .unwrap_or_else(|e| panic!("line {lineno}: send failed: {e}"));
        } else if let Some(rest) = line.strip_prefix("S: ") {
            let got = c
                .read_line()
                .await
                .unwrap_or_else(|e| panic!("line {lineno}: read failed: {e}"));
            assert!(
                got.starts_with(rest),
                "line {lineno}: expected reply starting with {rest:?}, got {got:?}"
            );
        } else if let Some(rest) = line.strip_prefix("C-DATA:") {
            // Accept both `C-DATA:` (empty body line, e.g. the
            // header/body separator) and `C-DATA: text`.
            let payload = rest.strip_prefix(' ').unwrap_or(rest);
            c.writer
                .write_all(payload.as_bytes())
                .await
                .unwrap_or_else(|e| panic!("line {lineno}: write data: {e}"));
            c.writer
                .write_all(b"\r\n")
                .await
                .unwrap_or_else(|e| panic!("line {lineno}: write crlf: {e}"));
        } else if line == "C-DATA-END" {
            c.writer
                .write_all(b".\r\n")
                .await
                .unwrap_or_else(|e| panic!("line {lineno}: write terminator: {e}"));
            c.writer.flush().await.expect("flush");
        } else {
            panic!("line {lineno}: unrecognized transcript directive: {line:?}");
        }
    }

    // Drain any remaining bytes so the server-side task can complete cleanly.
    let _ = tokio::time::timeout(Duration::from_millis(100), async {
        let mut buf = [0u8; 1024];
        loop {
            match c.reader.read(&mut buf).await {
                Ok(0) | Err(_) => return,
                Ok(_) => {}
            }
        }
    })
    .await;
}
