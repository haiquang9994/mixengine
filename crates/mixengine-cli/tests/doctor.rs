//! `mix doctor` against a real daemon.
//!
//! Roadmap task **T47a**'s client half. What the daemon's own `tests/api.rs` proves is that the
//! checks answer; what is proved here is the part only `mix` can be wrong about — that every check
//! reaches the screen, and that the exit code is the *report* rather than the call.
//!
//! **The assertion that matters is that a check can fail.** A suite that only ever sees a healthy
//! machine has proved that the doctor runs, not that it looks — so each `Ok` here is paired, in the
//! same test, with the arrangement that turns it into a `Problem` (T47a design, D10).
//!
//! **And nothing here may assume the machine running it is well.** The first version of this file
//! did, and CI answered: the GitHub Windows runner has **port 80 inside a reserved range**, so
//! `mix doctor` correctly reports a problem on a home with nothing in it. That is the check doing
//! its job — a front end on that machine genuinely could not bind 80 — so what was wrong was the
//! test's premise, not the finding. Each assertion below therefore names the condition it is about
//! and ignores every other, which isolates the variable instead of demanding a pristine machine.

mod harness;

use harness::{Home, stdout};

/// Every check reaches the screen, whatever it answered.
#[tokio::test(flavor = "multi_thread")]
async fn every_check_is_reported_and_named() {
    let home = Home::new();
    let _daemon = home.start_daemon();

    // Parsed rather than taken through `harness::json`, which asserts a zero exit — and this
    // command's exit code is the *report*, so a runner with a reserved port range is a legitimate
    // non-zero here.
    let printed = home.mix(&["doctor", "--json"]);
    let report: serde_json::Value = serde_json::from_slice(&printed.stdout)
        .unwrap_or_else(|error| panic!("{error}: {}", stdout(&printed)));

    assert_eq!(
        report["checks"].as_array().map(Vec::len),
        Some(11),
        "{report}"
    );

    let table = stdout(&home.mix(&["doctor"]));

    // The per-system fact ADR 0007 exists to keep honest, on the screen rather than only on the
    // wire.
    assert!(table.contains("descendant"), "{table}");
    assert!(table.lines().count() >= 11, "{table}");
}

/// A site whose name nothing routes is a problem, and the exit code says so — which is the half a
/// healthy machine cannot demonstrate.
#[tokio::test(flavor = "multi_thread")]
async fn a_name_nothing_resolves_is_a_problem_and_a_non_zero_exit() {
    let home = Home::new();
    let _daemon = home.start_daemon();
    let repository = tempfile::Builder::new()
        .prefix("mixengine-doctor")
        .tempdir()
        .expect("a temporary directory");
    let root = repository.path().display().to_string();

    // **The condition is absent first.** Not "this home is healthy" — CI measured a Windows runner
    // with port 80 reserved, where it never is — but "no domain is unreachable, because no domain is
    // declared". That is what makes the assertion below about the site rather than about the runner.
    assert!(
        !unreachable(&home),
        "a home with no sites has no domain that could fail to resolve"
    );

    home.mix(&["project", "create", &root, "--name", "blog"]);
    home.mix(&[
        "site",
        "create",
        "--project",
        "blog",
        "--domain",
        "blog.test",
        "--kind",
        "static",
    ]);

    assert!(
        unreachable(&home),
        "a declared name nothing routes is what this check is for"
    );

    // And the exit code follows the report, which is the half a client cannot see.
    let after = home.mix(&["doctor"]);

    assert!(!after.status.success(), "{}", stdout(&after));
    assert!(stdout(&after).contains("PROBLEM"), "{}", stdout(&after));
}

/// **Check 10 from the end a person is at.** A generated file somebody edited by hand is no longer
/// what the row renders to, and `mix doctor` says so.
///
/// The first assertion is the control, taken with the same instrument a moment earlier: a home whose
/// configuration was just rendered must be quiet, or the second assertion would be evidence that the
/// check fires on everything rather than evidence that it noticed this.
/// A plain `#[test]`, unlike its neighbours: `Home::declare` drives a runtime of its own to reach
/// `service.create`, and a runtime cannot be started from inside one. Every suite that declares a
/// service is arranged this way — see `tests/service.rs`.
#[test]
#[cfg_attr(
    not(debug_assertions),
    ignore = "the fakeservice recipe is compiled into debug builds only"
)]
fn a_generated_file_that_was_edited_by_hand_stops_matching_its_row() {
    let home = Home::new();
    let _daemon = home.start_daemon();
    home.declare(&[mixengine_testkit::Service::new("fakeservice@main")]);

    assert!(
        !stale(&home),
        "a home whose configuration was just rendered reported drift: {}",
        stdout(&home.mix(&["doctor"]))
    );

    let rendered = home
        .path()
        .join("etc")
        .join("fakeservice@main")
        .join("fakeservice.args");

    std::fs::write(&rendered, "tampered\n").expect("the generated file is writable");

    assert!(
        stale(&home),
        "editing a generated file by hand was not noticed: {}",
        stdout(&home.mix(&["doctor"]))
    );
}

