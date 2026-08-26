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

use std::collections::BTreeMap;
use std::path::Path;
use std::time::{Duration, Instant};

use mixengine_platform::lock::{Acquired, Lock};
use mixengine_platform::process::{Limits, Supervised, spawn_supervised};
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

/// The gap T13 left and T15 owed: a stop has to reach the group even when the leader is gone.
///
/// This is the shape of an ordinary failure, not an exotic one — a php-fpm master crashes and its
/// pool keeps the port; a wrapper script `exec`s into `mariadbd` and dies. The stop that a restart
/// policy then issues used to skip the kill entirely, because the process it *named* had already
/// exited, and the workers it left behind were exactly what the next start collided with.
#[test]
fn stopping_a_service_whose_leader_has_died_still_reaches_its_workers() {
    let home = tempfile::tempdir().expect("a directory to keep locks in");
    let service_lock = home.path().join("service.lock");
    let worker_lock = home.path().join("worker.lock");

    let mut service = supervised(
        FakeService::new()
            .hold_lock(&service_lock)
            .child(&worker_lock),
    );

    let leader = wait_until_held(&service_lock);
    wait_until_held(&worker_lock);

    // The crash. Killed rather than asked, so no destructor of the fixture's runs and the worker is
    // left in the state a supervisor really finds it in.
    assert!(try_kill(leader), "the leader was there to be killed");
    assert!(
        wait_until_free(&service_lock),
        "the leader survived being killed"
    );

    service
        .stop()
        .expect("a service can be stopped after its leader has gone");

    assert!(
        wait_until_free(&worker_lock),
        "the worker outlived a stop issued after its leader had died, so the stop was skipped for \
         the one process that had already ended"
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

/// A service is given its spec's environment and none of the daemon's.
///
/// The claim `spawn_supervised` makes in prose, asked of the child itself — the only place it can be
/// answered. `CARGO_MANIFEST_DIR` stands in for every variable a daemon happens to be holding: this
/// process has one because cargo is running it, and a wholesale inheritance is what would put it in
/// a managed MariaDB.
#[test]
fn a_supervised_child_gets_its_own_environment_and_not_this_process_s() {
    assert!(
        std::env::var_os("CARGO_MANIFEST_DIR").is_some(),
        "this test needs a variable it knows this process has, and cargo sets this one — run it \
         with `cargo test` rather than by path"
    );

    let home = tempfile::tempdir().expect("a directory to write the environment into");
    let dump = home.path().join("env");

    let spec_env = BTreeMap::from([
        ("MYSQL_HOME".to_owned(), "/opt/mixengine/mariadb".to_owned()),
        ("MIXENGINE_SERVICE".to_owned(), "mariadb@main".to_owned()),
    ]);

    let _service = supervised_with(FakeService::new().dump_env(&dump), &spec_env);
    let given = environment_of(&dump);

    assert_eq!(
        given.get("MIXENGINE_SERVICE").map(String::as_str),
        Some("mariadb@main"),
        "the spec's own variables did not reach the child"
    );
    assert_eq!(
        given.get("MYSQL_HOME").map(String::as_str),
        Some("/opt/mixengine/mariadb")
    );
    assert!(
        !given.contains_key("CARGO_MANIFEST_DIR"),
        "the daemon's environment was inherited wholesale, which is what a spec stating its \
         environment in full exists to prevent: {given:?}"
    );
    assert!(
        given.contains_key("PATH"),
        "the floor was not applied — a service that shells out finds nothing without a PATH: \
         {given:?}"
    );
}

/// The floor is a floor: a spec that names one of its variables gets its own value, not ours.
#[test]
fn a_spec_overrides_what_the_floor_would_have_inherited() {
    let home = tempfile::tempdir().expect("a directory to write the environment into");
    let dump = home.path().join("env");

    let spec_env = BTreeMap::from([("PATH".to_owned(), "/opt/mixengine/bin".to_owned())]);

    let _service = supervised_with(FakeService::new().dump_env(&dump), &spec_env);
    let given = environment_of(&dump);

    assert_eq!(
        given.get("PATH").map(String::as_str),
        Some("/opt/mixengine/bin"),
        "the inherited PATH won over the one the spec asked for: {given:?}"
    );
}

/// Asking a group to stop, where a system has a way to ask.
///
/// The polite half of a `StopBehaviour`'s grace period: the fixture honours the request and returns
/// from `main`, so the lock is released by a process that ended on its own rather than by one that
/// was killed. Waits for [`READY_LINE`] first, because a signal that arrives before the fixture has
/// installed its handlers ends it by default disposition and would prove nothing.
#[cfg(unix)]
#[test]
fn a_group_that_is_asked_to_stop_shuts_itself_down() {
    use std::io::{BufRead as _, BufReader};

    let home = tempfile::tempdir().expect("a directory to keep a lock in");
    let lock = home.path().join("service.lock");

    let mut service = supervised(FakeService::new().hold_lock(&lock));
    wait_until_held(&lock);

    let stdout = service.take_stdout().expect("a supervised child is piped");
    let mut lines = BufReader::new(stdout).lines();
    let announced = lines
        .next()
        .expect("the fixture says something before it is asked to stop")
        .expect("its stdout is readable");
    assert_eq!(announced, mixengine_testkit::service::READY_LINE);

    service.ask_to_stop().expect("this system can ask");

    assert!(
        wait_until_free(&lock),
        "the group ignored a request it was built to honour"
    );

    let exit = service.wait().expect("the child can be waited for");
    assert!(
        exit.is_success(),
        "the service was killed rather than allowed to stop itself: {exit}"
    );
}

/// The other half of the same claim, on the system that cannot make it.
///
/// Windows has no signal a daemon can send to a process it gave no console to, and
/// `.claude/decisions/0008-no-signal-stop-on-windows.md` records why the alternatives are worse than
/// saying so. Asserted rather than skipped, exactly as ADR 0007's macOS gap is: the day this becomes
/// possible, this test fails and points at the paragraph that has to change.
#[cfg(windows)]
#[test]
fn a_group_on_windows_cannot_be_asked_to_stop_at_all() {
    use mixengine_platform::Error;

    let home = tempfile::tempdir().expect("a directory to keep a lock in");
    let lock = home.path().join("service.lock");

    let service = supervised(FakeService::new().hold_lock(&lock));
    wait_until_held(&lock);

    // In a `const` block on purpose: the claim is about a constant, so the day it changes should be
    // a build that fails at this line rather than a run that fails after starting a process.
    const {
        assert!(
            !mixengine_platform::process::CAN_ASK_TO_STOP,
            "this system now claims it can ask a group to stop — the supervisor's grace period and \
             ADR 0008 both need revisiting"
        );
    }

    let refused = service
        .ask_to_stop()
        .expect_err("there is no such request on this system");

    assert!(
        matches!(
            &refused,
            Error::UnsupportedPlatform { capability, .. } if capability.contains("stop")
        ),
        "a system with no way to ask has to say so in the typed way: {refused:?}"
    );
}

/// A signal reaches the process that leads the group, and **only** that process.
///
/// **The leader and not the group**, which is what separates this from a stop: a stop is meant for
/// every process holding the port, and a reload is meant for the master, whose whole job is to
/// decide what its workers do about it. So the fixture is given a child, and the child is the
/// assertion: a signal that had gone to the group would have taken it too.
///
/// `SIGHUP` and not `SIGUSR2`, though `SIGUSR2` is what php-fpm reloads on: the fixture handles
/// neither and Rust installs no handler for either, so **the default disposition ends the leader —
/// and that is the delivery, observed**. The claim being made here is about the call and its target,
/// not about what a program does with what it is sent; what php-fpm does with `SIGUSR2` is judged
/// against php-fpm in `crates/mixengine-cli/tests/php_fpm.rs`.
///
/// The leader's lock is *waited* on rather than read once. `kill` returns as soon as the signal is
/// queued, so a check taken immediately after it asks whether the kernel has run the disposition
/// yet — a question whose answer is a race, and which was measured to be answered differently on
/// Linux and macOS.
#[cfg(unix)]
#[test]
fn a_supervised_process_can_be_signalled() {
    use mixengine_platform::process::{CAN_SIGNAL, Signal};

    const {
        assert!(CAN_SIGNAL, "this system has signals");
    }

    let home = tempfile::tempdir().expect("a directory to keep the locks in");
    let leader = home.path().join("service.lock");
    let child = home.path().join("child.lock");

    let mut service = supervised(FakeService::new().hold_lock(&leader).child(&child));
    wait_until_held(&leader);
    wait_until_held(&child);

    service
        .signal(Signal::Hup)
        .expect("a signal to a process this daemon started");

    assert!(
        wait_until_free(&leader),
        "the process this signal named never received it"
    );

    assert!(
        !is_free(&child),
        "the signal reached the whole group rather than the process it named"
    );

    // The group is still there — the child is in it — so this is an ordinary stop and not a question
    // about what a system says when asked to kill a group that has already gone.
    service.stop().expect("the fixture stops");
    assert!(wait_until_free(&child), "the stop left the child behind");
}

/// Windows says so rather than pretending, exactly as it does for `ask_to_stop`.
///
/// In a `const` block for that test's reason: the claim is about a constant, so the day it changes
/// should be a build that fails at this line rather than a run that fails after starting a process.
#[cfg(windows)]
#[test]
fn a_process_on_windows_cannot_be_signalled_at_all() {
    use mixengine_platform::Error;
    use mixengine_platform::process::Signal;

    const {
        assert!(
            !mixengine_platform::process::CAN_SIGNAL,
            "this system now claims it can signal a process — `ReloadBehaviour::Signal` and ADR \
             0008 both need revisiting"
        );
    }

    let home = tempfile::tempdir().expect("a directory to keep a lock in");
    let lock = home.path().join("service.lock");

    let mut service = supervised(FakeService::new().hold_lock(&lock));
    wait_until_held(&lock);

    let refused = service
        .signal(Signal::Usr2)
        .expect_err("there are no signals on this system");

    assert!(
        matches!(
            &refused,
            Error::UnsupportedPlatform { capability, .. } if capability.contains("signal")
        ),
        "a system with no signals has to say so in the typed way: {refused:?}"
    );

    service.stop().expect("the fixture stops");
}

/// Start `fixture` supervised, the way the daemon will.
///
/// The working directory is the system temporary directory rather than the one the locks are in,
/// which is `spawn_supervised`'s own warning being taken: a process's working directory is a
/// reference the OS holds for its whole life, so a child parked in the test's `TempDir` would stop
/// that directory from being removed on Windows.
fn supervised(fixture: FakeService) -> Supervised {
    supervised_with(fixture, &BTreeMap::new())
}

/// Start `fixture` supervised with `env` as its environment.
///
/// Split from [`supervised`] rather than given a default, so that the tests which are not about the
/// environment say `BTreeMap::new()` out loud: an empty map is a claim — *this child gets nothing
/// but the floor* — and not an omission.
fn supervised_with(fixture: FakeService, env: &BTreeMap<String, String>) -> Supervised {
    spawn_supervised(
        &FakeService::program(),
        fixture.args(),
        &std::env::temp_dir(),
        env,
        &Limits::default(),
    )
    .expect("a fakeservice can be supervised")
}

/// The environment a supervised child was actually given, as `NAME=value` pairs.
///
/// # Panics
///
/// If the child has not written it within [`EVENTUALLY`], which is the fixture failing to start.
fn environment_of(path: &Path) -> BTreeMap<String, String> {
    let deadline = Instant::now() + EVENTUALLY;

    while Instant::now() < deadline {
        // Read only once it is non-empty: `dump_env` writes it in one call, but a reader that
        // arrives between the create and the write would otherwise see a file with nothing in it and
        // conclude the child had no environment at all — which is exactly what these tests assert
        // the *absence* of, so the failure would look like a pass in the wrong direction.
        if let Ok(contents) = std::fs::read_to_string(path)
            && !contents.is_empty()
        {
            return contents
                .lines()
                .filter_map(|line| line.split_once('='))
                .map(|(name, value)| (name.to_owned(), value.to_owned()))
                .collect();
        }

        std::thread::sleep(Duration::from_millis(10));
    }

    panic!(
        "no supervised child wrote its environment to {} within {EVENTUALLY:?}",
        path.display()
    );
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
