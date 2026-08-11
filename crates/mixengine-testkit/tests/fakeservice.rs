//! The fixture, tested against itself.
//!
//! A fixture nobody has checked is worse than no fixture: a `fakeservice` that quietly ignored
//! `--never-ready` would turn a supervisor test about ready timeouts into a test that passes because
//! the service came up. Each mode below is one of the behaviours
//! `.claude/architecture/process-supervision.md` requires of it, proved here so that a failure in
//! Phase 1 is a failure of the supervisor rather than of the thing it is being measured with.
//!
//! Nothing here touches the network, and every file it writes is inside its own `TempDir`.

use std::path::Path;
use std::time::{Duration, Instant};

use mixengine_testkit::service::READY_LINE;
use mixengine_testkit::{FakeService, try_stop};

/// How long a test waits for a file another process is writing.
///
/// Only ever waited out in full when something is wrong: every use below returns as soon as the file
/// is there. Generous because process startup on a loaded Windows runner is measured in seconds.
const APPEARS: Duration = Duration::from_secs(20);

/// A run long enough to observe and short enough not to slow the suite down.
const BRIEF: u64 = 400;

#[test]
fn a_service_that_behaves_announces_itself_and_then_ends_successfully() {
    let output = FakeService::new().exit_after(BRIEF).spawn().finish();

    assert!(output.status.success(), "{output:?}");
    assert!(
        stdout(&output).contains(READY_LINE),
        "the baseline service announces readiness: {}",
        stdout(&output)
    );
}

#[test]
fn a_slow_start_has_announced_nothing_yet() {
    // The ready *timeout* case, written without measuring a clock: the service is given a readiness
    // delay it is not allowed to reach, so the absence of the line is a fact rather than a race.
    let output = FakeService::new()
        .ready_after(BRIEF * 10)
        .exit_after(BRIEF)
        .spawn()
        .finish();

    assert!(
        !stdout(&output).contains(READY_LINE),
        "a service still starting up has not announced readiness: {}",
        stdout(&output)
    );
}

#[test]
fn a_service_that_never_becomes_ready_says_so_by_saying_nothing() {
    let output = FakeService::new()
        .never_ready()
        .exit_after(BRIEF)
        .spawn()
        .finish();

    assert!(
        !stdout(&output).contains(READY_LINE),
        "--never-ready announced readiness anyway: {}",
        stdout(&output)
    );
}

#[test]
fn a_service_that_crashes_reports_the_code_it_crashed_with() {
    // What `RestartPolicy::OnFailure` turns on, and the reason `--exit-code` exists at all: a
    // service that ended by itself with a non-zero status is the definition of a crash here.
    let output = FakeService::new()
        .exit_after(BRIEF)
        .exit_code(3)
        .spawn()
        .finish();

    assert_eq!(output.status.code(), Some(3), "{output:?}");
}

#[test]
fn a_service_writes_the_lines_it_was_asked_for() {
    let output = FakeService::new()
        .log_every(BRIEF / 8)
        .exit_after(BRIEF)
        .spawn()
        .finish();

    let written = stdout(&output);
    assert!(written.contains("fakeservice: line 1"), "{written}");
    assert!(
        written.contains("fakeservice: line 2"),
        "the lines are numbered so log capture can prove it lost none: {written}"
    );
}

#[test]
fn a_service_records_the_pid_a_test_will_have_to_find_it_by() {
    let directory = tempfile::tempdir().expect("a directory to record a pid in");
    let pid_file = directory.path().join("service.pid");

    let service = FakeService::new()
        .pid_file(&pid_file)
        .exit_after(BRIEF * 10)
        .spawn();

    assert_eq!(
        pid_in(&pid_file),
        service.id(),
        "the pid on disk names the process that wrote it"
    );
}

#[test]
fn an_orphan_outlives_the_process_that_left_it_behind() {
    // The case the supervisor exists to prevent: a child that is still running after its parent is
    // gone. Nothing here supervises anything, so what is proved is that the fixture can *produce*
    // one — a Job Object taking it down with its parent is what T13 will assert on top of this.
    let directory = tempfile::tempdir().expect("a directory to record a pid in");
    let pid_file = directory.path().join("orphan.pid");

    let service = FakeService::new()
        .orphan(&pid_file)
        .exit_after(BRIEF)
        .spawn();
    let mut orphan = Orphan(Some(pid_in(&pid_file)));

    assert_ne!(
        orphan.pid(),
        service.id(),
        "the orphan is a process of its own"
    );

    // Timed, because the failure this guards against is a hang rather than a wrong answer: a
    // `fakeservice` that had spawned its orphan with a plain `Command` would leave it holding a copy
    // of the parent's stdout, and reading the parent to end-of-file would then wait for the *orphan*
    // — the whole of roadmap tasks T9 and T10, one process further out. Without this the suite would
    // sit here for `ORPHAN_LIFETIME` and then pass.
    let waited = Instant::now();
    let parent = service.finish();

    assert!(parent.status.success(), "{parent:?}");
    assert!(
        waited.elapsed() < APPEARS,
        "reading the parent to end-of-file waited for the orphan it left behind"
    );

    // Stopping it is how a test asks whether it is there: both systems report a pid that is not
    // running as a failure to stop it, and this is a process no test of ours could have left as a
    // zombie, which is the one case `try_stop` cannot tell apart.
    assert!(
        orphan.stop(),
        "nothing was left at the orphan's pid, so it did not outlive its parent"
    );
}

