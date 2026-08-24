//! What only an administrative token can prove: the audit log at its real system path.
//!
//! **`#[ignore]`d**, so a machine where these do not run says so instead of reporting a pass. CI's
//! `system` job runs them elevated on all three operating systems; `cargo test` on a developer
//! machine skips them and says how many it skipped.
//!
//! **The permissions are asserted structurally — by reading them, never by attempting an access.**
//! An elevated process can open anything, so a test that proved exclusion by trying it would pass
//! for a privilege the user will not have. That is the rule in `.claude/standards/testing.md`, and
//! `crates/mixengine-platform/tests/access.rs` is where this codebase already keeps it.

mod harness;

use std::path::Path;

use mixengine_proto::privileged::OpOutcome;

/// Where the log belongs on this system. Duplicated from `mixengine_platform::elevated` on purpose:
/// a test that asked the code under test where to look could not notice it looking in the wrong
/// place.
#[cfg(target_os = "linux")]
const DIRECTORY: &str = "/var/log/mixengine";
#[cfg(target_os = "macos")]
const DIRECTORY: &str = "/Library/Logs/MixEngine";

#[cfg(windows)]
fn directory() -> std::path::PathBuf {
    Path::new(&std::env::var_os("ProgramData").expect("Windows always sets ProgramData"))
        .join("MixEngine")
}

#[cfg(unix)]
fn directory() -> std::path::PathBuf {
    std::path::PathBuf::from(DIRECTORY)
}

#[test]
#[ignore = "needs an administrative token; run in CI's system job"]
fn the_helper_reports_itself_elevated_and_says_where_its_log_is() {
    let request = harness::Request::new().owned_by_the_caller();
    let path = request.write();

    let ran = harness::run(&path);

    assert_eq!(ran.code, Some(0), "{}", ran.stderr);
    let response = ran.response.expect("a report");
    assert!(
        response.elevated,
        "this suite is only meaningful under an administrative token"
    );
    assert_eq!(response.audit_log, directory().join("elevate.log"));
    assert!(matches!(
        response.results.as_slice(),
        [OpOutcome::Applied { .. }]
    ));
}

#[test]
#[ignore = "needs an administrative token; run in CI's system job"]
fn the_log_is_created_on_first_run_and_appended_to_on_the_second() {
    let log = directory().join("elevate.log");
    let before = lines(&log);

    // A nonce of this test's own, so the two lines counted below are this test's two and not a
    // neighbour's — measured, not reasoned about: this assertion read 4 where it expected 2 the
    // first time it ran.
    let nonce = "the-log-is-appended-to";

    for _ in 0..2 {
        let request = harness::Request::new().nonce(nonce).owned_by_the_caller();
        let ran = harness::run(&request.write());
        assert_eq!(ran.code, Some(0), "{}", ran.stderr);
    }

    // `skip(before)` and not a filter over the whole file: this log is never cleaned between runs,
    // so a second run of this suite on the same machine finds its own earlier lines carrying the
    // same nonce and counts four. CI never noticed, because every run there is a fresh machine.
    let text = std::fs::read_to_string(&log).expect("the helper created it");
    let mine: Vec<serde_json::Value> = text
        .lines()
        .skip(before)
        .map(|line| serde_json::from_str(line).expect("each line is its own document"))
        .filter(|entry: &serde_json::Value| entry["nonce"] == nonce)
        .collect();

    assert_eq!(
        mine.len(),
        2,
        "one line per operation per invocation, appended"
    );
    assert!(
        lines(&log) >= before + 2,
        "nothing that was already in the log was replaced"
    );
    assert!(mine[0]["at"].is_u64(), "{}", mine[0]);
    assert_eq!(mine[0]["op"], "probe");
}

#[cfg(unix)]
#[test]
#[ignore = "needs an administrative token; run in CI's system job"]
fn unix_keeps_the_log_root_owned_and_world_readable() {
    use std::os::unix::fs::MetadataExt as _;
    use std::os::unix::fs::PermissionsExt as _;

    let request = harness::Request::new().owned_by_the_caller();
    assert_eq!(harness::run(&request.write()).code, Some(0));

    let metadata = std::fs::metadata(directory()).expect("the helper created it");

    assert_eq!(
        metadata.uid(),
        0,
        "a directory root appends into must be root's"
    );
    assert_eq!(
        metadata.permissions().mode() & 0o7777,
        0o755,
        "the log is evidence, and evidence nobody may read is not evidence"
    );
}

