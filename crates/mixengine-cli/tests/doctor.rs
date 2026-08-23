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
        Some(9),
        "{report}"
    );

    let table = stdout(&home.mix(&["doctor"]));

    // The per-system fact ADR 0007 exists to keep honest, on the screen rather than only on the
    // wire.
    assert!(table.contains("descendant"), "{table}");
    assert!(table.lines().count() >= 9, "{table}");
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

/// Does `mix doctor` report `domain_unreachable` on this home?
///
/// **The one condition, ignoring every other**, so a machine that has something else wrong with it —
/// a reserved port range, a hosts block somebody edited — does not turn this suite red for a reason
/// it is not about.
///
/// Parsed rather than taken through `harness::json`: that helper asserts a zero exit, and
/// `mix doctor` deliberately has none when it found something. Both are right — the helper encodes
/// "a successful `--json` prints JSON", and here the exit code is the report rather than the call.
fn unreachable(home: &Home) -> bool {
    let printed = home.mix(&["doctor", "--json"]);
    let report: serde_json::Value = serde_json::from_slice(&printed.stdout)
        .unwrap_or_else(|error| panic!("{error}: {}", stdout(&printed)));

    report["checks"]
        .as_array()
        .unwrap_or_else(|| panic!("a list of checks: {report}"))
        .iter()
        .any(|check| check["outcome"]["id"] == "domain_unreachable")
}