/// The unprivileged half of a repair, end to end: what was edited by hand is put back.
///
/// **The control is the middle assertion, taken with the same instrument a moment earlier.** A test
/// that only looked afterwards would pass on a build whose check never fires.
///
/// **The exit code is deliberately not asserted.** `--repair` succeeds only when nothing was left
/// `Untouched`, and three conditions always are when they are present — the GitHub Windows runner
/// reserves a port range that holds 80, which is a fact about the machine and not about this repair.
/// What is asserted is the condition this test is about, which is the only thing it can honestly
/// claim.
#[test]
#[cfg_attr(
    not(debug_assertions),
    ignore = "the fakeservice recipe is compiled into debug builds only"
)]
fn a_generated_file_that_was_edited_by_hand_is_put_back() {
    let home = Home::new();
    let _daemon = home.start_daemon();
    home.declare(&[mixengine_testkit::Service::new("fakeservice@main")]);

    let rendered = home
        .path()
        .join("etc")
        .join("fakeservice@main")
        .join("fakeservice.args");

    std::fs::write(&rendered, "tampered\n").expect("the generated file is writable");

    assert!(
        stale(&home),
        "editing a generated file by hand was not noticed"
    );

    let repaired = stdout(&home.mix(&["doctor", "--repair"]));
    assert!(
        repaired.contains("repaired") || repaired.contains("generated"),
        "the repair said nothing about the configuration: {repaired}"
    );

    assert!(
        !stale(&home),
        "the generated file was not put back: {}",
        stdout(&home.mix(&["doctor"]))
    );
}

/// **`--repair` on its own never raises a prompt**, which is T64's rule and the reason the daemon
/// takes a `grant` flag rather than flushing the queue itself.
///
/// A home with no front end and no wired resolver has nothing queued, so what this proves is the
/// half that holds on every machine: the command completes without waiting on anybody, and says what
/// it did.
#[test]
fn a_repair_that_was_not_told_to_grant_does_not_wait_for_anybody() {
    let home = Home::new();
    let _daemon = home.start_daemon();

    let printed = home.mix(&["doctor", "--repair", "--json"]);
    let report: serde_json::Value = serde_json::from_slice(&printed.stdout)
        .unwrap_or_else(|error| panic!("{error}: {}", stdout(&printed)));

    assert!(
        report["actions"].is_array(),
        "a repair answers a list of actions: {report}"
    );
    assert_eq!(
        report["granting"],
        serde_json::Value::Null,
        "a repair nobody asked to grant raised a prompt: {report}"
    );
}

/// Does `mix doctor` report `id` on this home?
///
/// **The one condition, ignoring every other**, so a machine that has something else wrong with it —
/// a reserved port range, a hosts block somebody edited — does not turn this suite red for a reason
/// it is not about.
///
/// Parsed rather than taken through `harness::json`: that helper asserts a zero exit, and
/// `mix doctor` deliberately has none when it found something. Both are right — the helper encodes
/// "a successful `--json` prints JSON", and here the exit code is the report rather than the call.
fn reports(home: &Home, id: &str) -> bool {
    let printed = home.mix(&["doctor", "--json"]);
    let report: serde_json::Value = serde_json::from_slice(&printed.stdout)
        .unwrap_or_else(|error| panic!("{error}: {}", stdout(&printed)));

    report["checks"]
        .as_array()
        .unwrap_or_else(|| panic!("a list of checks: {report}"))
        .iter()
        .any(|check| check["outcome"]["id"] == id)
}

/// A declared name that does not resolve — check 6.
fn unreachable(home: &Home) -> bool {
    reports(home, "domain_unreachable")
}

/// An installed configuration that is not what its row renders to — check 10.
fn stale(home: &Home) -> bool {
    reports(home, "generated_config_stale")
}
