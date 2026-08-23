//! `mix` against a real `mixengined`, over a real endpoint.
//!
//! This is the test the whole of task T10 exists to pass, and none of it can be written any other
//! way: that the client and the daemon agree on which home they are talking about, that they agree
//! on the endpoint that home implies, and that a client which finds nothing there produces a daemon
//! rather than an error message. The pieces are mocked nowhere, because every one of those claims is
//! about two operating-system processes.
//!
//! It is also the guard on the one thing `mix` duplicates rather than shares. The daemon computes
//! `run/` through `mixengine_core::Paths` and the client computes it in `home.rs`, because `core`
//! carries `sqlx` — see the note there. Nothing but a run like this one would notice the two drifting
//! apart: a `mix` that looked in the wrong place would simply autostart a second daemon, forever.
//!
//! Every test gets its own `MIXENGINE_HOME` in a `TempDir` **passed as `--home`** — rule 2 in
//! `.claude/standards/testing.md`. Nothing here touches the network; a Unix socket and a named pipe
//! are neither.

mod harness;

use std::path::Path;
use std::process::Command;

use harness::{Home, json};
use serde_json::Value;

#[test]
fn status_starts_a_daemon_for_a_home_that_has_none_and_then_describes_it() {
    let home = Home::new();

    // Milestone M0, in one line: nothing is running, nothing has been created, and a person types
    // `mix status`.
    let status = json(&home.mix(&["status", "--json"]));
    let daemon = &status["daemon"];

    // Stopped by `Home::drop`, which asks the endpoint who is answering rather than being told
    // here: an assertion below that fails would otherwise leave a daemon serving a home that is
    // about to be deleted.
    assert!(daemon["pid"].as_u64().is_some(), "{status}");

    assert_eq!(daemon["protocol"], 1);
    assert_eq!(status["client"]["protocol"], 1);
    assert_eq!(daemon["version"], status["client"]["version"]);

    // The two halves of the claim this file exists for. The daemon resolved its home and its
    // endpoint through `mixengine_core::Paths`; the client resolved both on its own and reached it.
    assert!(
        Path::new(daemon["home"].as_str().expect("a home")).ends_with(
            home.path()
                .file_name()
                .expect("the temporary home has a name")
        ),
        "the daemon is serving {} and the client asked about {}",
        daemon["home"],
        home.path().display()
    );
    assert_eq!(daemon["endpoint"], home.endpoint());
}

#[test]
fn status_talks_to_the_daemon_that_is_already_there_instead_of_starting_another() {
    let home = Home::new();
    let daemon = home.start_daemon();

    let status = json(&home.mix(&["status", "--json"]));

    // The single-instance lock would have caught a second daemon, but only after the fact and only
    // in a log nobody reads. What is asserted is the thing a user would notice: the answer came from
    // the process that was already running.
    assert_eq!(status["daemon"]["pid"], daemon.pid());
}

#[test]
fn the_human_rendering_leads_with_the_state_and_the_home() {
    let home = Home::new();
    let _daemon = home.start_daemon();

    let output = home.mix(&["status"]);
    let rendered = String::from_utf8_lossy(&output.stdout);

    assert!(output.status.success(), "{output:?}");
    assert!(rendered.starts_with("mixengined "), "{rendered}");
    assert!(rendered.contains("running (pid "), "{rendered}");
    assert!(
        rendered.contains(&home.endpoint()),
        "the endpoint is what tells one daemon from another: {rendered}"
    );
    // **Roadmap task T44**, end to end: a real daemon, a real bind, and the one line that says how
    // this home resolves a name. `hosts file` and not `DNS`, on every machine until T45 wires a
    // resolver — a server nothing routes a name to resolves exactly as many names as no server.
    assert!(
        rendered.contains("names     hosts file"),
        "a client is told which mechanism this home is on: {rendered}"
    );
}

#[test]
fn no_autostart_answers_the_question_without_creating_anything() {
    let home = Home::new();
    let output = home.mix(&["status", "--no-autostart"]);

    // A non-zero exit, because a caller that asked about a daemon and found none needs to be able
    // to tell without parsing anything. The machine-readable half of the same answer is the code
    // below.
    assert!(!output.status.success());
    assert!(output.stdout.is_empty(), "{output:?}");

    let complaint = String::from_utf8_lossy(&output.stderr);
    assert!(
        complaint.contains("no MixEngine daemon is listening"),
        "{complaint}"
    );
    assert!(complaint.contains("--no-autostart"), "{complaint}");

    // The property the flag exists for: a monitoring check that asks whether MixEngine is running
    // must not be the thing that installs it. "Nothing" is what the fixture seeded and no more.
    assert_eq!(
        home.contents(),
        mixengine_testkit::Home::SEEDED,
        "asking about a daemon created something in {}",
        home.path().display()
    );
}

#[test]
fn a_failure_is_the_same_wire_error_a_daemon_would_have_sent() {
    let home = Home::new();
    let output = home.mix(&["status", "--json", "--no-autostart"]);

    assert!(!output.status.success());

    // On stderr, and not on stdout: a script redirecting stdout into a file gets a status object or
    // an empty file, never an error object where a status was meant to be.
    assert!(output.stdout.is_empty(), "{output:?}");

    let error: Value = serde_json::from_slice(&output.stderr).unwrap_or_else(|failure| {
        panic!(
            "mix --json fails in JSON too: {failure}\n{}",
            String::from_utf8_lossy(&output.stderr)
        )
    });

    // The whole point of carrying the wire error through: this code is the same one a daemon would
    // have sent for the same situation, so nothing branches on which side produced it.
    assert_eq!(error["code"], "precondition_failed");
    assert!(error["hint"].is_string(), "{error}");
}

#[test]
fn an_empty_home_never_gets_as_far_as_meaning_the_real_one() {
    let output = Command::new(env!("CARGO_BIN_EXE_mix"))
        .args(["status", "--home", ""])
        .output()
        .expect("the mix binary runs");

    // `clap` gets there first and refuses the value outright, with its own usage exit code rather
    // than ours — checked here because that is the claim `core` and `home.rs` both rest on, and
    // neither of them can see it. The guard behind it stays: `home.rs` is reachable from a test and
    // from whatever a later command does with a home, and treating an empty override as "not given"
    // would point a sandbox run at the real install.
    assert_eq!(output.status.code(), Some(2), "{output:?}");
    assert!(output.stdout.is_empty(), "{output:?}");

    let complaint = String::from_utf8_lossy(&output.stderr);
    assert!(complaint.contains("--home"), "{complaint}");
}
