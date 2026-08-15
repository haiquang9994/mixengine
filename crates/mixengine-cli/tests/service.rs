//! `mix service` against a real daemon supervising a real process.
//!
//! Task **T19b**, and the reason it is an end-to-end test rather than a unit one: everything below
//! the client has been proved in its own crate — the registry walks a plan, the handlers answer
//! `service.*` — and what has never been proved is that a person typing four words gets a process
//! started and an exit code that means it. That claim spans two operating-system processes and a
//! socket, so nothing here is mocked.
//!
//! **The services are `fakeservice` rows**, written by `mixengine_testkit::declare` — which is
//! Phase 3's `service.create` in the same sense — and rendered by the daemon's own generator (T30)
//! through the fixture recipe in `crates/mixengine-daemon/src/services/fakeservice.rs`. So what a
//! test below writes is a *declaration*, exactly as a user's would be, and the whole path from a row
//! to a running process is the one under test.
//!
//! **The five that declare a service are ignored in a release build**, because that recipe is
//! compiled into debug builds only: a release `mixengined` has nothing that can run a `fakeservice`,
//! so the home declares something it cannot start and those tests would fail for a reason that has
//! nothing to do with `mix service`. `ignore` rather than `#[cfg]`, so `cargo test --release` says
//! why they did not run.

mod harness;

use harness::{Home, json, stdout};
use mixengine_testkit::Service;
use serde_json::Value;

/// A home with a daemon in it, declaring `services`.
///
/// The order is forced: the migrations that create the schema run when the daemon opens the home, so
/// there is nothing to insert a row into until it is up.
fn running(services: &[Service]) -> (Home, harness::Daemon) {
    let home = Home::new();
    let daemon = home.start_daemon();

    home.declare(services);

    (home, daemon)
}

/// **What T30 added, from the end a person is at**: the configuration a service runs on is
/// generated from its row, and changing the row changes what the process does.
///
/// Every layer of that is proved where it lives — the merge, the template, the diff, the atomic
/// install, the spec — and none of those says that the file which reaches the disk is the file the
/// program is actually started with. That claim spans a database write, a daemon walk, a rendering
/// and a process, so it is here.
#[test]
#[cfg_attr(
    not(debug_assertions),
    ignore = "the fakeservice recipe is compiled into debug builds only"
)]
fn changing_an_override_regenerates_the_config_and_the_service_runs_on_it() {
    let (home, _daemon) = running(&[Service::new("mariadb@main")]);

    let started = json(&home.mix(&["service", "start", "mariadb@main", "--json"]));
    assert_eq!(started["complete"], true, "{started}");

    // Rendered by the walk that started it, into the directory the service id names.
    let arguments = home
        .path()
        .join("etc")
        .join("mariadb@main")
        .join("fakeservice.args");
    let rendered = std::fs::read_to_string(&arguments).expect("the generated arguments file");
    assert!(
        !rendered.contains("--exit-after"),
        "a service nobody configured to exit was told to: {rendered}"
    );

    home.mix(&["service", "stop", "mariadb@main"]);

    // The one thing a user edits. Long enough that the start below is an ordinary one — a service
    // that died inside its own ready check would prove the opposite of what this is about.
    mixengine_testkit::declare::reconfigure_blocking(
        &home.database_file(),
        "mariadb@main",
        r#"{"exit_after": 1500, "exit_code": 3}"#,
    );

    let restarted = json(&home.mix(&["service", "start", "mariadb@main", "--json"]));
    assert_eq!(restarted["complete"], true, "{restarted}");

    let rendered = std::fs::read_to_string(&arguments).expect("the regenerated arguments file");
    assert!(rendered.contains("--exit-after"), "{rendered}");

    // And the process really is the one that file describes, which is the whole point: nothing else
    // in this home asked it to stop, so an exit is the generated configuration taking effect.
    let deadline = std::time::Instant::now() + mixengine_testkit::home::STARTUP;
    loop {
        let status = json(&home.mix(&["service", "status", "mariadb@main", "--json"]));
        if status["state"] == "failed" {
            break;
        }

        assert!(
            std::time::Instant::now() < deadline,
            "the service ignored the configuration it was started with: {status}\n\
             --- {} ---\n{rendered}\n--- daemon.log ---\n{}",
            arguments.display(),
            home.daemon_log()
        );
        std::thread::sleep(std::time::Duration::from_millis(50));
    }
}

