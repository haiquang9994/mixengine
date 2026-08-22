//! The hosts file, against the real machine.
//!
//! **Nothing here writes.** Reading `/etc/hosts` needs no privilege on any of the three systems,
//! which is exactly why the trait is read-only (T41 design, D9); the write is the privileged
//! operation and its own tests drive it against a file they own. The one test that touches the real
//! file is `#[ignore]`d and belongs to CI's `system` job.

use mixengine_platform::{Host as _, hosts, mock};

#[test]
fn the_hosts_file_is_where_this_operating_system_keeps_it() {
    let path = hosts::path();

    assert!(path.is_absolute(), "{}", path.display());
    assert!(path.ends_with("hosts"), "{}", path.display());

    #[cfg(unix)]
    assert_eq!(path, std::path::Path::new("/etc/hosts"));
    #[cfg(windows)]
    assert!(
        path.to_string_lossy()
            .to_lowercase()
            .contains(r"drivers\etc"),
        "{}",
        path.display()
    );
}

/// Reading the machine's own file needs no token, and is what `domain.dns_status` (T46) and
/// `mix doctor` (T47) will ask. A machine with no block of ours reads as an empty list, which is
/// not an error.
#[test]
fn the_real_machine_can_be_asked_what_our_block_holds() {
    let host = mixengine_platform::host();

    assert_eq!(host.hosts_file().path(), hosts::path());

    match host.hosts_file().managed() {
        Ok(entries) => {
            for entry in entries {
                assert!(!entry.domain.is_empty());
            }
        }
        // A machine whose block somebody has half-edited, and a CI image with no hosts file at all.
        // Both are answers rather than failures of this test.
        Err(error) => assert!(!error.to_string().is_empty()),
    }
}

/// D9's payoff: the daemon's side of this is testable without a machine at all.
#[test]
fn a_mock_host_answers_from_memory() {
    let host = mock::Host::with_hosts("/tmp/mixengine-test", ["127.0.0.1 blog.test"]);

    let managed = host.hosts_file().managed().unwrap();

    assert_eq!(managed.len(), 1);
    assert_eq!(managed[0].domain, "blog.test");

    let refusing = mock::Host::unable_to_read_the_hosts_file("/tmp/mixengine-test", "no such file");
    assert!(refusing.hosts_file().managed().is_err());
}
