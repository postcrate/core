//! Transcript-driven RFC 5321 conformance tests.
//!
//! Each fixture under `tests/rfc5321/*.txt` is a wire transcript; see
//! `common::run_transcript` for the format. Adding a new corner case
//! means dropping a new `.txt` file into the directory and listing it
//! here.

mod common;

use std::path::PathBuf;

use common::{run_transcript, TestService};

async fn run_fixture(name: &str) {
    let ts = TestService::boot().await;
    let eph = ts.create_ephemeral(60).await;

    let path = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("tests")
        .join("rfc5321")
        .join(name);
    let transcript =
        std::fs::read_to_string(&path).unwrap_or_else(|e| panic!("read {}: {}", path.display(), e));
    run_transcript(&eph.host, eph.port, &transcript).await;
}

macro_rules! conformance {
    ($name:ident, $file:expr) => {
        #[tokio::test(flavor = "multi_thread")]
        async fn $name() {
            run_fixture($file).await;
        }
    };
}

conformance!(happy_path, "happy_path.txt");
conformance!(helo_legacy, "helo_legacy.txt");
conformance!(null_sender, "null_sender.txt");
conformance!(bad_sequence, "bad_sequence.txt");
conformance!(rset_mid_transaction, "rset_mid_transaction.txt");
conformance!(noop_help, "noop_help.txt");
conformance!(dot_stuffing, "dot_stuffing.txt");
conformance!(quoted_local_part, "quoted_local_part.txt");