#[test]
#[cfg_attr(
    not(debug_assertions),
    ignore = "the fakeservice recipe is compiled into debug builds only"
)]
fn a_service_starts_stops_and_says_so_in_both_renderings() {
    let (home, _daemon) = running(&[Service::new("mariadb@main")]);

    // Nothing is running yet, and the listing says which service that is about.
    let listed = json(&home.mix(&["service", "list", "--json"]));
    assert_eq!(listed["services"][0]["id"], "mariadb@main");
    assert_eq!(listed["services"][0]["state"], "stopped");
    assert_eq!(listed["services"][0]["supervised"], false);

    // The whole of T19b in one line: a person types four words and a process is running.
    let started = json(&home.mix(&["service", "start", "mariadb@main", "--json"]));
    assert_eq!(started["complete"], true, "{started}");
    assert_eq!(started["reached"][0], "mariadb@main", "{started}");
    assert!(started.get("failed").is_none(), "{started}");

    let status = json(&home.mix(&["service", "status", "mariadb@main", "--json"]));
    assert_eq!(status["state"], "running", "{status}");
    assert_eq!(status["supervised"], true, "{status}");
    assert!(status["pid"].as_u64().is_some(), "{status}");

    // The human rendering of the same answer, which is the half a person actually reads.
    let rendered = stdout(&home.mix(&["service", "status", "mariadb@main"]));
    assert!(rendered.starts_with("mariadb@main — running"), "{rendered}");
    assert!(rendered.contains("supervised  yes"), "{rendered}");

    let stopped = home.mix(&["service", "stop", "mariadb@main"]);
    assert!(stopped.status.success(), "{}", stdout(&stopped));
    assert_eq!(stdout(&stopped), "stopped mariadb@main\n");

    let after = json(&home.mix(&["service", "status", "mariadb@main", "--json"]));
    assert_eq!(after["state"], "stopped", "{after}");
    assert_eq!(after["supervised"], false, "{after}");
    assert!(after["last_started_at"].as_i64().is_some(), "{after}");

    // The field survives the stop, so the rendering has to be the part that stops calling it the
    // present: `stopped` with `started 4m ago` under it is a contradiction on one screen.
    let rendered = stdout(&home.mix(&["service", "status", "mariadb@main"]));
    assert!(rendered.starts_with("mariadb@main — stopped"), "{rendered}");
    assert!(rendered.contains("last start"), "{rendered}");
}

#[test]
#[cfg_attr(
    not(debug_assertions),
    ignore = "the fakeservice recipe is compiled into debug builds only"
)]
fn starting_one_service_starts_what_it_depends_on_and_says_which() {
    let (home, _daemon) = running(&[
        Service::new("mariadb@main"),
        Service::new("php-fpm@8.3").depends_on("mariadb@main"),
    ]);

    let walk = json(&home.mix(&["service", "start", "php-fpm@8.3", "--json"]));

    // The plan is the transitive set, and it is the only thing that tells a user that starting one
    // service is about to touch two — which is why it is rendered rather than summarised away.
    assert_eq!(
        walk["planned"],
        serde_json::json!(["mariadb@main", "php-fpm@8.3"]),
        "{walk}"
    );
    assert_eq!(
        walk["reached"],
        serde_json::json!(["mariadb@main", "php-fpm@8.3"]),
        "{walk}"
    );

    // And a stop of the dependency takes the dependent with it, in the opposite order.
    let stopped = json(&home.mix(&["service", "stop", "mariadb@main", "--json"]));
    assert_eq!(
        stopped["planned"],
        serde_json::json!(["php-fpm@8.3", "mariadb@main"]),
        "{stopped}"
    );

    let listed = json(&home.mix(&["service", "list", "--json"]));
    for service in listed["services"].as_array().expect("a list") {
        assert_eq!(service["state"], "stopped", "{listed}");
    }
}

#[test]
#[cfg_attr(
    not(debug_assertions),
    ignore = "the fakeservice recipe is compiled into debug builds only"
)]
fn a_service_that_never_becomes_ready_fails_the_command_and_names_the_one_to_fix() {
    let (home, _daemon) = running(&[
        // Two seconds, because this is the one test that waits the timeout out on purpose.
        Service::new("mariadb@main")
            .never_ready()
            .ready_timeout(2_000),
        Service::new("php-fpm@8.3").depends_on("mariadb@main"),
    ]);

    let output = home.mix(&["service", "start"]);

    // **The exit code is the point of waiting.** `mix service start && …` has to stop here, and the
    // only way a client could know better without this is to re-derive the daemon's verdict from the
    // event stream.
    assert!(
        !output.status.success(),
        "a walk that failed exited zero: {}",
        stdout(&output)
    );

    // The answer is still on stdout: a walk that stopped at the first of two services is a described
    // failure, not a lost one.
    let rendered = stdout(&output);
    assert!(
        rendered.starts_with("mariadb@main failed to start — not ready within 2s"),
        "{rendered}"
    );
    assert!(rendered.contains("blocked   php-fpm@8.3"), "{rendered}");

    // The dependent was never spawned, and its row says why the daemon did not try.
    let blocked: Value = json(&home.mix(&["service", "status", "php-fpm@8.3", "--json"]));
    assert_eq!(blocked["state"], "failed", "{blocked}");
    assert_eq!(blocked["pid"], Value::Null, "{blocked}");
}

