//! A supervised process cannot outlive what supervises it — as far as each system can promise that.
//!
//! Roadmap task **T13**, and the inverse of `an_orphan_outlives_the_process_that_left_it_behind` in
//! `fakeservice.rs`: the same fixture, the same `mixengine-platform`, and the opposite outcome
//! asserted.
//!
//! **These live here rather than in `crates/mixengine-platform/tests/` for one reason**: the
//! processes under test are `fakeservice`, which is a binary of *this* package, and
//! `CARGO_BIN_EXE_…` reaches only the package the test is in. Having `mixengine-platform`
//! dev-depend on this crate would be a dependency cycle to answer a question about where a file
//! sits. What is being tested is still `mixengine_platform::process`, which this crate depends on
//! in the ordinary way.
//!
//! # The oracle is a lock, not a pid
//!
//! Every assertion below is "the lock at this path can now be taken". That is deliberate and it is
//! most of the work of this task. `try_stop` answers a question about a *number* — on Unix `kill`
//! succeeds against a zombie, so a process that has exited and not been reaped still reports as
//! present — and a supervision test that used it would pass for a process that is gone and for one
//! that is merely unreaped alike. A lock is released by the kernel when the process really ends and
//! by nothing else, on both families of system, killed or not. `FakeService::hold_lock` is the
//! fixture side of it.
//!
//! # What each system is allowed to promise
//!
//! `.claude/decisions/0007-supervised-child-owns-a-process-group.md` sets it out, and the last two
//! tests here are that ADR written as code: a **killed** supervisor takes its child down on Windows
//! (the job object, a kernel guarantee) and on Linux (`PR_SET_PDEATHSIG`), and takes nothing down
//! on macOS, where crash recovery at the next boot (roadmap task T18) is what covers it. The macOS
//! test asserts the gap rather than skipping it, so that a day someone closes it is a day a test
//! fails and says so.

use std::path::Path;
use std::time::{Duration, Instant};

use mixengine_platform::lock::{Acquired, Lock};
use mixengine_platform::process::{Supervised, spawn_supervised};
use mixengine_testkit::{FakeService, try_kill, try_stop};

/// How long a test waits for a process on the other side of the machine to do something.
///
/// Only ever waited out in full when something is wrong; every use returns as soon as the state it
/// is waiting for arrives. Generous because process startup on a loaded Windows runner is measured
/// in seconds.
const EVENTUALLY: Duration = Duration::from_secs(20);

/// How long a process is watched for *not* doing something.
///
/// Paid in full every time, so it is short. It only appears in the macOS test, where the claim is
/// that a child goes on running — and no length of wait could prove that outright, so what this
/// buys is the difference between "still there" and "was about to go".
#[cfg(target_os = "macos")]
const FOR_A_WHILE: Duration = Duration::from_secs(2);

#[test]
fn stopping_a_supervised_process_ends_it() {
    let home = tempfile::tempdir().expect("a directory to keep a lock in");
    let lock = home.path().join("service.lock");

    let mut service = supervised(FakeService::new().hold_lock(&lock));
    wait_until_held(&lock);

    let exit = service.stop().expect("a supervised process can be stopped");

    assert!(
        !exit.is_success(),
        "a process that was killed did not end successfully: {exit}"
    );
    assert!(
        is_free(&lock),
        "stop returned while the process it stopped was still holding its lock"
    );
}

#[test]
fn stopping_a_supervised_process_reaches_what_it_started() {
    // The whole reason a group exists. A service is rarely one process — php-fpm has a master and
    // its pool workers, and `mariadbd` is often behind a wrapper script — and stopping only the one
    // we spawned would leave the rest holding the port the next start needs.
    let home = tempfile::tempdir().expect("a directory to keep locks in");
    let service_lock = home.path().join("service.lock");
    let worker_lock = home.path().join("worker.lock");

    let mut service = supervised(
        FakeService::new()
            .hold_lock(&service_lock)
            .child(&worker_lock),
    );

    wait_until_held(&service_lock);
    wait_until_held(&worker_lock);

    service.stop().expect("a supervised process can be stopped");

    assert!(is_free(&service_lock), "the service itself outlived a stop");
    assert!(
        wait_until_free(&worker_lock),
        "the child the service started outlived a stop, so the stop addressed a process rather \
         than a group"
    );
}

#[test]
fn dropping_the_handle_ends_the_process_too() {
    // The claim `Supervised`'s documentation makes, and the one that keeps a supervisor honest: a
    // handle that went out of scope while its processes kept running would be an orphan produced by
    // the module that exists to prevent them. It is also what makes an early `?` in the daemon safe.
    let home = tempfile::tempdir().expect("a directory to keep a lock in");
    let lock = home.path().join("service.lock");

    let service = supervised(FakeService::new().hold_lock(&lock));
    wait_until_held(&lock);

    drop(service);

    assert!(
        wait_until_free(&lock),
        "a dropped handle left its process running"
    );
}

#[test]
fn a_supervisor_that_goes_away_takes_its_child_with_it() {
    // The `fakeservice` here stands in for the daemon: only a separate process can be ended the way
    // a daemon is ended, and the test cannot be that process.
    //
    // The two systems reach the same outcome by different routes, which is worth knowing when this
    // fails on one of them. On Unix `try_stop` is a `SIGTERM`, the fixture honours it, `main`
    // returns, and the `Supervised` it was holding drops — the destructor above, proved from
    // outside. On Windows there is nothing gentler to send from out here, so the supervisor is
    // killed outright and no code of ours runs at all: the job object closes with the process and
    // the kernel does the rest.
    let home = tempfile::tempdir().expect("a directory to keep a lock in");
    let lock = home.path().join("child.lock");

    let supervisor = FakeService::new().supervise(&lock).spawn();
    wait_until_held(&lock);

    assert!(
        try_stop(supervisor.id()),
        "the supervisor was there to be stopped"
    );

    assert!(
        wait_until_free(&lock),
        "the supervised child outlived the process supervising it"
    );
}

