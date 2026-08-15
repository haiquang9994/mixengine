//! `mix daemon stop` against a real daemon, with a real service under it.
//!
//! Task **T9a**, and the reason it is an end-to-end test rather than a unit one: everything below
//! the client is proved where it lives — the handler stops the services before it cancels anything,
//! the budget bounds the sum — and what none of that proves is that a person typing three words is
//! left with no daemon. That claim is about a process ending, so nothing here is mocked and the
//! assertion is `Daemon::wait_until_gone`.
//!
//! **The ones that declare a service are ignored in a release build**, for the reason `service.rs`
//! gives at length: the only recipe a `fakeservice` row can be rendered by is compiled into debug
//! builds alone, so a release run would assert against a home that declares nothing and would pass
//! or fail for reasons that have nothing to do with `mix daemon stop`. That covers the two about a
//! shutdown nobody could order as well — an undeclarable source needs a service to be undeclarable,
//! and this build can declare one only in debug.
//! What holds in a release run is the layer below: `api/rpc.rs` drives the same case through
//! `fixture::Unavailable`, and `render.rs` proves the sentence it produces.

mod harness;

use harness::{Home, json, stdout};
use mixengine_testkit::Service;

/// Break the one service this home declares, the way a person editing overrides breaks it.
///
/// A setting no recipe has is **refused** rather than ignored, so the daemon can no longer say what
/// this home declares — which is the case T9a decided a daemon still stops for, and the one where an
/// empty walk is a lie rather than merely terse. What makes this work at all is that the generator
/// reads the row on every call instead of at startup.
fn misconfigured(home: &Home, id: &str) {
    mixengine_testkit::declare::reconfigure_blocking(
        &home.database_file(),
        id,
        r#"{"this_is_not_a_setting": 1}"#,
    );
}

/// The JSON `mix --json` printed, whatever it exited with.
///
/// `harness::json` insists on a zero exit, which is right for every other caller and wrong for the
/// one below: a shutdown that could not be ordered exits non-zero on purpose, and the object on
/// stdout is the whole of what says why.
fn answered(output: &std::process::Output) -> serde_json::Value {
    serde_json::from_slice(&output.stdout).unwrap_or_else(|error| {
        panic!(
            "mix --json prints JSON on stdout whether or not it succeeded: {error}\n{}",
            stdout(output)
        )
    })
}

#[test]
#[cfg_attr(
    not(debug_assertions),
    ignore = "the fakeservice recipe is compiled into debug builds only"
)]
fn stopping_the_daemon_stops_what_it_was_running_and_then_itself() {
    let home = Home::new();
    let mut daemon = home.start_daemon();
    home.declare(&[Service::new("mariadb@main")]);

    let started = json(&home.mix(&["service", "start", "mariadb@main", "--json"]));
    assert_eq!(started["reached"][0], "mariadb@main", "{started}");

    let stopped = home.mix(&["daemon", "stop", "--json"]);
    assert!(stopped.status.success(), "{}", stdout(&stopped));

    // **The service, in the answer, before the daemon has gone.** A client told only that its
    // request was accepted would have to work out from the event stream whether the database it
    // cares about was stopped or killed — which is the business-logic-in-a-client bug CLAUDE.md
    // forbids, and the reason this method waits for the walk rather than answering the moment it
    // has a plan.
    let answer = json(&stopped);
    assert_eq!(answer["services"]["complete"], true, "{answer}");
    assert_eq!(
        answer["services"]["reached"],
        serde_json::json!(["mariadb@main"]),
        "{answer}"
    );
    assert!(answer["services"].get("failed").is_none(), "{answer}");
    // And nothing about an order that was not kept, because it was: a note printed on an ordinary
    // shutdown is a note nobody would read on the one where it matters.
    assert!(answer.get("unordered").is_none(), "{answer}");

    // And then the daemon itself, which is the half no answer can carry: it is still running at the
    // moment it says all this, and the connection closing afterwards *is* the shutdown.
    assert!(
        daemon.wait_until_gone(),
        "the daemon answered `daemon.shutdown` and stayed up\n--- daemon.log ---\n{}",
        home.daemon_log()
    );

    // Nothing answers for this home any more, asked the way a monitoring check would.
    let after = home.mix(&["status", "--no-autostart"]);
    assert!(
        !after.status.success(),
        "something is still listening on {}",
        home.endpoint()
    );
}

