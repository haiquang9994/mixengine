//! Running a service's own programs, against real processes.
//!
//! The unit test beside `command.rs` is about what [`Surroundings`] prints. This is about the one
//! thing only a real process can show: **what a deadline is measured against**. A one-shot exits and
//! its pipes close, and those are two different moments — a caller that confuses them reports a
//! program that finished in milliseconds as one that hung.

use std::collections::BTreeMap;
use std::ffi::OsString;
use std::time::{Duration, Instant};

use mixengine_supervisor::Surroundings;
use mixengine_testkit::FakeService;

/// Longer than anything here is meant to need, so a test that fails says the deadline was reached
/// rather than that a loaded runner was slow.
///
/// It has a floor as well as a ceiling, which is easy to miss: the run below really does take about
/// two seconds even when everything works, because a process whose pipe is still held is exactly
/// the case `run_once` waits `LAST_WORDS` on before answering. Five seconds leaves that its margin.
/// Anything at or under two would fail the elapsed assertion on correct code.
const PATIENCE: Duration = Duration::from_secs(5);

/// How long a lingering child holds its parent's stdout for.
///
/// Longer than [`PATIENCE`], and that is the whole of what makes the test discriminate: on the code
/// this is a regression test for, the run below reached its deadline and reported a timeout, and a
/// child that let go any sooner would have let that code pass too.
///
/// It is also what this test costs, on Windows. `run_once` returns as soon as the process has gone,
/// but tokio reads a child's pipes on the blocking pool there — the read that is still waiting on
/// this child cannot be cancelled, and dropping the test's runtime waits for it. The assertions have
/// all been made by then; the clock is the only thing still running.
///
/// **Handing the test the release instead was tried and is worse.** The runtime is dropped after
/// every local in the test body, so a release file living in the test's own temporary directory is
/// deleted before the child's next poll ever sees it — and the run falls back to the fixture's
/// minute-long ceiling. Eight seconds spent on purpose beats sixty spent by accident.
const LINGER: u64 = 8_000;

/// Anywhere, with nothing: these tests are about the wait, not about where a probe runs.
fn anywhere() -> Surroundings {
    Surroundings::new(std::env::temp_dir(), BTreeMap::new())
}

/// A fixture's arguments in the shape [`Surroundings::run`] takes them.
fn args(fixture: &FakeService) -> Vec<String> {
    fixture
        .args()
        .iter()
        .map(OsString::as_os_str)
        .map(|arg| arg.to_string_lossy().into_owned())
        .collect()
}

/// The regression, and the one this module exists for.
///
/// `wait_with_output` waits for end of file as well as for the process, and end of file on a pipe is
/// the *last holder of it* exiting. So a one-shot with a helper behind it was reported as having
/// timed out however cleanly it had exited — which for a `StopBehaviour::Command` means killing the
/// database that had just been asked to shut down properly, and for a `HealthProbe::Command` means
/// degrading a healthy service every interval, for ever.
#[tokio::test]
async fn a_one_shot_that_leaves_a_child_holding_its_output_is_not_a_timeout() {
    let home = tempfile::tempdir().expect("a temporary directory");
    let ran_at = home.path().join("ran");

    // Leaves the child first and exits immediately after: by the time the run below has anything to
    // read, the program it started has already gone and its stdout is held open by something else.
    let fixture = FakeService::new().lingering_child(LINGER).touch(&ran_at);
    let began = Instant::now();

    let ran = anywhere()
        .run(&FakeService::program(), &args(&fixture), PATIENCE)
        .await
        .expect("a fixture that is on this machine can be run");

    assert!(
        !ran.timed_out(),
        "it exited long before the deadline; only its pipe was still open"
    );
    assert!(
        ran.succeeded(),
        "the status of a program that did its job: {:?}",
        ran.exit()
    );
    assert!(
        began.elapsed() < PATIENCE,
        "the wait was bounded by the process, so it cannot have taken the whole deadline: {:?}",
        began.elapsed()
    );
    assert!(
        ran_at.is_file(),
        "the run really happened rather than being answered from somewhere else"
    );
}

/// The other half, so the fix above cannot have been "never time out".
///
/// A program that is still running when its patience runs out is killed and reported as timed out,
/// and that is the answer a health probe with no deadline would have read as *healthy* for as long
/// as the service stayed broken.
#[tokio::test]
async fn a_one_shot_that_will_not_finish_runs_out_of_patience() {
    let fixture = FakeService::new().never_ready();

    let ran = anywhere()
        .run(
            &FakeService::program(),
            &args(&fixture),
            Duration::from_millis(250),
        )
        .await
        .expect("a fixture that is on this machine can be run");

    assert!(ran.timed_out(), "it never ends by itself: {:?}", ran.exit());
}

/// What a program said is what the log line about it is worth reading for, and it survives a run
/// that failed — `ERROR 1045: Access denied` is the whole of what a user has to act on.
#[tokio::test]
async fn what_a_one_shot_complained_about_comes_back_with_it() {
    // No such flag, so clap refuses it, prints its usage on stderr and exits non-zero. A misspelled
    // spec is exactly the shape of failure this is for.
    let args = vec!["--not-a-flag-any-fixture-has".to_owned()];

    let ran = anywhere()
        .run(&FakeService::program(), &args, PATIENCE)
        .await
        .expect("a fixture that is on this machine can be run");

    assert!(!ran.timed_out(), "clap answers at once");
    assert!(
        !ran.succeeded(),
        "an argument it does not know is a failure"
    );
    assert!(
        ran.complaint().is_some_and(|said| !said.is_empty()),
        "it printed its usage, and that is the line worth logging: {:?}",
        ran.complaint()
    );
}