/// The regression test for what CI caught: a directory that exists is not a directory that was
/// finished. Creating one is `mkdir` followed by a permissions call, so a second helper arriving
/// between the two sees a directory that is there and permissions that are still the parent's — and
/// a `prepare` that only applied them on the branch that creates the directory would leave that
/// state permanent. Here the wrong permissions are set deliberately; the next run must correct them.
#[cfg(unix)]
#[test]
#[ignore = "needs an administrative token; run in CI's system job"]
fn unix_repairs_permissions_it_finds_wrong() {
    use std::os::unix::fs::PermissionsExt as _;

    // Make sure it is there, and root's, before breaking it.
    let first = harness::Request::new().owned_by_the_caller();
    assert_eq!(harness::run(&first.write()).code, Some(0));

    std::fs::set_permissions(directory(), std::fs::Permissions::from_mode(0o700))
        .expect("root may change the mode of a directory root owns");

    let request = harness::Request::new().owned_by_the_caller();
    assert_eq!(harness::run(&request.write()).code, Some(0));

    let mode = std::fs::metadata(directory())
        .expect("still there")
        .permissions()
        .mode()
        & 0o7777;

    assert_eq!(
        mode, 0o755,
        "the log is evidence, and a run that found it unreadable left it unreadable"
    );
}

/// The same regression, in the form CI actually reported it: every ACE marked `(I)`, inherited
/// straight from `%ProgramData%`, on a directory whose whole purpose is not to inherit from there.
#[cfg(windows)]
#[test]
#[ignore = "needs an administrative token; run in CI's system job"]
fn windows_repairs_an_acl_it_finds_inherited() {
    let first = harness::Request::new().owned_by_the_caller();
    assert_eq!(harness::run(&first.write()).code, Some(0));

    let restored = std::process::Command::new("icacls")
        .arg(directory())
        .args(["/inheritance:e", "/q"])
        .output()
        .expect("icacls ships with Windows");
    assert!(restored.status.success(), "{restored:?}");

    let request = harness::Request::new().owned_by_the_caller();
    assert_eq!(harness::run(&request.write()).code, Some(0));

    let listing = icacls(&directory());

    assert!(
        !listing.contains("(I)"),
        "a run that found the ACL inherited left it inherited; got {listing}"
    );
}

/// D4, and only reachable here: producing a file owned by somebody else needs a privilege the
/// ordinary suite does not have. Running as root, this test's own request *is* root's.
#[cfg(unix)]
#[test]
#[ignore = "needs an administrative token; run in CI's system job"]
fn unix_refuses_a_request_that_belongs_to_root() {
    let request = harness::Request::new();
    let path = request.write();

    // Deliberately *not* handed to the calling user: this is what a request written by root looks
    // like, and no daemon runs as root.
    let ran = harness::run(&path);

    assert_eq!(ran.code, Some(65), "{}", ran.stderr);
    assert!(ran.response.is_none());
}

#[cfg(windows)]
#[test]
#[ignore = "needs an administrative token; run in CI's system job"]
fn windows_keeps_the_log_out_of_reach_of_an_ordinary_account() {
    let request = harness::Request::new().owned_by_the_caller();
    assert_eq!(harness::run(&request.write()).code, Some(0));

    let listing = icacls(&directory());

    assert!(
        !listing.contains("(I)"),
        "inheritance from %ProgramData% was not severed; got {listing}"
    );
    assert!(
        listing.contains("(OI)(CI)(F)") || listing.contains("(OI)(CI)F"),
        "Administrators and SYSTEM must be able to write it; got {listing}"
    );
    assert!(
        listing.contains("(OI)(CI)(RX)") || listing.contains("(OI)(CI)(R)"),
        "an ordinary account must be able to read it back; got {listing}"
    );
}

#[cfg(windows)]
fn icacls(path: &Path) -> String {
    let output = std::process::Command::new("icacls")
        .arg(path)
        .output()
        .expect("icacls ships with Windows");

    String::from_utf8_lossy(&output.stdout).into_owned()
}

fn lines(log: &Path) -> usize {
    std::fs::read_to_string(log).map_or(0, |text| text.lines().count())
}