#[test]
fn stopping_a_daemon_that_is_running_reads_as_one_sentence_about_the_daemon() {
    // No services at all, which is the ordinary state of a home until Phase 3: the rendering has to
    // say what happened to the *daemon* rather than print "nothing to stop: this home declares no
    // services", which is `mix service stop`'s sentence and answers a question nobody asked here.
    let home = Home::new();
    let mut daemon = home.start_daemon();

    let stopped = home.mix(&["daemon", "stop"]);

    assert!(stopped.status.success(), "{}", stdout(&stopped));
    assert_eq!(stdout(&stopped), "mixengined is stopping\n");
    assert!(
        daemon.wait_until_gone(),
        "--- daemon.log ---\n{}",
        home.daemon_log()
    );
}

/// **The shutdown T9a decided still happens, and the half of it that used to go only to
/// `daemon.log`.**
///
/// The daemon cannot say what it declares, so there is no order to stop anything in; it stops all
/// the same, because refusing over an override somebody mistyped would leave them a daemon they can
/// only kill. What the client then has to be handed is the difference between that and a home with
/// nothing to stop — the two produce the same empty, complete walk — because it is the difference
/// between a database that went after the sites talking to it and one that went at the same moment
/// they did.
#[test]
#[cfg_attr(
    not(debug_assertions),
    ignore = "the fakeservice recipe is compiled into debug builds only"
)]
fn a_shutdown_nobody_could_order_says_so_in_json_rather_than_reading_as_a_quiet_one() {
    let home = Home::new();
    let mut daemon = home.start_daemon();
    home.declare(&[Service::new("mariadb@main")]);

    misconfigured(&home, "mariadb@main");

    let stopped = home.mix(&["daemon", "stop", "--json"]);
    let answer = answered(&stopped);

    assert_eq!(
        answer["services"]["planned"],
        serde_json::json!([]),
        "no order could be worked out, so nothing was planned: {answer}"
    );
    // `invalid_argument` and not `internal`: since T30 the daemon knows the difference between a
    // generator that failed for its own reasons and a setting a person misspelled, and it is the
    // second here.
    assert_eq!(answer["unordered"]["code"], "invalid_argument", "{answer}");
    assert!(
        answer["unordered"]["message"]
            .as_str()
            .is_some_and(|message| message.contains("mariadb@main")),
        "the daemon's own sentence names the service to fix, and it survives `--json`: {answer}"
    );

    // Non-zero, on the same reading `services.failed` gets: the daemon did stop, and the stop that
    // was asked for — every service, in dependency order — is not the stop that happened.
    assert!(
        !stopped.status.success(),
        "a shutdown that skipped the ordered walk exited 0: {answer}"
    );

    assert!(
        daemon.wait_until_gone(),
        "the daemon reported a skipped order and stayed up\n--- daemon.log ---\n{}",
        home.daemon_log()
    );
}

#[test]
#[cfg_attr(
    not(debug_assertions),
    ignore = "the fakeservice recipe is compiled into debug builds only"
)]
fn a_shutdown_nobody_could_order_tells_a_person_what_happened_to_their_services() {
    let home = Home::new();
    let mut daemon = home.start_daemon();
    home.declare(&[Service::new("mariadb@main")]);

    misconfigured(&home, "mariadb@main");

    let stopped = home.mix(&["daemon", "stop"]);
    let printed = stdout(&stopped);

    // The daemon going is still the headline; what it did to the services is the correction under
    // it, and without the second line this output is the one a quiet home produces.
    assert!(printed.starts_with("mixengined is stopping\n"), "{printed}");
    assert!(
        printed.contains("were not stopped in dependency order"),
        "{printed}"
    );
    assert!(
        printed.contains("mariadb@main"),
        "the service to fix is named rather than left in daemon.log: {printed}"
    );
    assert!(!stopped.status.success(), "{printed}");

    assert!(
        daemon.wait_until_gone(),
        "--- daemon.log ---\n{}",
        home.daemon_log()
    );
}

#[test]
fn stopping_a_daemon_that_is_not_running_does_not_start_one_to_stop_it() {
    // The one command that refuses to autostart whatever the flags say. Starting a daemon in order
    // to ask it to stop leaves the machine exactly as it was found, one process later — and on a
    // home that has never been used it would also create the whole directory tree as a side effect
    // of asking for less.
    let home = Home::new();

    let stopped = home.mix(&["daemon", "stop", "--json"]);

    assert!(!stopped.status.success(), "{}", stdout(&stopped));

    let error: serde_json::Value =
        serde_json::from_slice(&stopped.stderr).unwrap_or_else(|failure| {
            panic!(
                "a failure is the wire error on stderr in both renderings: {failure}\n{}",
                String::from_utf8_lossy(&stopped.stderr)
            )
        });
    assert_eq!(error["code"], "precondition_failed", "{error}");

    // And it left the home the way it found it: nothing listening, nothing started.
    assert!(
        !home.mix(&["status", "--no-autostart"]).status.success(),
        "a daemon was started by the command that asks one to stop"
    );
}
