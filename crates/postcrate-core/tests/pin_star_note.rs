//! Pin / star / note + clear-preserves-pinned (FR-UX-40, FR-UX-50).

mod common;

use std::time::Duration;

use common::{quick_send, TestService};

async fn send_and_get_id(ts: &TestService, host: &str, port: u16, subject: &str) -> String {
    quick_send(host, port, "a@b", "c@d", subject, "body").await.unwrap();
    let deadline = std::time::Instant::now() + Duration::from_secs(5);
    let mailbox = ts
        .service
        .list_mailboxes(Some("test"))
        .await
        .unwrap()
        .into_iter()
        .next()
        .expect("a mailbox")
        .id;
    loop {
        let s = ts.service.list_emails(&mailbox, 100, 0).await.unwrap();
        if let Some(s0) = s.iter().find(|s| s.subject.as_deref() == Some(subject)) {
            return s0.id.clone();
        }
        if std::time::Instant::now() > deadline {
            panic!("email {subject:?} never landed");
        }
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
}

#[tokio::test(flavor = "multi_thread")]
async fn pin_persists_in_detail_and_summary() {
    let ts = TestService::boot().await;
    let eph = ts.create_ephemeral(60).await;
    let id = send_and_get_id(&ts, &eph.host, eph.port, "to be pinned").await;

    ts.service.set_pinned(&id, true).await.unwrap();
    let d = ts.service.get_email(&id).await.unwrap();
    assert!(d.pinned);
    let listed = ts.service.list_emails(&eph.id, 100, 0).await.unwrap();
    assert!(listed.iter().any(|s| s.id == id && s.pinned));
}

#[tokio::test(flavor = "multi_thread")]
async fn star_persists() {
    let ts = TestService::boot().await;
    let eph = ts.create_ephemeral(60).await;
    let id = send_and_get_id(&ts, &eph.host, eph.port, "noteworthy").await;
    ts.service.set_starred(&id, true).await.unwrap();
    let d = ts.service.get_email(&id).await.unwrap();
    assert!(d.starred);
    ts.service.set_starred(&id, false).await.unwrap();
    let d = ts.service.get_email(&id).await.unwrap();
    assert!(!d.starred);
}

#[tokio::test(flavor = "multi_thread")]
async fn note_round_trips() {
    let ts = TestService::boot().await;
    let eph = ts.create_ephemeral(60).await;
    let id = send_and_get_id(&ts, &eph.host, eph.port, "annotated").await;

    ts.service.set_note(&id, Some("Check this with QA")).await.unwrap();
    let d = ts.service.get_email(&id).await.unwrap();
    assert_eq!(d.note.as_deref(), Some("Check this with QA"));

    ts.service.set_note(&id, None).await.unwrap();
    let d = ts.service.get_email(&id).await.unwrap();
    assert!(d.note.is_none());
}

#[tokio::test(flavor = "multi_thread")]
async fn clear_preserves_pinned_purge_does_not() {
    let ts = TestService::boot().await;
    let eph = ts.create_ephemeral(60).await;
    let pinned_id = send_and_get_id(&ts, &eph.host, eph.port, "important").await;
    let regular_id = send_and_get_id(&ts, &eph.host, eph.port, "regular").await;
    ts.service.set_pinned(&pinned_id, true).await.unwrap();

    let cleared = ts.service.clear_mailbox(&eph.id).await.unwrap();
    assert_eq!(cleared, 1, "clear should only delete the regular email");

    let listed = ts.service.list_emails(&eph.id, 100, 0).await.unwrap();
    assert_eq!(listed.len(), 1);
    assert_eq!(listed[0].id, pinned_id);

    // Regular email should be gone.
    assert!(ts.service.get_email(&regular_id).await.is_err());

    // Purge takes everything.
    let purged = ts.service.purge_mailbox(&eph.id).await.unwrap();
    assert_eq!(purged, 1);
    assert!(ts.service.list_emails(&eph.id, 100, 0).await.unwrap().is_empty());
}

#[tokio::test(flavor = "multi_thread")]
async fn pinned_sort_first_in_list() {
    let ts = TestService::boot().await;
    let eph = ts.create_ephemeral(60).await;
    let _old_id = send_and_get_id(&ts, &eph.host, eph.port, "old").await;
    let new_id = send_and_get_id(&ts, &eph.host, eph.port, "newer").await;
    let oldest_id = send_and_get_id(&ts, &eph.host, eph.port, "oldest").await;
    ts.service.set_pinned(&oldest_id, true).await.unwrap();

    let listed = ts.service.list_emails(&eph.id, 100, 0).await.unwrap();
    assert_eq!(listed[0].id, oldest_id, "pinned should sort first regardless of received_at");
    // The newer non-pinned should come before older non-pinned in the rest.
    let new_pos = listed.iter().position(|s| s.id == new_id).unwrap();
    let old_pos = listed.iter().position(|s| s.subject.as_deref() == Some("old")).unwrap();
    assert!(new_pos < old_pos);
}
