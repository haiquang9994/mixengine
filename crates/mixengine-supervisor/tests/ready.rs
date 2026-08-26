//! Waiting for a real process to become ready — or to die trying.
//!
//! The unit tests beside `ready.rs` are about the parts a process cannot show: a pattern that does
//! not compile, a probe this build cannot make. These are about the race the module exists for, and
//! it needs three real things at once — a process, a clock, and something to connect to.

use std::collections::BTreeMap;
use std::net::{Ipv4Addr, SocketAddr};
use std::time::{Duration, Instant};

use mixengine_platform::process::{Limits, Supervised, spawn_supervised};
use mixengine_proto::{LogPolicy, Millis, ReadyCheck, ServiceId};
use mixengine_supervisor::Surroundings;
use mixengine_supervisor::logs::Capture;
use mixengine_supervisor::ready::{self, Ready};
use mixengine_testkit::FakeService;

/// Where a check that runs a program would run it.
///
/// Every check but `ReadyCheck::Command` ignores this, so the tests below that name it are saying
/// only that a wait needs somewhere to run — not that this is where anything ran.
fn nowhere() -> Surroundings {
    Surroundings::new(std::env::temp_dir(), BTreeMap::new())
}

fn service() -> ServiceId {
    ServiceId::parse("fakeservice").expect("a valid id")
}

/// Start `fixture` supervised, with its output captured the way the supervisor will.
fn started(fixture: FakeService) -> (Supervised, Capture) {
    let mut supervised = spawn_supervised(
        &FakeService::program(),
        fixture.args(),
        &std::env::temp_dir(),
        &BTreeMap::new(),
        &Limits::default(),
    )
    .expect("a fakeservice can be supervised");

    let capture = Capture::start(&mut supervised, &service(), LogPolicy::default(), None);

    (supervised, capture)
}

/// A pattern no `fakeservice` ever prints, for the tests that are about something else finishing
/// first.
fn never_matches(timeout: Millis) -> ReadyCheck {
    ReadyCheck::LogPattern {
        regex: "^this service will never say this$".to_owned(),
        timeout,
    }
}

#[tokio::test]
async fn a_service_that_announces_itself_is_ready() {
    let (mut supervised, capture) = started(FakeService::new().ready_after(100));
    let check = ReadyCheck::LogPattern {
        regex: "fakeservice: ready".to_owned(),
        timeout: Millis::from_secs(20),
    };

    let outcome = ready::wait(&check, &mut supervised, &capture, &nowhere())
        .await
        .expect("a pattern that compiles, on a system that can read a pipe");

    assert!(matches!(outcome, Ready::Ready), "{outcome:?}");

    supervised.stop().expect("the service can be stopped");
}

/// The check that measures nothing, and the one thing it does measure.
#[tokio::test]
async fn a_service_that_only_has_to_survive_is_ready_once_it_has() {
    let (mut supervised, capture) = started(FakeService::new().never_ready());
    let check = ReadyCheck::PidAlive {
        settle: Millis(200),
    };

    let started_at = Instant::now();
    let outcome = ready::wait(&check, &mut supervised, &capture, &nowhere())
        .await
        .expect("a settling period needs nothing of the system");

    assert!(matches!(outcome, Ready::Ready), "{outcome:?}");
    assert!(
        started_at.elapsed() >= Duration::from_millis(200),
        "the settling period was not waited out"
    );

    supervised.stop().expect("the service can be stopped");
}

/// The answer a naive implementation misses: the process ends while the check is still waiting.
///
/// The timeout here is twenty seconds and the fixture exits after two hundred milliseconds, so a
/// wait that reported `TimedOut` would take a hundred times longer *and* say the wrong thing. The
/// elapsed-time assertion is what makes that a failure rather than a slow pass.
#[tokio::test]
async fn a_service_that_dies_before_it_is_ready_says_so_at_once() {
    let (mut supervised, capture) = started(
        FakeService::new()
            .never_ready()
            .exit_after(200)
            .exit_code(2),
    );

    let started_at = Instant::now();
    let outcome = ready::wait(
        &never_matches(Millis::from_secs(20)),
        &mut supervised,
        &capture,
        &nowhere(),
    )
    .await
    .expect("a pattern that compiles");

    let Ready::Exited(exit) = outcome else {
        panic!("a process that exited was not reported as one: {outcome:?}");
    };

    assert!(!exit.is_success(), "exit code 2 is not a success: {exit}");
    assert!(
        started_at.elapsed() < Duration::from_secs(10),
        "the exit was noticed by the timeout rather than by the OS"
    );
}

#[tokio::test]
async fn a_service_that_never_becomes_ready_times_out_and_is_left_running() {
    let (mut supervised, capture) = started(FakeService::new().never_ready());

    let outcome = ready::wait(
        &never_matches(Millis(300)),
        &mut supervised,
        &capture,
        &nowhere(),
    )
    .await
    .expect("a pattern that compiles");

    assert!(matches!(outcome, Ready::TimedOut), "{outcome:?}");
    assert!(
        supervised
            .exited()
            .expect("the OS can be asked about a child")
            .is_none(),
        "a ready timeout stopped the process, which is the supervisor's decision and not this one's"
    );

    supervised.stop().expect("the service can be stopped");
}