/// The orphan's pid, stopped however this test ends.
///
/// A bare `try_stop` at the end of the body would be skipped by any assertion above it that failed —
/// which is exactly when a run is already in trouble — and leave a process on the machine for
/// `ORPHAN_LIFETIME`. The `Option` is what lets [`stop`](Self::stop) make the claim the test is
/// about while [`Drop`] keeps the promise on every other path.
struct Orphan(Option<u32>);

impl Orphan {
    fn pid(&self) -> u32 {
        self.0.expect("the pid is held until stop takes it")
    }

    /// Stop it now, and take the responsibility away from the drop below.
    fn stop(&mut self) -> bool {
        try_stop(self.0.take().expect("the pid is held until stop takes it"))
    }
}

impl Drop for Orphan {
    fn drop(&mut self) {
        if let Some(pid) = self.0.take() {
            // Ignored rather than asserted on: this only runs when the test is already failing for
            // some other reason, and a panic while unwinding aborts the whole run.
            let _ = try_stop(pid);
        }
    }
}

/// Ignoring a request to stop is a Unix-only claim from out here.
///
/// A test can send `SIGTERM` to a process it is not the parent of; the Windows equivalent is a
/// console control event, which is addressed to a process *group* and would reach `cargo test`
/// itself — `crates/mixengine-daemon/tests/lifecycle.rs` explains the same limitation at length, and
/// `try_stop` is a `taskkill /F` there precisely because there is nothing gentler to send. The
/// Windows half of this behaviour is proved by the supervisor instead, which sends the event to a
/// group it owns (roadmap task T15).
#[cfg(unix)]
#[test]
fn a_service_can_be_told_to_ignore_being_asked_to_stop() {
    let mut service = FakeService::new().ignoring_stop().spawn();

    // Waited for rather than assumed, and it is the whole test: a spawn returns as soon as the OS
    // has a process, and `SIGTERM` arriving before that process has installed its handlers ends it
    // through the default disposition — a service that was never asked anything, failing here as if
    // it had ignored nothing.
    assert!(
        service.wait_for_stdout(READY_LINE, APPEARS),
        "the service announced itself, so its stop handlers are installed"
    );

    // Asked to stop, and given several times the time it would take to honour it.
    assert!(try_stop(service.id()), "the service was there to be asked");
    std::thread::sleep(Duration::from_millis(BRIEF));

    assert!(
        service.still_running(),
        "--ignore-stop stopped anyway, so a grace-period test would prove nothing"
    );
}

#[cfg(unix)]
#[test]
fn a_service_that_is_not_ignoring_it_stops_when_asked() {
    // The other half, and the one that makes the test above mean something: the same request, to the
    // same program without the flag, ends it.
    let mut service = FakeService::new().spawn();

    // Same wait, for a different reason: without it this passes whether the service handled the
    // signal or was killed by the default disposition before it could, which is a pass that would
    // survive the handlers being removed altogether.
    assert!(
        service.wait_for_stdout(READY_LINE, APPEARS),
        "the service announced itself, so its stop handlers are installed"
    );

    assert!(try_stop(service.id()), "the service was there to be asked");

    let deadline = Instant::now() + APPEARS;
    while service.still_running() {
        assert!(
            Instant::now() < deadline,
            "the service ignored a stop it was not told to ignore"
        );
        std::thread::sleep(Duration::from_millis(10));
    }
}

/// Everything a finished process wrote to its standard output.
fn stdout(output: &std::process::Output) -> String {
    String::from_utf8_lossy(&output.stdout).into_owned()
}

/// The pid recorded at `path`, once whoever is writing it has.
///
/// Polls rather than sleeps: the file is written by another process, and how long that takes is a
/// property of the machine rather than of the test.
fn pid_in(path: &Path) -> u32 {
    let deadline = Instant::now() + APPEARS;

    while Instant::now() < deadline {
        if let Ok(contents) = std::fs::read_to_string(path)
            && let Ok(pid) = contents.trim().parse()
        {
            return pid;
        }

        std::thread::sleep(Duration::from_millis(10));
    }

    panic!(
        "nothing wrote a pid to {} within {APPEARS:?}",
        path.display()
    );
}
