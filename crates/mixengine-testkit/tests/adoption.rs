//! A pid is not an identity. A pid and a start time are — roadmap task **T18**.
//!
//! `mixengine_platform::process::started_at` and [`Adopted`] are what a daemon uses to decide, at
//! its next start, whether the process a `services` row names is still the one that row was written
//! about. Everything crash recovery does afterwards hangs off that answer: a survivor is supervised
//! again, and a row that fails the check is cleared **without anything being signalled**. Getting it
//! wrong in the other direction is the accident this product cannot have — asking somebody else's
//! program to shut down because the operating system handed out a number again.
//!
//! **Here rather than in `crates/mixengine-platform/tests/` for the reason `supervision.rs` gives**:
//! the process under test is `fakeservice`, a binary of *this* package, and `CARGO_BIN_EXE_…`
//! reaches only the package a test is in. What is being tested is still
//! `mixengine_platform::process`.
//!
//! # Why a process this test started is a fair subject
//!
//! Adoption is about a process this daemon did **not** start, and every process a test can reliably
//! produce is one it did. The distinction does not reach the code under test: `started_at` asks the
//! operating system about a pid and knows nothing about who spawned it, and `Adopted` holds a number
//! and a start time. What a child of this test buys is the two things a survivor cannot be asked
//! for — it can be killed on demand, and it cannot be left behind when the test fails.
//!
//! The one place the difference is real is Unix reaping, and it is *harder* here rather than easier:
//! a killed child of this process stays a zombie until something waits for it, which is precisely
//! the state `started_at` has to report as gone and the reason it checks for one.

use std::time::{Duration, Instant};

use mixengine_platform::process::{Adopted, StartTime, started_at};
use mixengine_testkit::{FakeService, service::READY_LINE};

/// How long a test waits for a process on the other side of the machine to do something.
///
/// Only ever waited out in full when something is wrong. Generous because starting a process on a
/// loaded Windows runner is measured in seconds.
const EVENTUALLY: Duration = Duration::from_secs(20);

#[test]
fn a_process_that_is_running_says_when_it_began() {
    // This process, which is the one subject that certainly exists and certainly has not been
    // recycled. Asked twice, because a reading that changed between two calls would not identify
    // anything at all — the whole mechanism is that the answer is fixed for the life of a process.
    let mine = started_at(std::process::id())
        .expect("this system can be asked when a process began")
        .expect("this process is running");

    let again = started_at(std::process::id())
        .expect("this system can be asked when a process began")
        .expect("this process is still running");

    assert_eq!(
        mine, again,
        "the start time of one process changed between two readings, so it identifies nothing"
    );
}

#[test]
fn a_running_process_is_identified_by_its_pid_and_the_moment_it_began() {
    let service = started();
    let pid = service.id();

    let started = started_at(pid)
        .expect("this system can be asked when a process began")
        .expect("the fixture is running");

    let adopted = Adopted::identify(pid, started)
        .expect("this system can be asked about a process it did not start")
        .expect("the process is the one that was recorded");

    assert_eq!(adopted.pid(), pid);
    assert!(
        adopted
            .exited()
            .expect("this system can be asked whether a process is still there")
            .is_none(),
        "a running process was reported as ended"
    );
}

/// **The guard the whole task rests on.** A pid the operating system has handed out again names a
/// process that began later, and adoption has to refuse it — the alternative is a daemon that stops
/// a stranger's program because a number matched.
///
/// The recycled pid is simulated rather than waited for: producing a real one means exhausting the
/// pid space, which takes minutes on Linux and is not reproducible at all on Windows. What the code
/// under test does with the two values is the same either way — they are compared — so the honest
/// subject is a start time that does not match, however it came to differ.
#[test]
fn a_pid_that_carries_a_different_start_time_is_refused() {
    let service = started();
    let pid = service.id();

    let started = started_at(pid)
        .expect("this system can be asked when a process began")
        .expect("the fixture is running");

    let somebody_else = StartTime::from_stored(started.stored() + 1);

    assert!(
        Adopted::identify(pid, somebody_else)
            .expect("this system can be asked about a process it did not start")
            .is_none(),
        "a live pid was adopted on a start time that was not its own, which is how a supervisor \
         ends up signalling a stranger"
    );
}

