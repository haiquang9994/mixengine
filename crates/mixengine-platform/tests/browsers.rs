//! The round trip against a real `certutil`, on a database made for the test — roadmap task
//! **T49b**.
//!
//! **`#[ignore]`d rather than skipped**, in the shape `crates/mixengine-cli/tests/caddy.rs` uses: a
//! machine without `libnss3-tools` says the test did not run instead of quietly reporting a pass.
//! CI's `test (ubuntu-latest)` job installs the package and runs this with `--ignored`, which is
//! what makes the claim measured rather than optional.
//!
//! **Nothing here touches a real profile.** `.claude/standards/testing.md`'s first rule is about the
//! user's own stores; the database below is made by `certutil -N` inside a temp directory that the
//! whole search is rooted at, and it is deleted with it.

#![cfg(target_os = "linux")]

use std::path::Path;
use std::process::Command;

use mixengine_platform::BrowserTrust as _;

/// Make an empty NSS database, the way a browser's first run would.
///
/// **The one `certutil -N` in this repository, and it is here.** MixEngine itself never creates a
/// database — the T49b design, D4 — so the only place one is made is a test that then deletes it.
fn database(directory: &Path) {
    let status = Command::new("certutil")
        .args([
            "-N",
            "-d",
            &format!("sql:{}", directory.display()),
            "--empty-password",
        ])
        .status()
        .expect("certutil runs — this test is #[ignore]d on machines without libnss3-tools");

    assert!(status.success(), "certutil -N refused to make a database");
}

/// A certificate shaped the way `mixengine_core::certs::ca` shapes one, and its identifier.
///
/// The same fixture `crates/mixengine-elevate/tests/system.rs` uses, and not T48's own derivation:
/// this suite has no daemon's key, and what is checked here is the *shape* of the name.
fn an_authority() -> (Vec<u8>, String) {
    use rcgen::{BasicConstraints, CertificateParams, DnType, IsCa, KeyPair, KeyUsagePurpose};

    let key_id = "5ec0de5a".to_owned();

    let key = KeyPair::generate().expect("a key pair");
    let mut params = CertificateParams::default();
    params.is_ca = IsCa::Ca(BasicConstraints::Constrained(0));
    params.key_usages = vec![KeyUsagePurpose::KeyCertSign, KeyUsagePurpose::CrlSign];
    params.not_before = rcgen::date_time_ymd(2026, 1, 1);
    params.not_after = rcgen::date_time_ymd(2036, 1, 1);
    params
        .distinguished_name
        .push(DnType::CommonName, format!("MixEngine Local CA {key_id}"));

    let der = params
        .self_signed(&key)
        .expect("a certificate")
        .der()
        .to_vec();

    (der, key_id)
}

/// Install, find, install again, remove, remove again.
///
/// **One test and not five**, because each step is only meaningful against the state the one before
/// it left: an install that reports `Unchanged` proves nothing unless something is known to be
/// there, and a removal proves nothing unless it is the second half of an install.
#[test]
#[ignore = "needs certutil from libnss3-tools; CI's ubuntu leg runs it with --ignored"]
fn an_authority_goes_into_a_database_and_comes_back_out() {
    let home = tempfile::tempdir().expect("a temp home");
    let profile = home.path().join(".mozilla/firefox/test.default");
    std::fs::create_dir_all(&profile).expect("the profile directory is made");
    database(&profile);

    // Discovery takes the home, so pointing it at the temp one is the whole of the isolation.
    let found = mixengine_platform::browsers::databases_under(home.path());
    assert_eq!(
        found.len(),
        1,
        "the fixture database was not found: {found:?}"
    );

    let (der, key_id) = an_authority();
    let browsers = mixengine_platform::browsers::under(home.path());

    let written = browsers.install(&der).expect("the install answers");
    assert_eq!(written.written.len(), 1, "refused: {:?}", written.refused);

    let survey = browsers.survey(&der).expect("the survey answers");
    assert!(survey.lacking().is_empty(), "still lacking: {survey:?}");

    // Idempotent: a database already holding exactly these bytes is not written to again.
    let again = browsers.install(&der).expect("the second install answers");
    assert!(again.written.is_empty(), "wrote twice: {again:?}");

    let removed = browsers.remove(&key_id).expect("the removal answers");
    assert_eq!(removed.written.len(), 1, "refused: {:?}", removed.refused);

    let after = browsers.survey(&der).expect("the survey answers");
    assert_eq!(after.lacking().len(), 1, "it is still there: {after:?}");

    // And a removal with nothing to remove is not a failure.
    let nothing = browsers
        .remove(&key_id)
        .expect("the second removal answers");
    assert!(nothing.written.is_empty(), "removed twice: {nothing:?}");
}

/// **A certificate under our nickname that is not ours is left alone** — the T49a design, D5's
/// second check, which this task gets for free because the reader already exists.
///
/// The removal is the direction that can do damage, and a name is not proof of provenance.
#[test]
#[ignore = "needs certutil from libnss3-tools; CI's ubuntu leg runs it with --ignored"]
fn something_else_wearing_our_nickname_is_not_deleted() {
    use rcgen::{CertificateParams, DnType, KeyPair};

    let home = tempfile::tempdir().expect("a temp home");
    let profile = home.path().join(".mozilla/firefox/test.default");
    std::fs::create_dir_all(&profile).expect("the profile directory is made");
    database(&profile);

    let (_, key_id) = an_authority();

    // Not an authority at all: no `basicConstraints`, no `keyUsage`. It wears the name and nothing
    // else, which is exactly the thing a nickname-only removal would delete.
    let key = KeyPair::generate().expect("a key pair");
    let mut params = CertificateParams::default();
    params.not_before = rcgen::date_time_ymd(2026, 1, 1);
    params.not_after = rcgen::date_time_ymd(2036, 1, 1);
    params
        .distinguished_name
        .push(DnType::CommonName, format!("MixEngine Local CA {key_id}"));
    let impostor = params.self_signed(&key).expect("a certificate");

    // Through a file, because `certutil` will not read a certificate from a pipe — measured, and
    // recorded beside the implementation's own `add`.
    let file = home.path().join("impostor.pem");
    std::fs::write(&file, impostor.pem()).expect("the impostor is written");

    let status = Command::new("certutil")
        .args([
            "-A",
            "-d",
            &format!("sql:{}", profile.display()),
            "-n",
            &format!("MixEngine Local CA {key_id}"),
            "-t",
            "C,,",
            "-i",
            &file.to_string_lossy(),
        ])
        .status()
        .expect("certutil runs");
    assert!(status.success(), "the impostor was not installed");

    let browsers = mixengine_platform::browsers::under(home.path());
    let removed = browsers.remove(&key_id).expect("the removal answers");

    assert!(
        removed.written.is_empty(),
        "something that is not ours was deleted: {removed:?}"
    );
    assert_eq!(
        removed.refused.len(),
        1,
        "the refusal was not reported: {removed:?}"
    );
}
