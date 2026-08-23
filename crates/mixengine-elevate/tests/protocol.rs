//! The request/response lifecycle, against the real binary, under whatever token this suite has.
//!
//! **Not `#[ignore]`d, and that is D5's doing:** `Probe` needs no administrative token, so the whole
//! protocol — read, validate, decode, apply, answer — is proved here rather than only in the elevated
//! job. What is left for `system.rs` is the audit directory, which genuinely needs one.
//!
//! **The trap this suite is written around.** The Windows leg of CI runs under a full administrator
//! token (T2b), so an assertion phrased "refused because the token is not elevated" would be red
//! there and green everywhere else for reasons that have nothing to do with the code. Nothing here
//! asserts on the value of `elevated`; the assertions are about what the helper *reports* and how it
//! answers, which reads the same from any token.

mod harness;

use std::path::Path;

use mixengine_proto::privileged::OpOutcome;

#[test]
fn a_probe_is_applied_and_the_report_arrives_with_it() {
    let request = harness::Request::new();
    let path = request.write();

    let ran = harness::run(&path);

    assert_eq!(ran.code, Some(0), "{}", ran.stderr);
    let response = ran
        .response
        .expect("exit 0 means there is a report to read");
    assert_eq!(
        response.nonce, "n",
        "the nonce is echoed so an old answer cannot pass for this one"
    );
    assert_eq!(
        response.supported_ops,
        mixengine_proto::privileged::PrivilegedOp::ALL,
        "the report names every operation this build knows"
    );
    assert!(!response.elevate_version.is_empty());
    assert!(
        response.audit_log.is_absolute(),
        "{}",
        response.audit_log.display()
    );
    assert!(matches!(
        response.results.as_slice(),
        [OpOutcome::Applied { .. }]
    ));
}

#[test]
fn no_arguments_is_a_caller_bug() {
    let ran = harness::run_with(&[]);

    assert_eq!(ran.code, Some(64), "{}", ran.stderr);
}

#[test]
fn a_second_argument_is_a_caller_bug_too() {
    let request = harness::Request::new();
    let path = request.write();

    let ran = harness::run_with(&[path.as_path(), Path::new("and-another")]);

    assert_eq!(ran.code, Some(64), "{}", ran.stderr);
}

#[test]
fn a_request_that_is_not_there_is_refused_without_a_response() {
    let request = harness::Request::new();
    let missing = request.directory().join("never-written.json");

    let ran = harness::run(&missing);

    assert_eq!(ran.code, Some(65), "{}", ran.stderr);
    assert!(ran.response.is_none());
}

#[test]
fn malformed_json_is_refused_without_a_response() {
    let request = harness::Request::new();
    let path = request.write();
    std::fs::write(&path, "{ not json").unwrap();

    let ran = harness::run(&path);

    assert_eq!(ran.code, Some(65), "{}", ran.stderr);
    assert!(ran.response.is_none());
}

#[test]
fn a_protocol_this_build_does_not_know_is_refused_without_a_response() {
    let request = harness::Request::new().version(99);
    let path = request.write();

    let ran = harness::run(&path);

    assert_eq!(ran.code, Some(65), "{}", ran.stderr);
    assert!(ran.response.is_none());
}

/// D10. And the old answer is left exactly as it was: a helper that overwrote it would destroy the
/// evidence of what the first run did.
#[test]
fn a_request_that_already_has_an_answer_is_refused_and_the_answer_is_untouched() {
    let request = harness::Request::new();
    let path = request.write();
    let response = path.with_file_name("response.json");
    std::fs::write(&response, "an earlier answer").unwrap();

    let ran = harness::run(&path);

    assert_eq!(ran.code, Some(65), "{}", ran.stderr);
    assert_eq!(
        std::fs::read_to_string(&response).unwrap(),
        "an earlier answer"
    );
}

/// D4. `/` and `C:\Windows` belong to an account this process is not, so a request naming one of them
/// as its home is a request trying to reach outside the home it was given.
#[test]
fn a_home_this_caller_does_not_own_is_refused_without_a_response() {
    let elsewhere = if cfg!(windows) { r"C:\Windows" } else { "/" };
    let request = harness::Request::new().home(Path::new(elsewhere));
    let path = request.write();

    let ran = harness::run(&path);

    assert_eq!(ran.code, Some(65), "{}", ran.stderr);
    assert!(ran.response.is_none());
}

/// D3, end to end: the whole point of decoding one at a time. An unknown operation is reported **at
/// its own index** and its neighbour is applied — a `Vec<PrivilegedOp>` would have failed the batch.
#[test]
fn an_unknown_operation_is_reported_at_its_index_and_its_neighbour_is_applied() {
    let request = harness::Request::new()
        .ops(r#"[{ "op": "probe" }, { "op": "trust-ca-install", "der": [1] }, { "op": "probe" }]"#);
    let path = request.write();

    let ran = harness::run(&path);

    assert_eq!(ran.code, Some(0), "{}", ran.stderr);
    let results = ran.response.expect("a report").results;
    assert!(matches!(results[0], OpOutcome::Applied { .. }));
    assert!(matches!(results[1], OpOutcome::Unsupported { .. }));
    assert!(matches!(results[2], OpOutcome::Applied { .. }));
}

/// The other half of D3: a field inside an operation this build knows is fatal for that operation and
/// for nothing else.
#[test]
fn a_field_this_build_does_not_know_is_fatal_for_its_own_operation_alone() {
    let request = harness::Request::new()
        .ops(r#"[{ "op": "probe", "only-if": "later" }, { "op": "probe" }]"#);
    let path = request.write();

    let ran = harness::run(&path);

    assert_eq!(ran.code, Some(0), "{}", ran.stderr);
    let results = ran.response.expect("a report").results;
    assert!(matches!(results[0], OpOutcome::Unsupported { .. }));
    assert!(matches!(results[1], OpOutcome::Applied { .. }));
}

#[test]
fn an_empty_batch_asks_for_nothing_and_is_refused() {
    let request = harness::Request::new().ops("[]");
    let path = request.write();

    let ran = harness::run(&path);

    assert_eq!(ran.code, Some(65), "{}", ran.stderr);
    assert!(ran.response.is_none());
}

#[cfg(unix)]
#[test]
fn a_request_reached_through_a_symlink_is_refused() {
    let request = harness::Request::new();
    let path = request.write();
    let link = path.with_file_name("linked.json");
    std::os::unix::fs::symlink(&path, &link).unwrap();

    let ran = harness::run(&link);

    assert_eq!(ran.code, Some(65), "{}", ran.stderr);
}

#[cfg(unix)]
#[test]
fn a_request_anybody_can_rewrite_is_refused() {
    use std::os::unix::fs::PermissionsExt as _;

    let request = harness::Request::new();
    let path = request.write();
    std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o666)).unwrap();

    let ran = harness::run(&path);

    assert_eq!(ran.code, Some(65), "{}", ran.stderr);
}