/// A process that has ended is gone even where the operating system still remembers it.
///
/// Both systems keep something behind after a kill, and for the same reason — somebody has to be
/// able to read the status — so both would answer "there is a process with that pid" to a naive
/// question. On Unix it is a zombie until this test reaps it; on Windows the process object stays
/// openable while this test holds a handle to it, and only its *exit time* separates it from a
/// process that is running. Either would have made a daemon adopt a corpse, supervise it for ever
/// and never restart the service.
#[test]
fn a_process_that_has_been_killed_is_reported_as_gone() {
    let mut service = started();
    let pid = service.id();

    let started = started_at(pid)
        .expect("this system can be asked when a process began")
        .expect("the fixture is running");

    let adopted = Adopted::identify(pid, started)
        .expect("this system can be asked about a process it did not start")
        .expect("the process is the one that was recorded");

    assert!(
        mixengine_testkit::try_kill(pid),
        "the fixture was there to be killed"
    );

    // Waited for rather than asserted at once: killing is a request the kernel carries out when it
    // gets to it, and on Unix `still_running` is also what reaps the zombie this test then asks
    // about — the harder of the two states, and the one a supervisor really meets.
    let deadline = Instant::now() + EVENTUALLY;
    while service.still_running() {
        assert!(
            Instant::now() < deadline,
            "the fixture survived being killed for {EVENTUALLY:?}"
        );

        std::thread::sleep(Duration::from_millis(10));
    }

    let exit = adopted
        .exited()
        .expect("this system can be asked whether a process is still there")
        .expect("the process was killed");

    assert!(
        !exit.is_success(),
        "a process that disappeared under an adopted watch cannot be called a clean exit: {exit}"
    );
    assert_eq!(
        exit.code(),
        None,
        "nothing watched this process end, so there is no status to report"
    );

    assert!(
        Adopted::identify(pid, started)
            .expect("this system can be asked about a process it did not start")
            .is_none(),
        "the pid of a process that has ended was adopted again"
    );
}

/// The other half of what an adopted handle is for: it can stop what it identified.
///
/// The process here is this test's child rather than a survivor, which changes nothing about the
/// call — `Adopted::stop` addresses a pid, or on Unix the group it leads, and neither is a
/// relationship the caller has to be in. What it cannot do is *wait*, so the test polls exactly as
/// the runner does.
#[test]
fn an_adopted_process_can_be_stopped() {
    let mut service = started();
    let pid = service.id();

    let started = started_at(pid)
        .expect("this system can be asked when a process began")
        .expect("the fixture is running");

    let adopted = Adopted::identify(pid, started)
        .expect("this system can be asked about a process it did not start")
        .expect("the process is the one that was recorded");

    adopted.stop().expect("an adopted process can be stopped");

    // Reaped here for the same reason as above, and it is what makes the identity check below the
    // question this test is asking rather than a question about a zombie.
    let deadline = Instant::now() + EVENTUALLY;
    while service.still_running() {
        assert!(
            Instant::now() < deadline,
            "the adopted process was still running {EVENTUALLY:?} after it was stopped"
        );

        std::thread::sleep(Duration::from_millis(10));
    }

    assert!(
        adopted
            .exited()
            .expect("this system can be asked whether a process is still there")
            .is_some(),
        "an adopted process that was stopped still reports as running"
    );
}

/// A `fakeservice` that has started, announced itself and is waiting to be stopped.
///
/// Waited for rather than used the moment `spawn` returns: a spawn returns as soon as the OS has a
/// process, which is before that process has parsed its arguments — and a start time read in that
/// window is still this process's own, so the test would be right for the wrong reason.
fn started() -> mixengine_testkit::service::Running {
    let service = FakeService::new().spawn();

    assert!(
        service.wait_for_stdout(READY_LINE, EVENTUALLY),
        "the fixture did not start within {EVENTUALLY:?}"
    );

    service
}
