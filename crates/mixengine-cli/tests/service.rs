//! `mix service` against a real daemon supervising a real process.
//!
//! Task **T19b**, and the reason it is an end-to-end test rather than a unit one: everything below
//! the client has been proved in its own crate — the registry walks a plan, the handlers answer
//! `service.*` — and what has never been proved is that a person typing four words gets a process
//! started and an exit code that means it. That claim spans two operating-system processes and a
//! socket, so nothing here is mocked.
//!
//! **The services are declared through `MIXENGINE_DEV_SPECS`**, a debug build's stand-in for the
//! generator of T30 (see `crates/mixengine-daemon/src/services/spec.rs`), and their `services` rows
//! are written by `mixengine_testkit::declare`, which is Phase 3's `service.create` in the same
//! sense. Both disappear when the real things arrive; what stays is every assertion below them.

mod harness;

use std::path::PathBuf;

use harness::{Home, json, stdout};
use mixengine_proto::{
    Millis, ReadyCheck, RestartPolicy, ServiceId, ServiceSpec, ServiceSpecBuilder, StopBehaviour,
};
use mixengine_testkit::FakeService;
use serde_json::Value;

/// A `fakeservice` that says it is ready and then waits to be stopped.
///
/// `RestartPolicy::Never`, because nothing here is about restarts: a policy that put a failed
/// service back would turn a failing assertion into a test that takes a minute to fail.
fn spec(id: &str, fake: FakeService) -> ServiceSpecBuilder {
    ServiceSpec::builder(service(id), FakeService::program())
        .args(
            fake.args()
                .iter()
                .map(|arg| arg.to_string_lossy().into_owned())
                .collect::<Vec<_>>(),
        )
        .cwd(std::env::temp_dir())
        .ready(ReadyCheck::LogPattern {
            regex: mixengine_testkit::service::READY_LINE.to_owned(),
            timeout: Millis::from_secs(20),
        })
        .restart(RestartPolicy::Never)
        .stop(StopBehaviour::Signal { grace: Millis(500) })
}

/// An id, or this test's own bug.
fn service(id: &str) -> ServiceId {
    ServiceId::parse(id).expect("a valid service id")
}

/// Write `specs` where a daemon told to read them will find them.
///
/// Inside the home, so it goes when the home does. The daemon reads the file on every walk, which is
/// why it is written before the daemon starts and never touched again.
fn declared(home: &Home, specs: &[ServiceSpec]) -> PathBuf {
    let path = home.path().join("dev-specs.json");
    std::fs::create_dir_all(home.path()).expect("the home directory");
    std::fs::write(
        &path,
        serde_json::to_vec_pretty(specs).expect("specs serialise"),
    )
    .expect("the declarations are written");

    path
}

/// A home with a daemon in it, declaring `specs` and with a row for each of them.
///
/// The order is forced: the migrations that create the schema run when the daemon opens the home, so
/// there is nothing to insert a row into until it is up.
fn running(specs: &[ServiceSpec]) -> (Home, harness::Daemon) {
    let home = Home::new();
    let file = declared(&home, specs);
    let daemon = home.start_daemon_declaring(&file);

    let ids: Vec<&str> = specs.iter().map(|spec| spec.id().as_str()).collect();
    home.declare(&ids);

    (home, daemon)
}

#[test]
fn a_service_starts_stops_and_says_so_in_both_renderings() {
    let specs = vec![
        spec("mariadb@main", FakeService::new())
            .build()
            .expect("a valid spec"),
    ];
    let (home, _daemon) = running(&specs);

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
fn starting_one_service_starts_what_it_depends_on_and_says_which() {
    let specs = vec![
        spec("mariadb@main", FakeService::new())
            .build()
            .expect("a valid spec"),
        spec("php-fpm@8.3", FakeService::new())
            .depends_on(service("mariadb@main"))
            .build()
            .expect("a valid spec"),
    ];
    let (home, _daemon) = running(&specs);

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
fn a_service_that_never_becomes_ready_fails_the_command_and_names_the_one_to_fix() {
    let specs = vec![
        spec("mariadb@main", FakeService::new().never_ready())
            .ready(ReadyCheck::LogPattern {
                // Short, because this is the one test that waits the timeout out on purpose.
                regex: mixengine_testkit::service::READY_LINE.to_owned(),
                timeout: Millis::from_secs(2),
            })
            .build()
            .expect("a valid spec"),
        spec("php-fpm@8.3", FakeService::new())
            .depends_on(service("mariadb@main"))
            .build()
            .expect("a valid spec"),
    ];
    let (home, _daemon) = running(&specs);

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
fn a_walk_nobody_waits_for_is_reported_as_accepted_rather_than_as_finished() {
    let specs = vec![
        spec("mariadb@main", FakeService::new().ready_after(1_000))
            .build()
            .expect("a valid spec"),
    ];
    let (home, _daemon) = running(&specs);

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
