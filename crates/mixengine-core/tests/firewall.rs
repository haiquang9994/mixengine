//! No firewall rule left behind — roadmap task **T76**, the second of two enforcement tests.
//!
//! `.claude/features/lan-sharing.md` promises that *disabling sharing leaves no firewall rule
//! behind, verified by enumerating rules by label*. This is that verification, and it is the half
//! `crates/mixengine-cli/tests/sharing.rs` explicitly cannot make: that suite asks what is
//! *listening*, from the machine to its own address, which never crosses a firewall at all.
//!
//! **Windows only, and CI's answer rather than a developer's.** Two reasons, and both are properties
//! of the systems rather than of this build. `ufw` has no comment field on a plain allow — see
//! `mixengine_platform`'s `firewall::unix_tools` — so a rule MixEngine wrote on Linux cannot be
//! found again by name, which makes "enumerating by label" a claim about one system. And writing a
//! rule at all needs a full token: CI's Windows third runs these suites under one and `cargo test`
//! on a developer's machine does not, so this skips itself rather than failing where it was never
//! able to run.
//!
//! **`mixengine-elevate` is run directly**, the way `tests/elevation.rs` runs it: going through the
//! daemon would raise a real UAC dialog at whoever is running the suite.

#![cfg(windows)]

use std::path::PathBuf;
use std::process::Command;

use mixengine_core::elevation;
use mixengine_proto::privileged::{FIREWALL_LABEL, FirewallPlan, OpOutcome, PrivilegedOp};
use mixengine_proto::{PendingOp, PendingOpId, Timestamp};

/// The `mixengine-elevate` built alongside this test — `tests/elevation.rs`' helper, verbatim.
fn helper() -> PathBuf {
    let name = format!("mixengine-elevate{}", std::env::consts::EXE_SUFFIX);
    let test = std::env::current_exe().expect("this test binary has a path");
    let directory = test.parent().expect("this test binary is in a directory");

    let beside = directory.join(&name);
    if beside.is_file() {
        return beside;
    }

    let above = directory
        .parent()
        .expect("the deps directory is inside the profile directory")
        .join(&name);

    assert!(
        above.is_file(),
        "{} is not there — this suite runs the real helper, so build it first: \
         `cargo build -p mixengine-elevate`",
        above.display()
    );

    above
}

/// One firewall plan, as the queue holds one.
fn plan(label: &str, ports: Vec<u16>) -> PendingOp {
    let op = PrivilegedOp::FirewallApply {
        plan: FirewallPlan {
            ports,
            label: label.to_owned(),
        },
    };

    PendingOp {
        id: PendingOpId(1),
        description: op.describe(),
        op,
        requested_at: Timestamp(1_760_000_000_000),
    }
}

/// Hand `operation` to the real helper, in the directory layout the daemon uses.
///
/// `run/elevate/<id>/` and not somewhere arbitrary: the helper takes a lock at `<home>/run/` before
/// it applies anything, so a request written outside that tree fails for a reason that has nothing
/// to do with what is under test.
fn applied(home: &std::path::Path, id: &str, operation: PendingOp) -> OpOutcome {
    let directory = home.join("run").join("elevate").join(id);

    let request =
        elevation::write_request(&directory, home, &[operation]).expect("the request is written");

    let status = Command::new(helper())
        .arg(request.path())
        .output()
        .expect("the helper binary runs");

    assert!(
        status.status.success(),
        "the helper refused the request: {}",
        String::from_utf8_lossy(&status.stderr)
    );

    let report = elevation::read_report(&request).expect("a report beside the request");

    report
        .results
        .into_iter()
        .next()
        .expect("one outcome per operation")
}

/// Whether `netsh` finds any rule under this name.
///
/// A name nothing matches exits non-zero with *No rules match the specified criteria*, which is the
/// same reading `mixengine_platform`'s `firewall::netsh::delete` already relies on.
fn rule_exists(label: &str) -> bool {
    std::process::Command::new("netsh")
        .args(["advfirewall", "firewall", "show", "rule"])
        .arg(format!("name={label}"))
        .output()
        .expect("netsh runs")
        .status
        .success()
}

/// Remove the label whatever state the test ended in.
///
/// A test that leaks a firewall rule onto a machine is the defect it was written to catch, so this
/// runs on the failing path as well as the passing one.
struct Cleanup(String);

impl Drop for Cleanup {
    fn drop(&mut self) {
        let _ = std::process::Command::new("netsh")
            .args(["advfirewall", "firewall", "delete", "rule"])
            .arg(format!("name={}", self.0))
            .output();
    }
}

/// **The empty plan is the revoke, and it leaves nothing behind** — T74's D6, asserted rather than
/// asserted *about*, and the promise every automatic revoke in T76 depends on.
#[test]
fn a_plan_that_carries_no_ports_leaves_no_rule_behind() {
    if !mixengine_platform::elevated::is_elevated() {
        eprintln!(
            "skipped: writing a firewall rule needs a full token, which this process does not have"
        );
        return;
    }

    // Under MixEngine's own prefix, because the helper refuses any label outside it — T74's D7 — but
    // named so that a rule this test leaked is obviously a test's and not a real home's.
    let label = format!("{FIREWALL_LABEL}shared sites (T76 test)");
    let _cleanup = Cleanup(label.clone());

    let home = tempfile::tempdir().expect("a temporary home");

    assert!(
        !rule_exists(&label),
        "this test's own label is already on this machine, so nothing it asserts would mean anything"
    );

    let opened = applied(home.path(), "open", plan(&label, vec![18080, 18443]));
    assert!(
        matches!(opened, OpOutcome::Applied { .. }),
        "the ports were not opened: {opened:?}"
    );
    assert!(
        rule_exists(&label),
        "the rule was asked for and is not on the machine"
    );

    // The revoke: the same whole-state operation carrying nothing, which is exactly what
    // `site.unshare` of the last shared site enqueues — and what every automatic revoke enqueues.
    let revoked = applied(home.path(), "revoke", plan(&label, Vec::new()));
    assert!(
        matches!(revoked, OpOutcome::Applied { .. }),
        "the revoke did not apply: {revoked:?}"
    );

    assert!(
        !rule_exists(&label),
        "an unshare left a firewall rule behind, which is the one thing this feature promises it \
         never does"
    );
}
