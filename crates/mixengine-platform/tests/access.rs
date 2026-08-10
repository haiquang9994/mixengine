//! Owner-only directories, against the real OS.
//!
//! These are not `#[ignore]`d: they touch a `TempDir` this test owns and nothing else. Nothing
//! here is a system test — no hosts file, no trust store, no privileged port.

use std::path::Path;

use mixengine_platform::{Host as _, host, mock};
use tempfile::TempDir;

/// What `MIXENGINE_HOME` looks like before anything is done to it: created by this process, with
/// whatever the umask (Unix) or the parent's ACL (Windows) decided.
fn fresh_directory() -> TempDir {
    TempDir::new().expect("the system temporary directory is writable")
}

#[cfg(unix)]
fn mode_of(path: &Path) -> u32 {
    use std::os::unix::fs::PermissionsExt as _;

    std::fs::metadata(path)
        .expect("the directory under test was created by this test")
        .permissions()
        .mode()
        & 0o7777
}

#[test]
fn a_restricted_directory_reports_itself_restricted() {
    let host = host();
    let directory = fresh_directory();
    let access = host.directory_access();

    access.restrict_to_owner(directory.path()).unwrap();

    assert!(
        access.is_restricted_to_owner(directory.path()).unwrap(),
        "what restrict_to_owner applies is not what is_restricted_to_owner looks for"
    );
}

#[test]
fn restricting_is_idempotent() {
    // The daemon does this on every start, not only on the start that creates the home.
    let host = host();
    let directory = fresh_directory();
    let access = host.directory_access();

    access.restrict_to_owner(directory.path()).unwrap();
    access.restrict_to_owner(directory.path()).unwrap();

    assert!(access.is_restricted_to_owner(directory.path()).unwrap());
}

#[test]
fn the_owner_can_still_use_the_directory() {
    // Shutting everyone out is only useful if it did not shut us out too.
    let host = host();
    let directory = fresh_directory();

    host.directory_access()
        .restrict_to_owner(directory.path())
        .unwrap();

    let file = directory.path().join("certs-go-here");
    std::fs::write(&file, b"the CA private key, one day").unwrap();
    assert_eq!(
        std::fs::read(&file).unwrap(),
        b"the CA private key, one day"
    );
    assert!(
        directory.path().read_dir().unwrap().count() == 1,
        "the directory should still be listable by its owner"
    );
}

#[test]
fn a_missing_directory_is_reported_rather_than_called_restricted() {
    // `mix doctor` has to tell "locked down" apart from "not there": they need different repairs.
    let host = host();
    let directory = fresh_directory();
    let missing = directory.path().join("never-created");

    let error = host
        .directory_access()
        .is_restricted_to_owner(&missing)
        .unwrap_err();

    assert!(
        error.to_string().contains("never-created"),
        "the path is the useful half of the message: {error}"
    );
}

#[cfg(unix)]
#[test]
fn unix_uses_exactly_0700() {
    let host = host();
    let directory = fresh_directory();

    host.directory_access()
        .restrict_to_owner(directory.path())
        .unwrap();

    assert_eq!(mode_of(directory.path()), 0o700);
}

#[cfg(unix)]
#[test]
fn unix_replaces_a_wider_mode_rather_than_masking_it() {
    use std::os::unix::fs::PermissionsExt as _;

    // A home restored from a backup, or moved off a filesystem that does not carry modes, can
    // arrive more permissive than this process's umask would ever have made it.
    let host = host();
    let directory = fresh_directory();
    std::fs::set_permissions(directory.path(), std::fs::Permissions::from_mode(0o777)).unwrap();

    host.directory_access()
        .restrict_to_owner(directory.path())
        .unwrap();

    assert_eq!(mode_of(directory.path()), 0o700);
}

#[cfg(unix)]
#[test]
fn unix_reports_a_group_readable_directory_as_unrestricted() {
    use std::os::unix::fs::PermissionsExt as _;

    let host = host();
    let directory = fresh_directory();
    std::fs::set_permissions(directory.path(), std::fs::Permissions::from_mode(0o750)).unwrap();

    assert!(
        !host
            .directory_access()
            .is_restricted_to_owner(directory.path())
            .unwrap()
    );
}

#[cfg(target_os = "macos")]
#[test]
fn macos_removes_an_acl_somebody_else_left_behind() {
    // The macOS half of `windows_removes_an_explicit_grant_somebody_else_left_behind`. An NFSv4 ACE
    // sits beside the mode rather than under it, so `chmod 0700` leaves it in place and working —
    // the directory reports `drwx------` while everyone can still list it. Reporting that as locked
    // down would be the worst of both.
    let host = host();
    let directory = fresh_directory();
    grant_everyone(directory.path());
    assert!(
        has_acl(directory.path()),
        "the test did not manage to set up the hostile ACE"
    );

    host.directory_access()
        .restrict_to_owner(directory.path())
        .unwrap();

    assert!(
        !has_acl(directory.path()),
        "the ACE survived: {}",
        listing(directory.path())
    );
    assert!(
        host.directory_access()
            .is_restricted_to_owner(directory.path())
            .unwrap()
    );
}

