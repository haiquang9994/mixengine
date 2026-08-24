//! Writing a file nobody else may read.
//!
//! **The Unix half runs under `umask(0o000)` on purpose.** With a permissive umask a plain
//! `fs::write` produces `0666`, so asserting `0600` proves the mode came from this crate. Without
//! setting the umask the same assertion passes on any machine whose umask happens to be `0o077`,
//! and would keep passing if somebody replaced the implementation with `fs::write` — a green test
//! measuring the machine it ran on rather than the code.
//!
//! The control file beside it is what turns that from a claim into a check: it is written with
//! `fs::write` under the same umask, and the test refuses to draw a conclusion unless the control
//! came out world-writable.

use std::path::Path;

/// Anything readable, so that a passing test is about permissions and not about an empty file.
const CONTENT: &[u8] = b"-----BEGIN PRIVATE KEY-----\nnot really\n-----END PRIVATE KEY-----\n";

#[test]
fn a_private_file_carries_its_content_back() {
    let home = tempfile::tempdir().expect("a temporary directory");
    let path = home.path().join("root.key");

    mixengine_platform::write_private(&path, CONTENT).expect("the file was written");

    assert_eq!(
        std::fs::read(&path).expect("the file is there"),
        CONTENT,
        "a file whose permissions are right and whose bytes are wrong is not a success"
    );

    assert_private(&path);
}

/// Overwriting is supported, and the second write is as private as the first.
///
/// The case this exists for is a home written by an older version, or restored from a backup that
/// did not carry permissions: the key is replaced, and it must not be readable on the way.
#[test]
fn writing_over_a_world_readable_file_leaves_it_private() {
    let home = tempfile::tempdir().expect("a temporary directory");
    let path = home.path().join("root.key");

    std::fs::write(&path, b"stale, and longer than what replaces it").expect("the decoy is there");
    widen(&path);

    mixengine_platform::write_private(&path, CONTENT).expect("the file was written");

    assert_eq!(
        std::fs::read(&path).expect("the file is there"),
        CONTENT,
        "the old content was not fully replaced"
    );
    assert_private(&path);
}

#[cfg(unix)]
#[test]
#[expect(
    unsafe_code,
    reason = "`umask` is the only way to set this process's file-creation mask, and setting it is \
              the whole of what makes this test able to fail"
)]
fn a_permissive_umask_does_not_reach_a_private_file() {
    use std::os::unix::fs::PermissionsExt as _;

    let home = tempfile::tempdir().expect("a temporary directory");
    let path = home.path().join("root.key");
    let control = home.path().join("control");

    // SAFETY: `umask` is process-wide and cannot fail. It is restored below before anything can
    // return, and the control file is written inside the same window so the comparison is between
    // two writes under one umask rather than between two moments.
    let previous = unsafe { libc::umask(0o000) };

    std::fs::write(&control, CONTENT).expect("the control is there");
    let outcome = mixengine_platform::write_private(&path, CONTENT);

    // SAFETY: as above.
    unsafe { libc::umask(previous) };

    outcome.expect("the file was written");

    let control_mode = std::fs::metadata(&control)
        .expect("the control is there")
        .permissions()
        .mode()
        & 0o777;

    assert_eq!(
        control_mode, 0o666,
        "the umask did not take effect, so this test cannot tell a private write from a plain one \
         and is proving nothing"
    );

    assert_private(&path);
}

/// Make `path` readable and writable by everyone, whatever this OS means by that.
fn widen(path: &Path) {
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt as _;
        std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o666))
            .expect("the decoy was widened");
    }

    // On Windows the decoy already carries what it inherited from the temporary directory, which is
    // the state this test wants: an ACL that came from somewhere else and has to be replaced.
    #[cfg(windows)]
    let _ = path;
}

/// What "private" means, judged the way this OS expresses it.
fn assert_private(path: &Path) {
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt as _;

        let mode = std::fs::metadata(path)
            .expect("the file is there")
            .permissions()
            .mode()
            & 0o777;

        assert_eq!(mode, 0o600, "the file is readable by somebody else");
    }

    #[cfg(windows)]
    {
        // The same reading `DirectoryAccess::is_restricted_to_owner` makes: no ACE arrived by
        // inheritance, and there are exactly the three this crate grants. Asked through the crate
        // rather than parsed here, so the test cannot drift from what the code applies.
        assert!(
            mixengine_platform::is_private_file(path).expect("the file is there"),
            "the file still carries ACEs it inherited"
        );
    }
}
