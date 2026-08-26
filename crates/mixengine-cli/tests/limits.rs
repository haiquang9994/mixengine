//! `mix service limits` against a real daemon.
//!
//! Roadmap task **T68**, and the half the daemon's own suite cannot reach: what a *person* is shown.
//! That `service.set_limits` replaces the whole value rather than merging is asserted over the wire
//! in `crates/mixengine-daemon/tests/limits.rs`; what is asserted here is the thing that makes that
//! rule liveable — **the cleared field is on the screen**. A client that took a patch would need no
//! such printing, and a client that merged would be holding business logic it may not hold. This
//! suite is the receipt for the choice between them.
//!
//! **The services are `fakeservice` rows**, so these are ignored in a release build for
//! `tests/service.rs`'s reason: that recipe is compiled into debug builds only.

mod harness;

use harness::{Home, json, stdout};
use mixengine_testkit::Service;

/// The service every test here caps.
const SERVICE: &str = "fakeservice@main";

/// A home with a daemon in it, declaring one service.
fn running() -> (Home, harness::Daemon) {
    let home = Home::new();
    let daemon = home.start_daemon();

    home.declare(&[Service::new(SERVICE)]);

    (home, daemon)
}

/// An uncapped service reads as uncapped, on every field, with a verdict beside each.
#[test]
#[cfg_attr(
    not(debug_assertions),
    ignore = "the fakeservice recipe is compiled into debug builds only"
)]
fn reading_limits_prints_every_field_and_what_this_machine_does_with_it() {
    let (home, _daemon) = running();

    let printed = stdout(&home.mix(&["service", "limits", SERVICE]));

    assert!(printed.contains("cpu"), "{printed}");
    assert!(printed.contains("memory"), "{printed}");
    assert!(printed.contains("priority"), "{printed}");
    assert!(printed.contains("uncapped"), "{printed}");

    // A number with no verdict beside it is exactly the ambiguity this rendering exists to remove.
    assert!(
        printed.contains("enforced"),
        "every field says what this machine does with it: {printed}",
    );

    // And the unit `cpu_percent` is in, because "50" is meaningless without it.
    assert!(printed.contains("one core"), "{printed}");
}

/// **The receipt for D8.** `set --cpu` alone clears a memory ceiling, and the clearing is visible.
///
/// A person who did not mean it finds out on the same screen, in the same breath as the change they
/// did mean — which is the whole reason `service.set_limits` is allowed to take the whole value.
#[test]
#[cfg_attr(
    not(debug_assertions),
    ignore = "the fakeservice recipe is compiled into debug builds only"
)]
fn set_prints_every_field_so_a_cleared_limit_is_visible() {
    let (home, _daemon) = running();

    let first = stdout(&home.mix(&["service", "limits", SERVICE, "set", "--memory", "512"]));
    assert!(first.contains("512 MB"), "{first}");

    let second = stdout(&home.mix(&["service", "limits", SERVICE, "set", "--cpu", "50"]));

    assert!(second.contains("50% of one core"), "{second}");
    assert!(
        second.contains("uncapped"),
        "the memory ceiling that was just cleared is on the screen: {second}",
    );
}

/// What was set survives, which is what makes `set` a setting rather than an answer.
#[test]
#[cfg_attr(
    not(debug_assertions),
    ignore = "the fakeservice recipe is compiled into debug builds only"
)]
fn what_was_set_is_what_is_read_back() {
    let (home, _daemon) = running();

    home.mix(&[
        "service",
        "limits",
        SERVICE,
        "set",
        "--cpu",
        "25",
        "--memory",
        "256",
        "--priority",
        "background",
    ]);

    let report = json(&home.mix(&["service", "limits", SERVICE, "--json"]));

    assert_eq!(report["limits"]["cpu_percent"], 25, "{report}");
    assert_eq!(report["limits"]["memory_mb"], 256, "{report}");
    assert_eq!(report["limits"]["priority"], "background", "{report}");
}

/// `clear` is a named operation rather than a `set` with three absent flags.
#[test]
#[cfg_attr(
    not(debug_assertions),
    ignore = "the fakeservice recipe is compiled into debug builds only"
)]
fn clear_removes_every_limit() {
    let (home, _daemon) = running();

    home.mix(&[
        "service", "limits", SERVICE, "set", "--cpu", "50", "--memory", "512",
    ]);
    home.mix(&["service", "limits", SERVICE, "clear"]);

    let report = json(&home.mix(&["service", "limits", SERVICE, "--json"]));

    assert_eq!(
        report["limits"]["cpu_percent"],
        serde_json::Value::Null,
        "{report}"
    );
    assert_eq!(
        report["limits"]["memory_mb"],
        serde_json::Value::Null,
        "{report}"
    );
    assert_eq!(report["limits"]["priority"], "normal", "{report}");
}

/// The machine's own answer travels with the number, in both renderings.
///
/// **What it says is not asserted**, only that it is there: a runner with a delegated cgroup and one
/// without give different and equally correct answers, and a test that pinned one would be asserting
/// the runner rather than MixEngine.
#[test]
#[cfg_attr(
    not(debug_assertions),
    ignore = "the fakeservice recipe is compiled into debug builds only"
)]
fn the_machines_own_support_travels_with_the_limits() {
    let (home, _daemon) = running();

    let report = json(&home.mix(&["service", "limits", SERVICE, "--json"]));

    assert!(
        report["support"]["cores"].as_u64().unwrap_or(0) >= 1,
        "{report}"
    );
    assert!(report["support"]["cpu"]["kind"].is_string(), "{report}");
    assert!(report["support"]["memory"]["kind"].is_string(), "{report}");
    assert!(report["support"]["memory_measure"].is_string(), "{report}");
}

/// A limit this machine will not enforce is still stored, and the report says both things.
///
/// macOS's case, run everywhere: what is asserted is that storing and enforcing are reported
/// separately, not which of them this runner does.
#[test]
#[cfg_attr(
    not(debug_assertions),
    ignore = "the fakeservice recipe is compiled into debug builds only"
)]
fn a_limit_is_stored_and_its_enforcement_is_reported_separately() {
    let (home, _daemon) = running();

    home.mix(&["service", "limits", SERVICE, "set", "--memory", "512"]);

    let report = json(&home.mix(&["service", "limits", SERVICE, "--json"]));

    assert_eq!(report["limits"]["memory_mb"], 512, "stored: {report}");
    assert!(
        report["support"]["memory"]["kind"].is_string(),
        "and separately answered for: {report}",
    );
}
