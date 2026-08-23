//! `mix doctor` against a real daemon.
//!
//! Roadmap task **T47a**'s client half. What the daemon's own `tests/api.rs` proves is that the
//! checks answer; what is proved here is the part only `mix` can be wrong about — that every check
//! reaches the screen, and that the exit code is the *report* rather than the call.
//!
//! **The assertion that matters is that a check can fail.** A suite that only ever sees a healthy
//! machine has proved that the doctor runs, not that it looks — so each `Ok` here is paired, in the
//! same test, with the arrangement that turns it into a `Problem` (T47a design, D10).

mod harness;

use harness::{Home, json, stdout};

/// Every check reaches the screen, whatever it answered.
#[tokio::test(flavor = "multi_thread")]
async fn every_check_is_reported_and_named() {
    let home = Home::new();
    let _daemon = home.start_daemon();

    let report = json(&home.mix(&["doctor", "--json"]));

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

    // **Healthy first.** With no sites there is no domain that could be unreachable, so this home
    // has nothing wrong with it — and that is what makes the failure below an assertion rather than
    // a coincidence.
    let before = home.mix(&["doctor"]);
    assert!(
        before.status.success(),
        "a home with nothing in it has nothing wrong with it: {}",
        stdout(&before)
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

    let after = home.mix(&["doctor"]);

    assert!(!after.status.success(), "{}", stdout(&after));
    assert!(stdout(&after).contains("PROBLEM"), "{}", stdout(&after));

    // **Parsed here rather than through `harness::json`**, which asserts a zero exit — and a
    // `mix doctor` that found something deliberately does not have one. The two are both right: the
    // helper encodes "a successful `--json` prints JSON", and this command's exit code is the
    // report rather than the call.
    let printed = home.mix(&["doctor", "--json"]);
    let report: serde_json::Value = serde_json::from_slice(&printed.stdout)
        .unwrap_or_else(|error| panic!("{error}: {}", stdout(&printed)));

    assert!(
        report["checks"]
            .as_array()
            .expect("a list")
            .iter()
            .any(|check| check["outcome"]["id"] == "domain_unreachable"),
        "{report}"
    );
}