/// The case only a kernel can cover: a supervisor that is given no chance to tidy up.
///
/// `SIGKILL`, or a `taskkill /F`. No destructor runs, so everything the test above proved about a
/// graceful exit is beside the point — what is left is the mechanism the operating system provides,
/// and here two of the three have one: `JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE` on Windows, which is
/// total, and `PR_SET_PDEATHSIG` on Linux, which covers the immediate child. macOS has neither and
/// gets the test below instead.
#[cfg(not(target_os = "macos"))]
#[test]
fn a_supervisor_that_is_killed_takes_its_child_with_it() {
    let home = tempfile::tempdir().expect("a directory to keep a lock in");
    let lock = home.path().join("child.lock");

    let supervisor = FakeService::new().supervise(&lock).spawn();
    wait_until_held(&lock);

    assert!(
        try_kill(supervisor.id()),
        "the supervisor was there to be killed"
    );

    assert!(
        wait_until_free(&lock),
        "the supervised child survived its supervisor being killed, which this system has a \
         mechanism to prevent"
    );
}

/// The gap, asserted rather than skipped.
///
/// macOS has no `PR_SET_PDEATHSIG` and no job object, so a supervised child of a killed daemon goes
/// on running and nothing in the child or the kernel notices. That is written down in ADR 0007 and
/// is covered by crash recovery at the next boot (roadmap task T18) rather than by anything here.
///
/// Testing it the other way round — asserting the child survives — is what makes the ADR falsifiable
/// on the machine rather than only in a document: the day macOS gains a mechanism, or the day
/// somebody adds a watchdog, this fails and points at the paragraph that has to change.
#[cfg(target_os = "macos")]
#[test]
fn a_killed_supervisor_on_macos_leaves_its_child_running() {
    let home = tempfile::tempdir().expect("a directory to keep a lock in");
    let lock = home.path().join("child.lock");

    let supervisor = FakeService::new().supervise(&lock).spawn();
    let child = wait_until_held(&lock);

    assert!(
        try_kill(supervisor.id()),
        "the supervisor was there to be killed"
    );

    std::thread::sleep(FOR_A_WHILE);

    let survived = !is_free(&lock);

    // Stopped before the assertion, so a run that fails here still leaves nothing behind: the whole
    // point of this test is a process nothing is going to clean up on its own.
    let _ = try_kill(child);

    assert!(
        survived,
        "the supervised child was taken down with its supervisor — if macOS has grown a mechanism \
         for that, ADR 0007 and the macOS half of the platform layer both need rewriting"
    );
}

/// Start `fixture` supervised, the way the daemon will.
///
/// The working directory is the system temporary directory rather than the one the locks are in,
/// which is `spawn_supervised`'s own warning being taken: a process's working directory is a
/// reference the OS holds for its whole life, so a child parked in the test's `TempDir` would stop
/// that directory from being removed on Windows.
fn supervised(fixture: FakeService) -> Supervised {
    spawn_supervised(
        &FakeService::program(),
        fixture.args(),
        &std::env::temp_dir(),
    )
    .expect("a fakeservice can be supervised")
}

/// Wait until somebody holds the lock at `path`, and say who.
///
/// Reads the pid the holder recorded rather than trying to take the lock, and the difference
/// matters: a test that polled by *acquiring* would be racing the process it is waiting for, and
/// would occasionally take the lock first and make the fixture fail for the wrong reason. A pid in
/// the file means the lock was already held when it was written, because that is the order
/// `Lock::acquire` does the two in.
///
/// # Panics
///
/// If nobody has taken it within [`EVENTUALLY`], which is the fixture failing to start.
fn wait_until_held(path: &Path) -> u32 {
    let deadline = Instant::now() + EVENTUALLY;

    while Instant::now() < deadline {
        if let Ok(contents) = std::fs::read_to_string(path)
            && let Ok(pid) = contents.trim().parse()
        {
            return pid;
        }

        std::thread::sleep(Duration::from_millis(10));
    }

    panic!(
        "nothing took the lock at {} within {EVENTUALLY:?}",
        path.display()
    );
}

/// Wait for the lock at `path` to be released. `false` if it still was not within [`EVENTUALLY`].
///
/// Polled rather than asserted at once because the processes these tests kill are not this process's
/// children on the far side of a group: the OS reaps them when it gets to it, and how long that
/// takes belongs to the machine rather than to the test.
fn wait_until_free(path: &Path) -> bool {
    let deadline = Instant::now() + EVENTUALLY;

    loop {
        if is_free(path) {
            return true;
        }

        if Instant::now() >= deadline {
            return false;
        }

        std::thread::sleep(Duration::from_millis(10));
    }
}

/// Whether the lock at `path` is free right now.
///
/// Taking it is the only way to ask, so this takes it and immediately gives it back — which is safe
/// here because every process that competes for one of these locks in a test has already been
/// stopped or is expected to keep holding it.
fn is_free(path: &Path) -> bool {
    match Lock::acquire(path).expect("this system can be asked about a lock file") {
        Acquired::Held(lock) => {
            drop(lock);
            true
        }
        Acquired::Taken(_) => false,
    }
}
