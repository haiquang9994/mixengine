//! The primitives the elevated helper is built out of, against the real OS.
//!
//! Nothing here is a system test: no privileged directory is created, and every assertion reads a
//! fact rather than attempting an access a token gets to decide. That distinction is the Privilege
//! section of `.claude/standards/testing.md` — the Windows leg of CI runs under a full administrator
//! token, so a test that proved something by *trying* it would pass there for a reason that will not
//! exist on a user's machine.

use mixengine_platform::elevated;
use tempfile::TempDir;

#[test]
fn a_file_this_process_just_created_belongs_to_this_process() {
    let directory = TempDir::new().unwrap();
    let file = directory.path().join("request.json");
    std::fs::write(&file, b"{}").unwrap();

    let owner = elevated::owner_of(&file).unwrap();
    let of_the_directory = elevated::owner_of(directory.path()).unwrap();

    assert_eq!(
        owner, of_the_directory,
        "the same account created both, so the helper's `home` check would refuse its own request"
    );
    assert!(
        !owner.to_string().is_empty(),
        "an owner has to be printable"
    );
}

#[test]
fn a_path_that_is_not_there_is_reported_rather_than_guessed_at() {
    let directory = TempDir::new().unwrap();

    let error = elevated::owner_of(&directory.path().join("never-created")).unwrap_err();

    assert!(error.to_string().contains("never-created"), "{error}");
}

#[test]
fn the_audit_directory_is_an_absolute_path_outside_any_home() {
    let directory = elevated::audit_directory().unwrap();

    assert!(directory.is_absolute(), "{}", directory.display());
    assert!(
        directory.ends_with("mixengine") || directory.ends_with("MixEngine"),
        "{}",
        directory.display()
    );
}

/// Reading the answer twice must give the same answer: the helper asks once and reports it, and a
/// report that disagreed with the gate beside it would be worse than no report.
#[test]
fn elevation_is_a_stable_fact_about_this_process() {
    assert_eq!(elevated::is_elevated(), elevated::is_elevated());
}

#[cfg(unix)]
#[test]
fn unix_notices_a_mode_that_lets_anybody_write() {
    use std::os::unix::fs::PermissionsExt as _;

    let directory = TempDir::new().unwrap();
    let file = directory.path().join("request.json");
    std::fs::write(&file, b"{}").unwrap();

    std::fs::set_permissions(&file, std::fs::Permissions::from_mode(0o600)).unwrap();
    assert!(!elevated::others_can_write(&file).unwrap());

    std::fs::set_permissions(&file, std::fs::Permissions::from_mode(0o666)).unwrap();
    assert!(elevated::others_can_write(&file).unwrap());

    std::fs::set_permissions(&file, std::fs::Permissions::from_mode(0o620)).unwrap();
    assert!(
        elevated::others_can_write(&file).unwrap(),
        "the group can write it, which is enough for somebody else to rewrite the request"
    );
}

#[cfg(unix)]
#[test]
fn unix_knows_root_from_everybody_else() {
    let directory = TempDir::new().unwrap();
    let owner = elevated::owner_of(directory.path()).unwrap();

    // `unsafe`-free: the id this process runs as is what the file it just created carries.
    let is_root = owner.to_string() == "0";

    assert_eq!(owner.is_superuser(), is_root);
    assert_eq!(owner.is_administrative(), is_root);
}

#[cfg(windows)]
#[test]
fn windows_reports_an_owner_as_a_sid() {
    let directory = TempDir::new().unwrap();

    let owner = elevated::owner_of(directory.path()).unwrap();

    assert!(
        owner.to_string().starts_with("S-1-"),
        "an owner is named by SID, which survives a rename and is not localised: {owner}"
    );
}

/// An administrator's own files are owned by `BUILTIN\Administrators`, and most Windows users are
/// administrators — so "owned by root" cannot mean that here without refusing the ordinary case.
/// SYSTEM is what it means; Administrators is *administrative* but not SYSTEM.
#[cfg(windows)]
#[test]
fn windows_separates_system_from_merely_administrative() {
    let directory = TempDir::new().unwrap();
    let owner = elevated::owner_of(directory.path()).unwrap();

    assert!(
        !owner.is_superuser(),
        "a directory this process created must not be read as SYSTEM's: {owner}"
    );

    if owner.to_string() == "S-1-5-32-544" {
        assert!(owner.is_administrative(), "{owner}");
    }
}