/// A port nothing is listening on, and then one something is.
#[tokio::test]
async fn a_tcp_check_passes_once_something_accepts() {
    let (mut supervised, capture) = started(FakeService::new().never_ready());

    // Bound first to be given a free port, then closed, so the check really does start against a
    // port nothing is on — asking the OS for one and hoping it stays free is the flaky version.
    let scout = tokio::net::TcpListener::bind(SocketAddr::from((Ipv4Addr::LOCALHOST, 0)))
        .await
        .expect("a loopback port");
    let addr = scout.local_addr().expect("a bound listener has an address");
    drop(scout);

    let listening = tokio::spawn(async move {
        tokio::time::sleep(Duration::from_millis(200)).await;
        tokio::net::TcpListener::bind(addr)
            .await
            .expect("the port is still free")
    });

    let check = ReadyCheck::Tcp {
        addr,
        timeout: Millis::from_secs(20),
    };

    let outcome = ready::wait(&check, &mut supervised, &capture, &nowhere())
        .await
        .expect("a TCP check needs nothing of the spec");

    assert!(matches!(outcome, Ready::Ready), "{outcome:?}");

    drop(listening.await.expect("the listener task did not panic"));
    supervised.stop().expect("the service can be stopped");
}

/// A spec written for the other family of system, on Windows.
///
/// Asserted rather than skipped, the way ADR 0007's macOS gap is: the answer owed here is a typed
/// refusal naming the spec, not a wait that ends in a timeout blaming the service.
#[cfg(windows)]
#[tokio::test]
async fn a_unix_socket_check_on_windows_blames_the_spec_rather_than_the_service() {
    use mixengine_supervisor::Error;

    let (mut supervised, capture) = started(FakeService::new().never_ready());
    let check = ReadyCheck::UnixSocket {
        path: std::path::PathBuf::from("C:\\mixengine\\run\\php-fpm.sock"),
        timeout: Millis::from_secs(20),
    };

    let started_at = Instant::now();
    let error = ready::wait(&check, &mut supervised, &capture, &nowhere())
        .await
        .expect_err("this system has no such socket");

    assert!(
        matches!(&error, Error::Platform(_)),
        "the platform's own answer is what should reach the caller: {error:?}"
    );
    assert!(
        started_at.elapsed() < Duration::from_secs(5),
        "a check that cannot be made was waited out instead of refused"
    );

    supervised.stop().expect("the service can be stopped");
}

/// A program that exits zero only when `sentinel` exists, spelled for this system.
///
/// `cfg!` as a value rather than an attribute, so both arms compile everywhere. Reading the file is
/// what makes the answer, rather than a shell conditional: `cmd.exe` does not parse the quoting
/// Rust applies to an argument with spaces in it, so a one-argument `if exist "..."` is a command
/// that fails for a reason that has nothing to do with the file.
fn probe_for(sentinel: &std::path::Path) -> (std::path::PathBuf, Vec<String>) {
    if cfg!(windows) {
        (
            std::path::PathBuf::from(r"C:\Windows\System32\cmd.exe"),
            vec![
                "/c".to_owned(),
                "type".to_owned(),
                sentinel.display().to_string(),
            ],
        )
    } else {
        (
            std::path::PathBuf::from("/bin/cat"),
            vec![sentinel.display().to_string()],
        )
    }
}

/// A ready check that runs a program passes when the program starts exiting zero.
///
/// The point is the *retry*: the first runs fail, exactly as `mariadb-admin ping` fails against a
/// server still recovering, and the check is only satisfied when one of them succeeds.
#[tokio::test]
async fn a_command_ready_check_waits_until_the_command_succeeds() {
    let directory = tempfile::tempdir().expect("a directory");
    let sentinel = directory.path().join("up");
    let (program, args) = probe_for(&sentinel);

    let (mut supervised, capture) = started(FakeService::new().never_ready());
    let place = Surroundings::new(std::env::temp_dir(), BTreeMap::new());

    let waiting = async {
        ready::wait(
            &ReadyCheck::Command {
                program,
                args,
                timeout: Millis::from_secs(20),
            },
            &mut supervised,
            &capture,
            &place,
        )
        .await
    };

    let arriving = async {
        tokio::time::sleep(Duration::from_millis(300)).await;
        std::fs::write(&sentinel, b"").expect("the sentinel can be written");
    };

    let (outcome, ()) = tokio::join!(waiting, arriving);

    assert!(
        matches!(outcome.expect("a program this system has"), Ready::Ready),
        "the check never passed once the program started succeeding"
    );

    supervised.stop().expect("the service can be stopped");
}

/// And it gives up at its own deadline rather than waiting for ever.
#[tokio::test]
async fn a_command_ready_check_that_never_succeeds_times_out() {
    let directory = tempfile::tempdir().expect("a directory");
    let (program, args) = probe_for(&directory.path().join("never"));

    let (mut supervised, capture) = started(FakeService::new().never_ready());
    let place = Surroundings::new(std::env::temp_dir(), BTreeMap::new());

    let outcome = ready::wait(
        &ReadyCheck::Command {
            program,
            args,
            timeout: Millis(600),
        },
        &mut supervised,
        &capture,
        &place,
    )
    .await
    .expect("a program this system has");

    assert!(matches!(outcome, Ready::TimedOut), "{outcome:?}");

    supervised.stop().expect("the service can be stopped");
}
