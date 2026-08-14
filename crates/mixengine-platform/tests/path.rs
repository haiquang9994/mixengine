//! The [`PathIntegration`] contract, against the host that answers from memory.
//!
//! `.claude/architecture/platform-abstraction.md` splits this deliberately: trait-level behaviour is
//! proved against `mock`, and each real implementation proves *its own* mechanism in its own module
//! — the registry round trip inside `windows/path.rs`, the marked block inside `unix/path.rs`, each
//! against a key or a home directory the test creates and deletes. What is left for this file is the
//! part every implementation has to agree on, and the part a caller writes code against: that adding
//! twice writes once, that a read is not a mutation, and that a machine which cannot do this at all
//! says so rather than reporting a PATH it did not change.

use std::path::Path;

use mixengine_platform::Host as _;
use mixengine_platform::mock::{self, PathOp};

/// The directory every case here is about.
fn bin() -> &'static Path {
    Path::new("/opt/mixengine/bin")
}

#[test]
fn a_directory_is_off_the_path_until_it_is_put_on_it() {
    let host = mock::Host::with_home("/opt/mixengine");
    let path = host.path_integration();

    assert!(!path.state(bin()).unwrap().complete());

    let added = path.add(bin()).unwrap();
    assert!(added.complete() && added.changed());
    assert!(path.state(bin()).unwrap().complete());

    let removed = path.remove(bin()).unwrap();
    assert!(!removed.complete() && removed.changed());
    assert!(!path.state(bin()).unwrap().complete());
}

/// The distinction every client renders: "already set up" is not the same sentence as "set up just
/// now", and only one of them is true on a second call.
#[test]
fn doing_it_twice_reports_the_second_time_as_no_change() {
    let host = mock::Host::with_home("/opt/mixengine");
    let path = host.path_integration();

    path.add(bin()).unwrap();

    assert!(!path.add(bin()).unwrap().changed());
    assert!(path.remove(bin()).unwrap().changed());
    assert!(!path.remove(bin()).unwrap().changed());
}

/// A read reports and never writes, which is what lets `path.status` be the safe call a client makes
/// without being asked.
#[test]
fn asking_is_not_one_of_the_operations_the_host_records() {
    let host = mock::Host::with_home("/opt/mixengine");

    host.path_integration().state(bin()).unwrap();
    assert!(host.path_operations().is_empty());

    host.path_integration().add(bin()).unwrap();
    host.path_integration().remove(bin()).unwrap();

    assert_eq!(
        host.path_operations(),
        vec![
            PathOp::Added(bin().to_path_buf()),
            PathOp::Removed(bin().to_path_buf())
        ]
    );
}

/// The account with no home directory to write a profile into — a service account, a stripped-down
/// container. `.claude/architecture/platform-abstraction.md` rule 4: `Unsupported` is a valid
/// answer, and `reason` describes the way out.
#[test]
fn a_machine_that_cannot_do_this_says_so_rather_than_claiming_it_did() {
    let host = mock::Host::refusing_to_change_the_path(
        "/opt/mixengine",
        "this account has no home directory",
    );

    for attempt in [
        host.path_integration().state(bin()),
        host.path_integration().add(bin()),
        host.path_integration().remove(bin()),
    ] {
        let error = attempt.expect_err("a host that refuses");

        assert!(
            matches!(
                error,
                mixengine_platform::Error::UnsupportedPlatform { capability, .. }
                    if capability == "PathIntegration"
            ),
            "{error}"
        );
        assert!(error.to_string().contains("no home directory"), "{error}");
    }

    // Nothing was recorded, because nothing was attempted: the refusal is the first thing each of
    // the three does, which is what makes the recording a list of what really happened to a machine
    // rather than a list of what was asked.
    assert!(host.path_operations().is_empty());
}
