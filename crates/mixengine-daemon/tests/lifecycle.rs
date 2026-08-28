//! Starting, not starting twice, and stopping — against real `mixengined` processes.
//!
//! These are the tests that cannot be written any other way. A single-instance lock is a claim about
//! two operating-system processes, and a `--detach` that waits for its child is a claim about three;
//! neither means anything asserted inside one process against a mock.
//!
//! Every test gets its own `MIXENGINE_HOME` in a `TempDir` **passed as `--home`** — rule 2 in
//! `.claude/standards/testing.md`. Nothing here touches the network.

use std::process::{Child, Command, Stdio};
use std::time::Duration;

use mixengine_core::config::PathOverrides;
use mixengine_core::{Paths, Store, services};
use mixengine_platform::ipc::{Connection, Endpoint};
use mixengine_platform::lock;
use mixengine_proto::{ServiceId, ServiceState};
use mixengine_testkit::{FakeService, Home, Service, declare, stop};

/// Run `mixengined` against `home` with the given arguments, to completion.
///
/// The home fixture is `mixengine-testkit`'s and knows nothing about this binary: it provides the
/// directory, the endpoint that directory implies, and the waits. Starting the daemon is what stays
/// here, because `CARGO_BIN_EXE_…` reaches binaries of this package alone.
fn run(home: &Home, args: &[&str]) -> std::process::Output {
    Command::new(env!("CARGO_BIN_EXE_mixengined"))
        .args(args)
        .arg("--home")
        .arg(home.path())
        .output()
        .expect("the daemon binary runs")
}

/// Run `mixengined` against `home` with the given arguments, *without* waiting for it.
///
/// Distinct from [`run`], which reads the process to end-of-file: a test about what a command does
/// while it is still running cannot be written against its output.
fn spawn(home: &Home, args: &[&str]) -> Foreground {
    let mut command = Command::new(env!("CARGO_BIN_EXE_mixengined"));

    command
        .args(args)
        .arg("--home")
        .arg(home.path())
        .stdout(Stdio::null())
        .stderr(Stdio::null());

    Foreground(command.spawn().expect("the daemon binary runs"))
}

/// Start one in the foreground, as a service manager would, and wait until it answers.
async fn start(home: &Home) -> Foreground {
    let daemon = spawn(home, &[]);
    home.wait_until_listening().await;
    daemon
}

/// A process this test started and is still holding — a daemon in the foreground, or a `--detach`
/// waiting for one.
///
/// Killed rather than stopped on the way out, because a test that fails halfway must not leave a
/// process holding a `TempDir` open. The graceful path is what the tests below exercise deliberately.
struct Foreground(Child);

impl Foreground {
    /// Whether it is still going, without waiting for it either way.
    fn still_running(&mut self) -> bool {
        self.0
            .try_wait()
            .expect("this system can be asked about a process it started")
            .is_none()
    }
}

impl Drop for Foreground {
    fn drop(&mut self) {
        let _ = self.0.kill();
        let _ = self.0.wait();
    }
}

#[tokio::test]
async fn the_fixture_and_the_daemon_agree_on_the_paths_it_restates() {
    // `mixengine-testkit` restates four things `mixengine_core::Paths` owns — that `run/` is
    // directly under the root, what the lock file inside it is called, where the daemon's own log
    // goes, and which file the database is — because it is linked into test binaries that have no
    // business bundling SQLite to find a socket. This is the one place both answers exist at once,
    // so it is where they are held together. Every other test in this file rests on them being the
    // same, and would fail confusingly rather than clearly.
    //
    // The log is the one that needs this most. `Paths::new` refuses to let a `[paths]` override move
    // `run/`, so the first two answers cannot drift without somebody deciding they should; `logs/`
    // has no such guard, and a fixture reading the wrong file would turn every
    // `wait_until_daemon_log_says` into a thirty-second timeout blaming the daemon.
    let home = Home::new();
    let paths = Paths::new(home.path().to_owned(), &PathOverrides::default());

    assert_eq!(home.run_dir(), paths.run());
    assert_eq!(home.lock_file(), paths.lock_file());
    assert_eq!(home.daemon_log_file(), paths.daemon_log_file());
    assert_eq!(home.database_file(), paths.database_file());
    assert_eq!(
        home.endpoint().to_string(),
        Endpoint::in_run_dir(paths.run())
            .expect("an endpoint for this home")
            .to_string()
    );
}

