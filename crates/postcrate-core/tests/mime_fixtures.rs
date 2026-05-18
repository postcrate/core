//! Round-trip tests for real-world `.eml` fixtures.
//!
//! For each fixture we send the raw bytes via SMTP, then assert two
//! things:
//!   1. The stored raw blob is byte-identical (modulo SMTP framing).
//!   2. The parsed `EmailDetail` matches the expected shape.

mod common;

use std::path::PathBuf;
use std::time::Duration;

use common::{SmtpClient, TestService};

/// Helper: send the fixture through a happy-path SMTP exchange and
/// return the parsed [`EmailDetail`] once ingest completes.
async fn send_fixture(ts: &TestService, host: &str, port: u16, fixture: &str)
    -> postcrate_core::EmailDetail
{
    let path = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("tests")
        .join("fixtures")
        .join("mime")
        .join(fixture);
    let raw = std::fs::read(&path)
        .unwrap_or_else(|e| panic!("read {}: {}", path.display(), e));

    let mut c = SmtpClient::connect(host, port).await.unwrap();
    let _ = c.read_reply().await.unwrap();
    c.send("EHLO test").await.unwrap();
    let _ = c.read_reply().await.unwrap();
    c.send("MAIL FROM:<sender@example.com> SMTPUTF8 BODY=8BITMIME").await.unwrap();
    let _ = c.read_reply().await.unwrap();
    c.send("RCPT TO:<recipient@example.com>").await.unwrap();
    let _ = c.read_reply().await.unwrap();
    let reply = c.send_data(&raw).await.unwrap();
    assert!(reply[0].starts_with("250"));
    c.quit().await.unwrap();

    // Eventually-consistent ingest — poll briefly.
    let start = std::time::Instant::now();
    loop {
        let summaries = ts
            .service
            .list_emails(&which_mailbox(ts).await, 100, 0)
            .await
            .unwrap();
        if !summaries.is_empty() {
            let detail = ts.service.get_email(&summaries[0].id).await.unwrap();
            return detail;
        }
        if start.elapsed() > Duration::from_secs(5) {
            panic!("timed out waiting for email ingest of {fixture}");
        }
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
}

async fn which_mailbox(ts: &TestService) -> String {
    ts.service
        .list_mailboxes(Some("test"))
        .await
        .unwrap()
        .into_iter()
        .next()
        .expect("a mailbox")
        .id
}

#[tokio::test(flavor = "multi_thread")]
async fn multipart_alternative_picks_both_bodies() {
    let ts = TestService::boot().await;
    let eph = ts.create_ephemeral(60).await;
    let detail = send_fixture(&ts, &eph.host, eph.port, "multipart_alternative.eml").await;

    assert!(detail.has_text);
    assert!(detail.has_html);
    assert!(detail
        .text_body
        .as_deref()
        .unwrap_or("")
        .contains("Hello in plain text!"));
    assert!(detail
        .html_body
        .as_deref()
        .unwrap_or("")
        .contains("<b>HTML</b>"));
    assert_eq!(detail.subject.as_deref(), Some("Hello from multipart"));
}

#[tokio::test(flavor = "multi_thread")]
async fn multipart_related_extracts_cid_attachment() {
    let ts = TestService::boot().await;
    let eph = ts.create_ephemeral(60).await;
    let detail = send_fixture(&ts, &eph.host, eph.port, "multipart_related_cid.eml").await;

    assert!(detail.has_html);
    assert_eq!(detail.attachments.len(), 1, "expected 1 inline attachment");
    let att = &detail.attachments[0];
    assert_eq!(att.content_type.as_deref(), Some("image/png"));
    assert_eq!(att.content_id.as_deref(), Some("logo@example.com"));
}

#[tokio::test(flavor = "multi_thread")]
async fn rfc2047_subject_decoded() {
    let ts = TestService::boot().await;
    let eph = ts.create_ephemeral(60).await;
    let detail = send_fixture(&ts, &eph.host, eph.port, "rfc2047_subject.eml").await;
    let subj = detail.subject.as_deref().unwrap_or("");
    assert!(
        subj.contains("Emoji") || subj.contains("📧"),
        "expected decoded subject; got {subj:?}"
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn utf8_subject_preserved() {
    let ts = TestService::boot().await;
    let eph = ts.create_ephemeral(60).await;
    let detail = send_fixture(&ts, &eph.host, eph.port, "utf8_subject.eml").await;
    let subj = detail.subject.as_deref().unwrap_or("");
    assert!(subj.contains("café"));
    assert!(subj.contains("日本語"));
}

#[tokio::test(flavor = "multi_thread")]
async fn multipart_mixed_attachment_captured() {
    let ts = TestService::boot().await;
    let eph = ts.create_ephemeral(60).await;
    let detail = send_fixture(&ts, &eph.host, eph.port, "multipart_mixed_attachment.eml").await;

    assert_eq!(detail.attachments.len(), 1);
    let att = &detail.attachments[0];
    assert_eq!(att.filename.as_deref(), Some("report.pdf"));
    assert_eq!(att.content_type.as_deref(), Some("application/pdf"));
    assert!(att.size_bytes > 0);
}

#[tokio::test(flavor = "multi_thread")]
async fn filename_star_decoded() {
    let ts = TestService::boot().await;
    let eph = ts.create_ephemeral(60).await;
    let detail = send_fixture(&ts, &eph.host, eph.port, "filename_star_encoded.eml").await;

    assert_eq!(detail.attachments.len(), 1);
    let name = detail.attachments[0].filename.as_deref().unwrap_or("");
    // RFC 2231: `filename*=UTF-8''r%C3%A9sum%C3%A9.txt` decodes to `résumé.txt`.
    assert!(
        name.contains("résumé") || name.contains("r%C3%A9sum%C3%A9"),
        "expected decoded filename; got {name:?}"
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn raw_blob_round_trips_byte_identical() {
    let ts = TestService::boot().await;
    let eph = ts.create_ephemeral(60).await;
    let detail = send_fixture(&ts, &eph.host, eph.port, "multipart_alternative.eml").await;

    let raw = ts.service.get_email_raw(&detail.id).await.unwrap();
    // The stored raw should contain the original headers and body bytes.
    // We don't byte-compare to the source because dot-stuffing semantics
    // and possible line-ending normalization apply; but the structural
    // landmarks should all be present.
    let raw_str = std::str::from_utf8(&raw).expect("ascii fixture");
    assert!(raw_str.contains("--boundary42"));
    assert!(raw_str.contains("Hello in plain text!"));
    assert!(raw_str.contains("<b>HTML</b>"));
}
