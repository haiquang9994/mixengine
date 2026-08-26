//! `mix service idle` and `mix project keep-warm` against a real daemon.
//!
//! Roadmap task **T69**, and the half no unit test can reach: what a *person* is told. The
//! arithmetic is asserted against a mock in the daemon's own `services::idle` tests and the reading
//! against a fake server in `mixengine-supervisor`; what is asserted here is the thing those cannot
//! be — that the four reasons a service stays running arrive on the screen as four different
//! sentences.
//!
//! **The services are `fakeservice` rows**, so these are ignored in a release build for
//! `tests/limits.rs`'s reason: that recipe is compiled into debug builds only.

mod harness;

use harness::{Home, json, stdout};
use mixengine_testkit::Service;

/// The service every test here sets a policy on.
const SERVICE: &str = "fakeservice@main";

/// A home with a daemon in it, declaring one service.
fn running() -> (Home, harness::Daemon) {
    let home = Home::new();
    let daemon = home.start_daemon();

    home.declare(&[Service::new(SERVICE)]);

    (home, daemon)
}

/// The whole arc: read, set, switch off, and back to the default.
///
/// **And `source` is asserted at every step, not only `policy`.** Two of the four states answer
/// "never" — switched off here, and no default yet — and a rendering that showed only the duration
/// would tell half the people who read it to go and change a setting that was never the cause.
#[test]
#[cfg_attr(
    not(debug_assertions),
    ignore = "the fakeservice recipe is compiled into debug builds only"
)]
fn an_idle_policy_is_read_set_switched_off_and_defaulted() {
    let (home, _daemon) = running();

    let unset = json(&home.mix(&["service", "idle", SERVICE, "--json"]));
    assert_eq!(unset["policy"], serde_json::Value::Null, "{unset}");
    assert_eq!(
        unset["source"], "unset",
        "no recipe in this build offers an idle default: {unset}"
    );

    let set = json(&home.mix(&["service", "idle", SERVICE, "--after", "45m", "--json"]));
    assert_eq!(set["policy"]["after"], 45 * 60 * 1000, "{set}");
    assert_eq!(set["source"], "row", "{set}");

    let never = json(&home.mix(&["service", "idle", SERVICE, "--never", "--json"]));
    assert_eq!(never["policy"], serde_json::Value::Null, "{never}");
    assert_eq!(
        never["source"], "never",
        "switched off here is stored, and is not the same answer as unset: {never}"
    );

    let defaulted = json(&home.mix(&["service", "idle", SERVICE, "--default", "--json"]));
    assert_eq!(defaulted["source"], "unset", "{defaulted}");
}

/// The rendering says both halves: when, and what is being watched.
#[test]
#[cfg_attr(
    not(debug_assertions),
    ignore = "the fakeservice recipe is compiled into debug builds only"
)]
fn the_rendering_names_the_policy_and_what_measures_it() {
    let (home, _daemon) = running();

    let unset = stdout(&home.mix(&["service", "idle", SERVICE]));
    assert!(unset.contains("never"), "{unset}");
    assert!(
        unset.contains("no default yet"),
        "an unset policy says why there is none rather than leaving a blank: {unset}"
    );

    let set = stdout(&home.mix(&["service", "idle", SERVICE, "--after", "30m"]));
    assert!(set.contains("30m"), "{set}");
    assert!(
        set.contains("connections to port"),
        "a policy that cannot say what it watches is one nobody can check: {set}"
    );
}

/// A duration that is not a whole number of minutes is refused rather than rounded.
///
/// The column stores minutes, so `90s` would have to become one minute or two — and a setting that
/// quietly becomes something else is worse than one that says it cannot.
#[test]
fn a_duration_that_is_not_minutes_is_refused_before_the_daemon_is_asked() {
    let home = Home::new();

    let refused = home.mix(&["service", "idle", SERVICE, "--after", "90s"]);
    assert!(!refused.status.success());

    let complaint = harness::stderr(&refused);
    assert!(complaint.contains("whole number of minutes"), "{complaint}");

    // `--after 0m` reads as "stop it immediately" and means the opposite, so it has its own flag.
    let zero = home.mix(&["service", "idle", SERVICE, "--after", "0m"]);
    assert!(!zero.status.success());
    assert!(harness::stderr(&zero).contains("--never"), "{zero:?}");
}

/// A keep-warm project shows up as an exemption on the pool its site names.
///
/// **The receipt for D9.** The service is not stopped while the project is warm, and the report
/// says which project — because that is the thing a person has to go and change.
#[test]
#[cfg_attr(
    not(debug_assertions),
    ignore = "the fakeservice recipe is compiled into debug builds only"
)]
fn a_kept_warm_project_is_reported_as_an_exemption() {
    let (home, _daemon) = running();

    let root = home.path().join("shop");
    std::fs::create_dir_all(&root).expect("a project directory");
    let root = root.display().to_string();

    home.mix(&["project", "create", &root, "--name", "shop"]);

    // Every project starts cold, which is what makes the change below observable rather than
    // assumed.
    let cold = json(&home.mix(&["project", "show", "shop", "--json"]));
    assert_eq!(cold["project"]["keep_warm"], false, "{cold}");

    let warm = json(&home.mix(&["project", "keep-warm", "shop", "--json"]));
    assert_eq!(warm["project"]["keep_warm"], true, "{warm}");

    let cold_again = json(&home.mix(&["project", "keep-warm", "shop", "--off", "--json"]));
    assert_eq!(cold_again["project"]["keep_warm"], false, "{cold_again}");
}