#[tokio::test]
async fn a_daemon_records_the_pid_that_holds_its_home() {
    let home = Home::new();
    let daemon = start(&home).await;

    assert_eq!(
        home.locked_by(),
        Some(daemon.0.id()),
        "the lock file names the daemon that is running, so a second one can say who has the home"
    );
}

#[tokio::test]
async fn a_second_daemon_prints_the_endpoint_and_exits_successfully() {
    let home = Home::new();
    let _daemon = start(&home).await;

    let second = run(&home, &[]);

    // Not a failure: the caller asked for a running daemon for this home, and there is one.
    assert!(
        second.status.success(),
        "a second daemon exited with {} — {}",
        second.status,
        String::from_utf8_lossy(&second.stderr)
    );
    assert_eq!(
        String::from_utf8_lossy(&second.stdout).trim(),
        home.endpoint().to_string(),
        "it prints where the daemon that is running can be reached"
    );

    // And it left the first one alone, which is the half of this that would be worth catching: a
    // second daemon that bound anything, unlinked anything or migrated anything would show up here.
    assert!(
        Connection::connect(home.endpoint()).await.is_ok(),
        "the daemon that was already running is still listening"
    );
}

#[tokio::test]
async fn detaching_returns_only_once_the_daemon_answers() {
    let home = Home::new();

    let detached = run(&home, &["--detach"]);

    assert!(
        detached.status.success(),
        "--detach exited with {} — {}",
        detached.status,
        String::from_utf8_lossy(&detached.stderr)
    );
    assert_eq!(
        String::from_utf8_lossy(&detached.stdout).trim(),
        home.endpoint().to_string(),
        "it prints the endpoint the daemon it started is listening on"
    );

    // No polling, deliberately. The contract is that a client which gets a zero exit status can
    // connect *now*, rather than retrying against a daemon that may still be migrating a database.
    let connected = Connection::connect(home.endpoint()).await;
    let pid = home
        .locked_by()
        .expect("the detached daemon recorded its pid");

    // Stopped before anything is asserted, so a failing assertion cannot leave a daemon running.
    stop(pid);
    home.wait_until_gone().await;

    assert!(
        connected.is_ok(),
        "the daemon was listening the moment --detach returned"
    );
    assert_ne!(
        pid,
        std::process::id(),
        "the daemon is a process of its own and not this one"
    );
}

#[tokio::test]
async fn detaching_leaves_the_daemon_that_is_already_running_alone() {
    // The common case for the caller `--detach` exists for: a client autostarts a daemon whenever it
    // cannot reach the endpoint, and two clients doing that at once means the second one arrives to
    // a daemon that is already up. It has to answer with the endpoint and change nothing.
    let home = Home::new();
    let daemon = start(&home).await;

    let detached = run(&home, &["--detach"]);

    assert!(
        detached.status.success(),
        "--detach exited with {} — {}",
        detached.status,
        String::from_utf8_lossy(&detached.stderr)
    );
    assert_eq!(
        String::from_utf8_lossy(&detached.stdout).trim(),
        home.endpoint().to_string(),
        "it prints where the daemon that is running can be reached"
    );
    assert_eq!(
        home.locked_by(),
        Some(daemon.0.id()),
        "the daemon that was already running still holds the home, so nothing took it over"
    );
}

#[tokio::test]
async fn a_detached_daemon_does_not_hold_the_directory_it_was_started_from() {
    // A working directory is a reference the OS keeps for the life of the process, and the directory
    // a client autostarting a daemon happens to be in is a project folder somebody is working in.
    // Windows is where this bites — a folder that is some process's working directory cannot be
    // renamed or deleted — so on Unix this passes whatever the daemon inherited. The assertion is
    // worth making on both: it is the same claim either way, and only one of the two OSes will
    // notice when it stops being true.
    let home = Home::new();
    let project = tempfile::tempdir().expect("a project directory to start from");

    let detached = Command::new(env!("CARGO_BIN_EXE_mixengined"))
        .arg("--detach")
        .arg("--home")
        .arg(home.path())
        .current_dir(project.path())
        .output()
        .expect("the daemon binary runs");

    assert!(
        detached.status.success(),
        "--detach exited with {} — {}",
        detached.status,
        String::from_utf8_lossy(&detached.stderr)
    );

    let pid = home
        .locked_by()
        .expect("the detached daemon recorded its pid");

    let removed = std::fs::remove_dir_all(project.path());

    // Stopped before the assertion, so a failure cannot leave a daemon running.
    stop(pid);
    home.wait_until_gone().await;

    removed.expect("the directory --detach was started from can be removed while the daemon runs");
}