/// `mix service logs` — roadmap task **T16b**, from the side a person types it.
///
/// **The human rendering is the service's own output and nothing else**, which is what makes
/// `mix service logs caddy | grep …` work the way it does for every other program's log. The `--json`
/// rendering is the frame, because a script filtering on `stream` or ordering by `at` needs what the
/// human one deliberately drops.
#[test]
#[cfg_attr(
    not(debug_assertions),
    ignore = "the fakeservice recipe is compiled into debug builds only"
)]
fn logs_print_what_a_service_printed_and_nothing_of_mixengines() {
    let (home, _daemon) = running(&[Service::new("mariadb@main").log_every(50)]);

    home.mix(&["service", "start", "mariadb@main"]);

    let printed = stdout(&home.mix(&["service", "logs", "mariadb@main", "-n", "200"]));

    assert!(
        printed.contains(mixengine_testkit::service::READY_LINE),
        "the service's own line is there verbatim: {printed:?}"
    );
    assert!(
        !printed.contains("stdout") && !printed.contains('['),
        "nothing of ours is in front of it: {printed:?}"
    );

    // The same lines as frames, one JSON object per line, with the two things the text does not
    // carry: which stream it came from and when it was read.
    let framed = stdout(&home.mix(&["service", "logs", "mariadb@main", "--json"]));
    let first: Value = serde_json::from_str(framed.lines().next().expect("at least one frame"))
        .expect("mix --json prints one frame per line");

    assert_eq!(first["type"], "line", "{first}");
    assert!(first["stream"].is_string(), "{first}");
    assert!(first["at"].as_i64().is_some(), "{first}");

    home.mix(&["service", "stop", "mariadb@main"]);
}

/// A service id nothing declares is the daemon's `not_found`, in the shape every other command
/// fails in — the endpoint's status is the envelope and `mix` never invents a sentence of its own.
#[test]
fn logs_for_a_service_nothing_declares_fail_the_way_every_other_command_does() {
    let home = Home::new();
    let _daemon = home.start_daemon();

    let missing = home.mix(&["service", "logs", "mariadb@main", "--json"]);

    assert!(!missing.status.success());

    let error: Value =
        serde_json::from_slice(&missing.stderr).expect("mix --json fails in JSON too");
    assert_eq!(error["code"], "not_found", "{error}");
}

#[test]
fn a_home_that_declares_nothing_says_so_rather_than_printing_nothing() {
    let home = Home::new();
    let _daemon = home.start_daemon();

    let listed = home.mix(&["service", "list"]);
    assert!(listed.status.success(), "{listed:?}");
    assert_eq!(stdout(&listed), "no services are declared in this home\n");

    // A `list` of nothing is a fact; a `status` of a service that is not there is a mistake, and the
    // two are answered differently on purpose.
    let missing = home.mix(&["service", "status", "mariadb@main", "--json"]);
    assert!(!missing.status.success());
    let error: Value =
        serde_json::from_slice(&missing.stderr).expect("mix --json fails in JSON too");
    assert_eq!(error["code"], "not_found", "{error}");
}

#[test]
fn a_service_id_that_cannot_exist_is_refused_before_a_daemon_is_started() {
    let home = Home::new();

    let output = home.mix(&["service", "status", "not a service id"]);

    // clap's own usage exit code, not ours: the value never became a `ServiceId`, so no call was
    // made — which is the point. Nothing was created in the home either.
    assert_eq!(output.status.code(), Some(2), "{output:?}");
    assert_eq!(
        std::fs::read_dir(home.path())
            .expect("the temporary home is readable")
            .count(),
        0,
        "a typo created something in {}",
        home.path().display()
    );
}

#[test]
#[cfg_attr(
    not(debug_assertions),
    ignore = "the fakeservice recipe is compiled into debug builds only"
)]
fn a_walk_nobody_waits_for_is_reported_as_accepted_rather_than_as_finished() {
    let (home, _daemon) = running(&[Service::new("mariadb@main").ready_after(1_000)]);

    let rendered = stdout(&home.mix(&["service", "start", "--no-wait"]));
    assert_eq!(
        rendered,
        "accepted — mixengined is starting mariadb@main in the background\n"
    );

    // And the walk really is going on behind that answer, which is the difference between this and a
    // command that did nothing. Waiting for the service to reach `running` is the only proof of that
    // worth having: everything cheaper — a directory, a file, a row — is something the daemon had
    // already produced at startup, and would pass just as happily against an answer that was a lie.
    let deadline = std::time::Instant::now() + mixengine_testkit::home::STARTUP;
    loop {
        let status = json(&home.mix(&["service", "status", "mariadb@main", "--json"]));
        if status["state"] == "running" {
            break;
        }

        assert!(
            std::time::Instant::now() < deadline,
            "the accepted walk never started the service: {status}\n--- daemon.log ---\n{}",
            home.daemon_log()
        );
        std::thread::sleep(std::time::Duration::from_millis(50));
    }
}