/// The whole of T49a against a real trust store: in, idempotent, out, idempotent.
///
/// **One test rather than four**, so a runner is never left holding a certificate a later job would
/// find. The four assertions are stages of one round trip and the removal runs whatever the install
/// answered, because a test that installed and then failed before removing would poison the machine
/// it ran on.
///
/// What is proved is the pair of answers only a real store can give: `Applied` the first time and
/// `AlreadyDone` the second. A unit test can prove the shape check refuses things; only this can
/// prove the mechanism underneath it works and is idempotent.
#[test]
#[ignore = "changes this machine's trust store; run in CI's system job"]
fn an_authority_goes_into_this_machines_trust_store_and_comes_back_out() {
    let (der, key_id) = an_authority();

    let installed = apply(&format!(
        r#"[{{ "op": "trust-ca-install", "plan": {{ "method": "{}", "der": {} }} }}]"#,
        method(),
        numbers(&der)
    ));
    assert!(
        matches!(installed, OpOutcome::Applied { .. }),
        "the install did not change the machine: {installed:?}"
    );

    let again = apply(&format!(
        r#"[{{ "op": "trust-ca-install", "plan": {{ "method": "{}", "der": {} }} }}]"#,
        method(),
        numbers(&der)
    ));

    let removed = apply(&format!(
        r#"[{{ "op": "trust-ca-remove", "target": {{ "method": "{}", "key_id": "{key_id}" }} }}]"#,
        method()
    ));

    // Asserted after the removal has run, so a machine is not left holding this certificate because
    // an assertion above it failed.
    assert!(
        matches!(again, OpOutcome::AlreadyDone),
        "a second install was not idempotent: {again:?}"
    );
    assert!(
        matches!(removed, OpOutcome::Applied { .. }),
        "the removal did not change the machine: {removed:?}"
    );

    let gone = apply(&format!(
        r#"[{{ "op": "trust-ca-remove", "target": {{ "method": "{}", "key_id": "{key_id}" }} }}]"#,
        method()
    ));
    assert!(
        matches!(gone, OpOutcome::AlreadyDone),
        "a second removal was not idempotent: {gone:?}"
    );
}

/// **The security claim, end to end through the real binary under a real token.**
///
/// A unit test proves `trust::remove` refuses a name that is not an authority's. This proves the
/// refusal survives the whole path — the request file, the validation, the dispatch — while the
/// process actually holds the privilege to have done the damage.
#[test]
#[ignore = "needs an administrative token; run in CI's system job"]
fn an_elevated_helper_still_refuses_to_remove_something_that_is_not_our_authority() {
    let outcome = apply(&format!(
        r#"[{{ "op": "trust-ca-remove", "target": {{ "method": "{}", "key_id": "DigiCert" }} }}]"#,
        method()
    ));

    assert!(
        matches!(outcome, OpOutcome::Refused { .. }),
        "an elevated helper accepted a removal aimed at something that is not ours: {outcome:?}"
    );
}

/// And the same for an install that is not a MixEngine authority.
#[test]
#[ignore = "needs an administrative token; run in CI's system job"]
fn an_elevated_helper_still_refuses_to_trust_something_it_did_not_make() {
    let outcome = apply(&format!(
        r#"[{{ "op": "trust-ca-install", "plan": {{ "method": "{}", "der": [48, 130, 1, 0] }} }}]"#,
        method()
    ));

    assert!(
        matches!(outcome, OpOutcome::Refused { .. }),
        "an elevated helper accepted a certificate it did not make: {outcome:?}"
    );
}

/// Which store this machine has, spelled the way the wire spells it.
fn method() -> &'static str {
    #[cfg(windows)]
    return "system-root";
    #[cfg(target_os = "macos")]
    return "system-keychain";
    // The runners are Debian-family; a Red Hat one would need the other name, and the assertion
    // that fails would say which.
    #[cfg(target_os = "linux")]
    return "ca-certificates";
}

/// A certificate shaped the way `mixengine_core::certs::ca` shapes one, and its identifier.
fn an_authority() -> (Vec<u8>, String) {
    use rcgen::{BasicConstraints, CertificateParams, DnType, IsCa, KeyPair, KeyUsagePurpose};

    // Not T48's own derivation: this suite never has a daemon's key, and what the helper checks is
    // the *shape* of the name rather than its relationship to the key — the T49a design, D4.
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

/// DER as the array of numbers `serde` renders a `Vec<u8>` into.
fn numbers(der: &[u8]) -> String {
    let rendered: Vec<String> = der.iter().map(u8::to_string).collect();

    format!("[{}]", rendered.join(","))
}

/// Run one operation and hand back its outcome.
fn apply(ops: &str) -> OpOutcome {
    let request = harness::Request::new().ops(ops).owned_by_the_caller();
    let ran = harness::run(&request.write());

    assert_eq!(ran.code, Some(0), "{}", ran.stderr);

    ran.response
        .expect("a report")
        .results
        .into_iter()
        .next()
        .expect("one outcome for one operation")
}