#[tokio::test]
async fn detaching_keeps_waiting_when_its_child_stood_aside_for_a_daemon_still_starting_up() {
    // The window this is about is real and narrow, and racing two `--detach` starts does not open it
    // reliably: it lasts only as long as the winner spends between taking the lock and binding the
    // endpoint, which is `Store::open` and is usually shorter than the time it takes the loser to
    // start at all. So it is *held* open here instead — this test takes the lock and never binds
    // anything, which is exactly the state the winner is in mid-migration.
    //
    // What used to happen: the child finds the lock taken, prints the endpoint and exits 0; the
    // parent sees an exit, retries the endpoint once, and calls the whole thing a failure. Two
    // clients autostarting at the same instant (roadmap task T10) would have had one of them fail.
    let home = Home::new();

    std::fs::create_dir_all(home.run_dir()).expect("a run directory to lock in");

    let held = match lock::Lock::acquire(&home.lock_file()).expect("the lock can be asked for") {
        lock::Acquired::Held(held) => held,
        lock::Acquired::Taken(holder) => panic!("a brand new home was already locked by {holder}"),
    };

    let mut detaching = spawn(&home, &["--detach"]);

    // Waited for rather than slept past: how long the child takes to start varies by more than a
    // second on Windows, and a test that guessed would be measuring Defender rather than the daemon.
    // This line is the child's last act before it exits, so the parent meets the case under test on
    // its very next poll.
    home.wait_until_daemon_log_says("a daemon is already running")
        .await;

    // Twenty turns of the parent's 50ms poll, and a fraction of its 30s ceiling: a parent still going
    // after this is one that is genuinely waiting, not one that has not got round to failing yet.
    //
    // **Slept and not polled, because what is asserted is that nothing happened.** There is no
    // condition to wait for here — the evidence is a whole second in which the parent did not give
    // up — so this is the one shape a deadline loop cannot replace. It is also the shape a slow
    // machine cannot break: the only thing that fails this assertion is the parent giving up
    // *early*, and a busy runner delays every poll it makes, which is more evidence and not less.
    tokio::time::sleep(Duration::from_millis(1000)).await;

    let waiting = detaching.still_running();

    // Released before the assertion, so a failure cannot leave a locked `TempDir` behind on Windows.
    drop(detaching);
    drop(held);

    assert!(
        waiting,
        "--detach gave up while the daemon holding {} had not begun listening yet",
        home.lock_file().display()
    );
}

