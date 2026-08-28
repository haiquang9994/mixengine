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

    // **A delay is safe here where it was not for the command check**, and the difference is what
    // the check's own attempt costs. A refused connection on loopback comes back in microseconds
    // against a 50ms retry, so 200ms is four attempts of margin and the check cannot miss its first
    // failure; spawning `cmd.exe` costs more than the delay the command test used to sleep, which
    // is why that one waits on evidence instead. There is no evidence to wait on here — a connection
    // that was refused leaves nothing behind.
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

/// A program that exits zero only when its sentinel exists, and records every run it makes.
///
/// **The record is the point.** What the retry test proves is that the check runs the program more
/// than once, and an argument list cannot leave a trace: a process spawned with `cat missing` fails
/// and is gone. So the probe is a script this test writes, it appends a byte before it answers, and
/// the file it appends to is both how the test knows an attempt has happened and how it asserts
/// afterwards that more than one did.
///
/// **A script rather than `cat`/`type` directly, and the quoting is why it can be one.** `cmd.exe`
/// does not parse the quoting Rust applies to an argument with spaces in it, which is what makes a
/// one-argument `if exist "..."` fail for a reason that has nothing to do with the file. Inside a
/// `.cmd` file the quoting is ours: `%~1` strips what Rust added and the script puts its own back.
///
/// **The record is written after the answer is decided, and that ordering is the whole of it.** A
/// probe that marked its arrival first would let the test see an attempt that had not yet looked:
/// the sentinel would land between the mark and the look, that same attempt would succeed, and the
/// retry would be gone again — the failure this was written to stop, arriving by a shorter route.
/// So the sentinel is tested, then the byte is appended, then the verdict is returned; a byte on
/// disk therefore means an attempt whose answer can no longer change.
struct Probe {
    program: std::path::PathBuf,
    args: Vec<String>,
    attempts: std::path::PathBuf,
}

impl Probe {
    /// How many times the check has run this probe.
    ///
    /// Counted by byte rather than by line: `echo` on Windows writes a CRLF and `printf` on Unix
    /// writes nothing at all, so the `x` is the only part both agree on.
    fn made(&self) -> usize {
        std::fs::read(&self.attempts).map_or(0, |bytes| {
            bytes.iter().filter(|byte| **byte == b'x').count()
        })
    }
}

/// `cfg!` as a value rather than an attribute, so both arms compile everywhere.
fn probe_for(directory: &std::path::Path, sentinel: &std::path::Path) -> Probe {
    let attempts = directory.join("attempts");

    let (script, body, program) = if cfg!(windows) {
        (
            directory.join("probe.cmd"),
            "@echo off\r\nset RC=1\r\nif exist \"%~2\" set RC=0\r\n>>\"%~1\" echo x\r\nexit %RC%\r\n",
            std::path::PathBuf::from(r"C:\Windows\System32\cmd.exe"),
        )
    } else {
        (
            directory.join("probe.sh"),
            "if test -f \"$2\"; then rc=0; else rc=1; fi\nprintf x >> \"$1\"\nexit $rc\n",
            std::path::PathBuf::from("/bin/sh"),
        )
    };

    std::fs::write(&script, body).expect("the probe can be written");

    let mut args = Vec::new();

    // `/c` on Windows and nothing on Unix: `sh` takes the script as its first argument already.
    if cfg!(windows) {
        args.push("/c".to_owned());
    }

    args.push(script.display().to_string());
    args.push(attempts.display().to_string());
    args.push(sentinel.display().to_string());

    Probe {
        program,
        args,
        attempts,
    }
}

/// A ready check that runs a program passes when the program starts exiting zero.
///
/// The point is the *retry*: the first runs fail, exactly as `mariadb-admin ping` fails against a
/// server still recovering, and the check is only satisfied when one of them succeeds.
///
/// **And the retry is asserted, not assumed.** A test that only checks the outcome is `Ready` is
/// green whether the check retried or answered first time, which is the same test with none of the
/// point left. [`Probe::made`] is what closes that, and the wait below is what makes it hold on a
/// machine of any speed.
#[tokio::test]
async fn a_command_ready_check_waits_until_the_command_succeeds() {
    let directory = tempfile::tempdir().expect("a directory");
    let sentinel = directory.path().join("up");
    let probe = probe_for(directory.path(), &sentinel);

    let (mut supervised, capture) = started(FakeService::new().never_ready());
    let place = Surroundings::new(std::env::temp_dir(), BTreeMap::new());

    let waiting = async {
        ready::wait(
            &ReadyCheck::Command {
                program: probe.program.clone(),
                args: probe.args.clone(),
                timeout: Millis::from_secs(20),
            },
            &mut supervised,
            &capture,
            &place,
        )
        .await
    };

    // **Waited for rather than slept past, and the wait is on the probe's own record.** This used to
    // sleep 300ms and hope the check had failed by then, which is a guess against a process spawn:
    // `cmd.exe` costs more than that on a busy Windows runner, so the sentinel could land before the
    // first attempt ever read for it. Nothing went red when it did — the check simply passed first
    // time and the retry this test exists for was never exercised.
    let arriving = async {
        let deadline = Instant::now() + Duration::from_secs(10);

        while probe.made() == 0 {
            assert!(
                Instant::now() < deadline,
                "the check never ran the probe at all"
            );
            tokio::time::sleep(Duration::from_millis(10)).await;
        }

        std::fs::write(&sentinel, b"").expect("the sentinel can be written");
    };

    let (outcome, ()) = tokio::join!(waiting, arriving);

    assert!(
        matches!(outcome.expect("a program this system has"), Ready::Ready),
        "the check never passed once the program started succeeding"
    );

    assert!(
        probe.made() >= 2,
        "the check passed on its first attempt, so the retry this test exists for never happened"
    );

    supervised.stop().expect("the service can be stopped");
}

/// And it gives up at its own deadline rather than waiting for ever.
#[tokio::test]
async fn a_command_ready_check_that_never_succeeds_times_out() {
    let directory = tempfile::tempdir().expect("a directory");
    let probe = probe_for(directory.path(), &directory.path().join("never"));

    let (mut supervised, capture) = started(FakeService::new().never_ready());
    let place = Surroundings::new(std::env::temp_dir(), BTreeMap::new());

    let outcome = ready::wait(
        &ReadyCheck::Command {
            program: probe.program.clone(),
            args: probe.args.clone(),
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
