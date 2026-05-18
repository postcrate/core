//! SMTP AUTH PLAIN / LOGIN compatibility (T3.1).
//!
//! We accept *any* credentials — AUTH is for client compatibility
//! (some libraries refuse to send unless AUTH is advertised), not for
//! security on a local capture server.

mod common;

use common::TestService;

#[tokio::test(flavor = "multi_thread")]
async fn ehlo_advertises_auth() {
    let ts = TestService::boot().await;
    let eph = ts.create_ephemeral(60).await;

    let mut c = common::SmtpClient::connect(&eph.host, eph.port).await.unwrap();
    let _ = c.read_reply().await.unwrap();
    c.send("EHLO test").await.unwrap();
    let reply = c.read_reply().await.unwrap();
    let joined = reply.join("\n");
    assert!(
        joined.contains("AUTH PLAIN LOGIN"),
        "AUTH not advertised; got {joined}"
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn auth_plain_with_initial_response_accepted() {
    let ts = TestService::boot().await;
    let eph = ts.create_ephemeral(60).await;
    let mut c = common::SmtpClient::connect(&eph.host, eph.port).await.unwrap();
    let _ = c.read_reply().await.unwrap();
    c.send("EHLO test").await.unwrap();
    let _ = c.read_reply().await.unwrap();
    // `AUTH PLAIN AGFsaWNlAHBhc3M=` is base64 of `\0alice\0pass`.
    c.send("AUTH PLAIN AGFsaWNlAHBhc3M=").await.unwrap();
    let reply = c.read_reply().await.unwrap();
    assert!(reply[0].starts_with("235"), "got {reply:?}");
}

#[tokio::test(flavor = "multi_thread")]
async fn auth_plain_no_initial_response() {
    let ts = TestService::boot().await;
    let eph = ts.create_ephemeral(60).await;
    let mut c = common::SmtpClient::connect(&eph.host, eph.port).await.unwrap();
    let _ = c.read_reply().await.unwrap();
    c.send("EHLO test").await.unwrap();
    let _ = c.read_reply().await.unwrap();
    c.send("AUTH PLAIN").await.unwrap();
    let r = c.read_reply().await.unwrap();
    assert!(r[0].starts_with("334"));
    // Send a (fake) base64 blob; the server accepts anything.
    c.send("AGFsaWNlAHBhc3M=").await.unwrap();
    let r = c.read_reply().await.unwrap();
    assert!(r[0].starts_with("235"), "got {r:?}");
}

#[tokio::test(flavor = "multi_thread")]
async fn auth_login_two_step() {
    let ts = TestService::boot().await;
    let eph = ts.create_ephemeral(60).await;
    let mut c = common::SmtpClient::connect(&eph.host, eph.port).await.unwrap();
    let _ = c.read_reply().await.unwrap();
    c.send("EHLO test").await.unwrap();
    let _ = c.read_reply().await.unwrap();
    c.send("AUTH LOGIN").await.unwrap();
    let r = c.read_reply().await.unwrap();
    assert!(r[0].starts_with("334"));
    // username
    c.send("YWxpY2U=").await.unwrap();
    let r = c.read_reply().await.unwrap();
    assert!(r[0].starts_with("334"));
    // password
    c.send("cGFzc3dvcmQ=").await.unwrap();
    let r = c.read_reply().await.unwrap();
    assert!(r[0].starts_with("235"), "got {r:?}");
}

#[tokio::test(flavor = "multi_thread")]
async fn auth_unknown_mechanism_504() {
    let ts = TestService::boot().await;
    let eph = ts.create_ephemeral(60).await;
    let mut c = common::SmtpClient::connect(&eph.host, eph.port).await.unwrap();
    let _ = c.read_reply().await.unwrap();
    c.send("EHLO test").await.unwrap();
    let _ = c.read_reply().await.unwrap();
    c.send("AUTH CRAM-MD5").await.unwrap();
    let r = c.read_reply().await.unwrap();
    assert!(r[0].starts_with("504"), "got {r:?}");
}
