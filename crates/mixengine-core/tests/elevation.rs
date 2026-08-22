//! The seam nothing else touches: the document this crate writes is one the shipped helper accepts,
//! and the report the shipped helper writes is one this crate can read.
//!
//! **No token and no prompt.** `mixengine-elevate` is run directly, as this user, which is possible
//! only because T40/D5 made `Probe` the operation that does not require elevation — the operation
//! whose job includes reporting whether the token is elevated has to work when it is not. What is
//! under test is the file protocol across the two crates, not the launcher: raising a prompt is
//! `mixengine-platform`'s and is asserted in its own suite.
//!
//! Every test gets its own home in a `TempDir`, passed as a path — rule 2 in
//! `.claude/standards/testing.md`.

use std::path::PathBuf;
use std::process::Command;

use mixengine_core::elevation;
use mixengine_proto::privileged::{OpOutcome, PrivilegedOp};
use mixengine_proto::{PendingOp, PendingOpId, Timestamp};

/// The `mixengine-elevate` built alongside this test.
///
/// `CARGO_BIN_EXE_…` reaches only binaries of the package the test is in, and the helper is another
/// one — so it is found next to the test binary, which is where a workspace build puts both. The
/// same mechanism `crates/mixengine-platform/tests/elevation.rs` uses.
fn helper() -> PathBuf {
    let name = format!("mixengine-elevate{}", std::env::consts::EXE_SUFFIX);
    let test = std::env::current_exe().expect("this test binary has a path");
    let directory = test.parent().expect("this test binary is in a directory");

    let beside = directory.join(&name);
    if beside.is_file() {
        return beside;
    }

    let above = directory
        .parent()
        .expect("the deps directory is inside the profile directory")
        .join(&name);

    assert!(
        above.is_file(),
        "{} is not there — this suite runs the real helper, so build it first: \
         `cargo build -p mixengine-elevate`",
        above.display()
    );

    above
}

fn waiting(id: i64) -> PendingOp {
    let op = PrivilegedOp::Probe {};

    PendingOp {
        id: PendingOpId(id),
        description: op.describe(),
        op,
        requested_at: Timestamp(1_760_000_000_000),
    }
}

/// The whole round trip, in the directory layout the daemon uses.
///
/// `run/elevate/<id>/` and not somewhere arbitrary, because the helper takes a lock at
/// `<home>/run/` before it applies anything — a request written outside that tree would fail for a
/// reason that has nothing to do with the protocol.
#[test]
fn the_document_this_crate_writes_is_one_the_shipped_helper_answers() {
    let home = tempfile::tempdir().expect("a temporary home");
    let directory = home.path().join("run").join("elevate").join("t40b");

    let request = elevation::write_request(&directory, home.path(), &[waiting(1), waiting(2)])
        .expect("the request is written");

    let status = Command::new(helper())
        .arg(request.path())
        .output()
        .expect("the helper binary runs");

    assert!(
        status.status.success(),
        "the helper refused the whole request: {}",
        String::from_utf8_lossy(&status.stderr)
    );

    let report = elevation::read_report(&request).expect("a report beside the request");

    assert_eq!(report.nonce, request.nonce());
    assert_eq!(report.results.len(), 2, "one outcome per operation");
    assert!(
        report.supported_ops.iter().any(|name| name == "probe"),
        "{:?}",
        report.supported_ops
    );

    // `Probe` applies under any token — D5's payoff — so this is the same answer elevated or not.
    for outcome in &report.results {
        assert!(matches!(outcome, OpOutcome::Applied { .. }), "{outcome:?}");
    }

    // And what this leg is uniquely able to say: an unelevated helper reports itself honestly.
    // On CI's Windows third the daemon suites run under a full token, so this is asserted as a
    // property of the *helper's own* process rather than as a constant.
    assert_eq!(
        report.elevated,
        mixengine_platform::elevated::is_elevated(),
        "the helper inherits this process's token, and says which one it got"
    );
}

/// T40/D10, from the daemon's side: the existence of the answer is the anti-replay check, so a
/// directory that has one is finished. The daemon never reuses one — this is what would happen if a
/// later change made it.
#[test]
fn a_request_that_has_already_been_answered_is_refused() {
    let home = tempfile::tempdir().expect("a temporary home");
    let directory = home.path().join("run").join("elevate").join("t40b");

    let request = elevation::write_request(&directory, home.path(), &[waiting(1)])
        .expect("the request is written");

    let first = Command::new(helper())
        .arg(request.path())
        .output()
        .expect("the helper binary runs");
    assert!(first.status.success());

    let second = Command::new(helper())
        .arg(request.path())
        .output()
        .expect("the helper binary runs");

    assert!(
        !second.status.success(),
        "a request with an answer beside it has been processed"
    );
    assert!(
        String::from_utf8_lossy(&second.stderr).contains("already been answered"),
        "{}",
        String::from_utf8_lossy(&second.stderr)
    );
}