#[cfg(target_os = "macos")]
#[test]
fn macos_severs_an_inherited_ace() {
    // The macOS half of `windows_severs_an_inherited_ace`. macOS inherits ACLs too, from
    // the parent directory rather than from the volume: an ACE carrying `directory_inherit` on a
    // parent of `MIXENGINE_HOME` lands on every directory created below it, marked `inherited` and
    // fully in force. `file_inherit` reaches the files as well — which is what makes this the CA
    // private key's problem (T48) and not a tidiness one. Clearing the ACL on the directory is also
    // what stops the files created in it later from inheriting anything.
    let host = host();
    let parent = fresh_directory();
    grant_everyone_inheritable(parent.path());

    let child = parent.path().join("certs");
    std::fs::create_dir(&child).unwrap();
    assert!(
        has_acl(&child),
        "the child should have inherited an ACE; got {}",
        listing(&child)
    );

    host.directory_access().restrict_to_owner(&child).unwrap();

    assert!(
        !has_acl(&child),
        "the inherited ACE survived: {}",
        listing(&child)
    );

    // And a file created afterwards inherits nothing, because there is no longer an ACL to inherit.
    let key = child.join("ca.key");
    std::fs::write(&key, b"the CA private key, one day").unwrap();
    assert!(
        !has_acl(&key),
        "a new file inherited an ACE: {}",
        listing(&key)
    );
}

#[cfg(target_os = "macos")]
#[test]
fn macos_reports_an_acl_as_unrestricted_despite_a_correct_mode() {
    // And the reading half: `mix doctor` has to notice an ACE even though the mode beside it is
    // exactly the `0700` that was applied.
    let host = host();
    let directory = fresh_directory();
    let access = host.directory_access();

    access.restrict_to_owner(directory.path()).unwrap();
    assert!(access.is_restricted_to_owner(directory.path()).unwrap());

    grant_everyone(directory.path());
    assert_eq!(
        mode_of(directory.path()),
        0o700,
        "the mode should be untouched"
    );

    assert!(
        !access.is_restricted_to_owner(directory.path()).unwrap(),
        "an ACE went unnoticed: {}",
        listing(directory.path())
    );
}

/// Grant `everyone` an ACE the way a user sharing a folder would.
#[cfg(target_os = "macos")]
fn grant_everyone(path: &Path) {
    grant_everyone_ace(path, "everyone allow read,execute,list,search");
}

/// The same, but inherited by everything created below — how a parent directory hands its ACL down.
#[cfg(target_os = "macos")]
fn grant_everyone_inheritable(path: &Path) {
    grant_everyone_ace(
        path,
        "everyone allow list,search,file_inherit,directory_inherit",
    );
}

#[cfg(target_os = "macos")]
fn grant_everyone_ace(path: &Path, ace: &str) {
    let status = std::process::Command::new("/bin/chmod")
        .arg("+a")
        .arg(ace)
        .arg(path)
        .status()
        .expect("chmod ships with macOS");

    assert!(status.success(), "could not set up the hostile ACE");
}

#[cfg(target_os = "macos")]
fn has_acl(path: &Path) -> bool {
    // `ls -lde` prints the directory on one line and each ACE on a line of its own, so anything
    // past the first means there is an ACL. Not the `+` it also appends to the mode: that would
    // find one in the *path* too, and a temporary directory whose name happened to contain one
    // would report an ACL for ever — passing the test that wants an ACE and failing the test that
    // wants it gone, both without a word about why.
    listing(path).lines().count() > 1
}

#[cfg(target_os = "macos")]
fn listing(path: &Path) -> String {
    let output = std::process::Command::new("/bin/ls")
        .arg("-lde")
        .arg(path)
        .output()
        .expect("ls ships with macOS");

    // A failed `ls` prints nothing to stdout, which `has_acl` would read as "no ACL" — the answer
    // every assertion here is hoping for. Fail loudly instead of passing for the wrong reason.
    assert!(
        output.status.success(),
        "ls could not describe {}: {}",
        path.display(),
        String::from_utf8_lossy(&output.stderr).trim()
    );

    String::from_utf8_lossy(&output.stdout).into_owned()
}