/// **Milestone M1, end to end**: kill a daemon mid-run; the next one adopts what survived and cleans
/// what did not.
///
/// Everything under this has been proved in the crates it belongs to — the platform layer identifies
/// a process by pid *and* start time, the registry reconciles a table of rows — and what has never
/// been proved is that a `mixengined` process does any of it before it serves its first client. That
/// claim spans two daemons, two services and a database, so nothing here is mocked.
///
/// **The two services are this test's own children**, and that is not a shortcut. What a killed
/// daemon leaves behind differs by system — everything dies on Windows, the immediate child dies on
/// Linux, nothing dies on macOS (ADR 0007) — so a test that produced its survivor by killing a
/// daemon would assert three different things and prove the recovery on only one of them. Started
/// here, both cases exist on every system, and neither reaches the code under test as anything but
/// a row: a pid, and the moment the process bearing it began.
///
/// The services are created through `service.create` and the pid columns written by
/// `mixengine_testkit::declare::running`, which is the only way to produce the state
/// this is about: a daemon that is *running* writes those columns and clears them on its way out,
/// whichever way it is asked to stop. What is left when it is given no way out at all is what this
/// hands the second daemon.
#[tokio::test]
#[cfg_attr(
    not(debug_assertions),
    ignore = "the fakeservice recipe is compiled into debug builds only"
)]
async fn a_daemon_adopts_what_outlived_the_last_one_and_clears_what_did_not() {
    let home = Home::new();

    // The schema is created when a daemon opens the home, so there is nothing to write a row into
    // until one has. This is also the daemon the two services below are recorded as belonging to.
    let first = start(&home).await;

    // The async half of `Home::declare`, which builds a runtime of its own and cannot be called
    // from inside this one. The row *is* the declaration since T30, so this is both halves of what
    // the second daemon meets: a service it can supervise, and a claim about a process.
    mixengine_testkit::create(
        home.endpoint(),
        &home.database_file(),
        &[
            Service::new("fakeservice@kept"),
            Service::new("fakeservice@lost"),
        ],
    )
    .await;

    let mut survivor = started(FakeService::new());
    let mut casualty = started(FakeService::new());

    declare::running(
        &home.database_file(),
        "fakeservice@kept",
        survivor.id(),
        began(survivor.id()),
    )
    .await;
    declare::running(
        &home.database_file(),
        "fakeservice@lost",
        casualty.id(),
        began(casualty.id()),
    )
    .await;

    // One service goes with its daemon and one does not, which is the pair the next daemon has to
    // tell apart. The casualty is waited for rather than assumed gone: on Unix it is this process's
    // child and stays a zombie until something reaps it, which is the harder of the two states.
    assert!(
        mixengine_testkit::try_kill(casualty.id()),
        "the casualty was there to be killed"
    );
    assert!(ended(&mut casualty), "the casualty survived being killed");

    // Killed rather than stopped: a daemon that exits runs its destructors, and this test is about
    // the one that is given no chance to.
    drop(first);
    home.wait_until_gone().await;

    let second = start(&home).await;
    home.wait_until_daemon_log_says("reconciled what the last daemon left behind")
        .await;

    let kept = record(&home, "fakeservice@kept").await;
    assert_eq!(
        kept.state,
        ServiceState::Running,
        "the service whose process survived was not left where it was"
    );
    assert_eq!(
        kept.pid,
        Some(survivor.id()),
        "the row names a process other than the one that survived"
    );
    assert!(
        survivor.still_running(),
        "the daemon stopped a process it was supposed to take over"
    );

    let lost = record(&home, "fakeservice@lost").await;
    assert_eq!(
        lost.state,
        ServiceState::Stopped,
        "a service whose process did not survive is not still running"
    );
    assert_eq!(
        lost.pid, None,
        "a row that kept a dead pid is one the next daemon would adopt, and by then it is somebody \
         else's"
    );

    // Stopped here rather than left to the fixture's destructor, so that what this test leaves
    // behind is nothing at all — including on macOS, where the daemon being killed takes nothing
    // with it.
    stop(second.0.id());
    home.wait_until_gone().await;
    drop(survivor);
}

/// A `fakeservice` that has started and announced itself.
///
/// Waited for, because a spawn returns before the process has parsed its arguments — and a start
/// time read in that window would identify the right process for the wrong reason.
fn started(fake: FakeService) -> mixengine_testkit::service::Running {
    let service = fake.spawn();

    assert!(
        service.wait_for_stdout(
            mixengine_testkit::service::READY_LINE,
            Duration::from_secs(20)
        ),
        "a fakeservice did not start"
    );

    service
}

/// When the process with this id began, as `services.pid_start_time` holds it.
fn began(pid: u32) -> i64 {
    mixengine_platform::process::started_at(pid)
        .expect("this system can be asked when a process began")
        .expect("the process is running")
        .stored()
}

/// Wait for a process this test started to have ended, and reap it. `false` if it had not.
fn ended(service: &mut mixengine_testkit::service::Running) -> bool {
    let deadline = std::time::Instant::now() + Duration::from_secs(20);

    while service.still_running() {
        if std::time::Instant::now() >= deadline {
            return false;
        }

        std::thread::sleep(Duration::from_millis(10));
    }

    true
}

/// What the database says about one service, read from outside the daemon that owns it.
async fn record(home: &Home, id: &str) -> services::ServiceRecord {
    let store = Store::open(&home.database_file())
        .await
        .expect("the daemon's database can be read while it runs");

    let record = services::record(&store, &ServiceId::parse(id).expect("a valid service id"))
        .await
        .unwrap_or_else(|error| panic!("a row for `{id}`: {error}"));

    store.close().await;

    record
}

#[tokio::test]
async fn the_home_can_be_taken_over_once_the_daemon_holding_it_stops() {
    // The other half of the lock: it has to be *released*, and by a process that was killed rather
    // than asked nicely — which is the case a pid file could not survive and an open handle does.
    let home = Home::new();

    let first = start(&home).await;
    let pid = first.0.id();
    drop(first); // kills it outright, without a chance to clean anything up

    home.wait_until_gone().await;

    let second = start(&home).await;

    assert_ne!(home.locked_by(), Some(pid));
    assert_eq!(
        home.locked_by(),
        Some(second.0.id()),
        "the next daemon took the home over"
    );
}