#[cfg(windows)]
#[test]
fn windows_severs_an_inherited_ace() {
    // The whole point on Windows: an ACE carrying (OI)(CI) on a parent of `MIXENGINE_HOME` lands on
    // every directory created below it, marked `(I)` by `icacls` and fully in force. A directory
    // that still shows one has not been protected, whatever else was granted.
    //
    // The ACE is set up here rather than taken from whatever the parent happens to carry: the
    // system temporary directory inherits from `C:\` on a normal desktop, but on a build agent it
    // sits inside a service account's profile that hands nothing down, and a new directory there
    // gets the token's default DACL — no `(I)` anywhere, and the test would fail on its premise
    // without ever reaching the code under test.
    let host = host();
    let parent = fresh_directory();
    grant_everyone(parent.path());

    let child = parent.path().join("certs");
    std::fs::create_dir(&child).unwrap();
    let before = icacls(&child);
    assert!(
        before.contains("(I)"),
        "the child should have inherited an ACE; got {before}"
    );

    host.directory_access().restrict_to_owner(&child).unwrap();

    let after = icacls(&child);
    assert!(
        !after.contains("(I)"),
        "inheritance was not severed; got {after}"
    );
}

#[cfg(windows)]
#[test]
fn windows_still_grants_the_current_user() {
    let host = host();
    let directory = fresh_directory();

    host.directory_access()
        .restrict_to_owner(directory.path())
        .unwrap();

    let listing = icacls(directory.path());
    let user = std::env::var("USERNAME").expect("Windows always sets USERNAME");
    assert!(
        listing.to_lowercase().contains(&user.to_lowercase()),
        "the account that has to keep working is missing from {listing}"
    );
}

#[cfg(windows)]
#[test]
fn windows_removes_an_explicit_grant_somebody_else_left_behind() {
    // The hole `/inheritance:r` alone does not close. An ACE that is explicit rather than
    // inherited — a directory the user had already shared, or one restored from a backup with its
    // ACL — carries no `(I)` flag and survives everything except `/reset`. Reporting that
    // directory as locked down while Everyone holds full control is the worst of both.
    let host = host();
    let directory = fresh_directory();
    grant_everyone(directory.path());
    assert!(
        icacls(directory.path()).contains("Everyone"),
        "the test did not manage to set up the hostile ACE"
    );

    host.directory_access()
        .restrict_to_owner(directory.path())
        .unwrap();

    let listing = icacls(directory.path());
    assert!(
        !listing.contains("Everyone"),
        "Everyone survived: {listing}"
    );
    assert!(
        host.directory_access()
            .is_restricted_to_owner(directory.path())
            .unwrap()
    );
}

#[cfg(windows)]
#[test]
fn windows_reports_an_extra_grant_as_unrestricted() {
    // And the reading half: once somebody adds a fourth ACE, `mix doctor` has to notice, even
    // though nothing about that ACE is marked inherited.
    let host = host();
    let directory = fresh_directory();
    let access = host.directory_access();

    access.restrict_to_owner(directory.path()).unwrap();
    assert!(access.is_restricted_to_owner(directory.path()).unwrap());

    grant_everyone(directory.path());

    assert!(
        !access.is_restricted_to_owner(directory.path()).unwrap(),
        "a fourth ACE went unnoticed: {}",
        icacls(directory.path())
    );
}

/// `S-1-1-0` is `Everyone`, by SID so the test reads the same on a localised Windows.
#[cfg(windows)]
fn grant_everyone(path: &Path) {
    // `output` rather than `status`: `/q` silences the per-file line but not the summary, and a
    // test that prints over the harness is a test people stop reading.
    let output = std::process::Command::new("icacls")
        .arg(path)
        .arg("/grant")
        .arg("*S-1-1-0:(OI)(CI)F")
        .arg("/q")
        .output()
        .expect("icacls ships with Windows");

    assert!(output.status.success(), "could not set up the hostile ACE");
}

#[cfg(windows)]
fn icacls(path: &Path) -> String {
    let output = std::process::Command::new("icacls")
        .arg(path)
        .output()
        .expect("icacls ships with Windows");

    String::from_utf8_lossy(&output.stdout).into_owned()
}

#[test]
fn the_mock_records_what_it_was_asked_to_restrict() {
    let host = mock::Host::with_home("/unused");
    let first = Path::new("/unused/certs");
    let second = Path::new("/unused/data");

    host.directory_access().restrict_to_owner(first).unwrap();
    host.directory_access().restrict_to_owner(second).unwrap();

    assert_eq!(
        host.restricted(),
        vec![first.to_path_buf(), second.to_path_buf()]
    );
    assert!(
        host.directory_access()
            .is_restricted_to_owner(first)
            .unwrap()
    );
    assert!(
        !host
            .directory_access()
            .is_restricted_to_owner(Path::new("/unused/etc"))
            .unwrap()
    );
}

#[test]
fn a_mock_that_refuses_reports_why() {
    let host = mock::Host::refusing_to_restrict("/unused", "this filesystem has no permissions");

    let error = host
        .directory_access()
        .restrict_to_owner(Path::new("/unused/certs"))
        .unwrap_err();

    assert!(error.to_string().contains("no permissions"), "{error}");
    assert!(host.restricted().is_empty());
}
