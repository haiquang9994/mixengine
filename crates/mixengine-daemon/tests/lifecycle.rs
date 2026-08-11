//! Starting, not starting twice, and stopping — against real `mixengined` processes.
//!
//! These are the tests that cannot be written any other way. A single-instance lock is a claim about
//! two operating-system processes, and a `--detach` that waits for its child is a claim about three;
//! neither means anything asserted inside one process against a mock.
//!
//! Every test gets its own `MIXENGINE_HOME` in a `TempDir` **passed as `--home`** — rule 2 in
//! `.claude/standards/testing.md`. Nothing here touches the network.

use std::path::Path;
use std::process::{Child, Command, Stdio};
use std::time::Duration;

use mixengine_core::Paths;
use mixengine_core::config::PathOverrides;
use mixengine_platform::ipc::{Connection, Endpoint};
use mixengine_platform::lock;
use tempfile::TempDir;

/// How long a freshly spawned daemon is given to bind its endpoint.
///
/// Generous, because the first start of a daemon creates its home, runs the migrations and opens
/// SQLite — and because a loaded CI runner is the machine this has to be reliable on.
const STARTUP: Duration = Duration::from_secs(30);

/// How long a daemon that has been asked to stop is given to disappear.
///
/// Longer than the daemon's own two-second grace period for open connections, so that a test which
/// fails here is reporting a daemon that did not stop rather than one that was merely tidy about it.
const SHUTDOWN: Duration = Duration::from_secs(10);

/// A home directory that exists only for this test, with the paths of the daemon that will own it.
struct Home {
    dir: TempDir,
    paths: Paths,
    endpoint: Endpoint,
}

impl Home {
    fn new() -> Self {
        let dir = tempfile::tempdir().expect("a temporary home");
        let paths = Paths::new(dir.path().to_owned(), &PathOverrides::default());
        let endpoint = Endpoint::in_run_dir(paths.run()).expect("an endpoint for this home");

        Self {
            dir,
            paths,
            endpoint,
        }
    }

    fn path(&self) -> &Path {
        self.dir.path()
    }

    /// The pid the daemon holding this home recorded in its lock file.
    fn locked_by(&self) -> Option<u32> {
        std::fs::read_to_string(self.paths.lock_file())
            .ok()?
            .trim()
            .parse()
            .ok()
    }

    /// Whatever the daemon wrote to its own log, for a failure message.
    fn log(&self) -> String {
        std::fs::read_to_string(self.paths.daemon_log_file())
            .unwrap_or_else(|error| format!("(unreadable: {error})"))
    }

    /// Run `mixengined` against this home with the given arguments, to completion.
    fn run(&self, args: &[&str]) -> std::process::Output {
        Command::new(env!("CARGO_BIN_EXE_mixengined"))
            .args(args)
            .arg("--home")
            .arg(self.path())
            .output()
            .expect("the daemon binary runs")
    }

    /// Run `mixengined` against this home with the given arguments, *without* waiting for it.
    ///
    /// Distinct from [`run`](Self::run), which reads the process to end-of-file: a test about what a
    /// command does while it is still running cannot be written against its output.
    fn spawn(&self, args: &[&str]) -> Foreground {
        let child = Command::new(env!("CARGO_BIN_EXE_mixengined"))
            .args(args)
            .arg("--home")
            .arg(self.path())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .spawn()
            .expect("the daemon binary runs");

        Foreground(child)
    }

    /// Start one in the foreground, as a service manager would, and wait until it answers.
    async fn start(&self) -> Foreground {
        let child = Command::new(env!("CARGO_BIN_EXE_mixengined"))
            .arg("--home")
            .arg(self.path())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .spawn()
            .expect("the daemon binary runs");

        let daemon = Foreground(child);
        self.wait_until_listening().await;
        daemon
    }

    /// Poll the endpoint until something is behind it.
    async fn wait_until_listening(&self) {
        let deadline = tokio::time::Instant::now() + STARTUP;

        while tokio::time::Instant::now() < deadline {
            if Connection::connect(&self.endpoint).await.is_ok() {
                return;
            }

            tokio::time::sleep(Duration::from_millis(25)).await;
        }

        panic!(
            "the daemon did not start listening on {} within {STARTUP:?}\n--- daemon.log ---\n{}",
            self.endpoint,
            self.log()
        );
    }

    /// Poll `daemon.log` until a daemon has written the line that says it got somewhere.
    ///
    /// The one signal available for a daemon that deliberately never binds anything: its endpoint
    /// says nothing about it, and the lock file names whoever *holds* the lock rather than whoever
    /// just gave up on it.
    async fn wait_until_log_says(&self, wanted: &str) {
        let deadline = tokio::time::Instant::now() + STARTUP;

        while tokio::time::Instant::now() < deadline {
            if self.log().contains(wanted) {
                return;
            }

            tokio::time::sleep(Duration::from_millis(25)).await;
        }

        panic!(
            "no daemon said {wanted:?} within {STARTUP:?}\n--- daemon.log ---\n{}",
            self.log()
        );
    }

    /// Poll the endpoint until nothing is behind it any more.
    async fn wait_until_gone(&self) {
        let deadline = tokio::time::Instant::now() + SHUTDOWN;

        while tokio::time::Instant::now() < deadline {
            if Connection::connect(&self.endpoint).await.is_err() {
                return;
            }

            tokio::time::sleep(Duration::from_millis(25)).await;
        }

        panic!(
            "{} was still answering {SHUTDOWN:?} after the daemon was asked to stop\
             \n--- daemon.log ---\n{}",
            self.endpoint,
            self.log()
        );
    }
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

/// Ask the process with this id to stop, the way this operating system asks.
///
/// The one `#[cfg]` outside `mixengine-platform` in this crate, and it is here rather than in the
/// platform layer because nothing in the product stops a process *by pid* yet: that arrives with the
/// supervisor in Phase 1 (roadmap task T15), which is where this belongs once it exists. A test
/// cannot wait for it — the thing being proved is precisely that a daemon somebody else stops shuts
/// down properly.
///
/// Unix gets `SIGTERM`, which is the graceful path. Windows gets `taskkill /F`, which is not: a
/// process started with `DETACHED_PROCESS` has no console for a control event to be delivered
/// through, so there is nothing gentler to send it from here. What both prove is the part that must
/// hold either way — the endpoint stops answering and the lock is released.
fn stop(pid: u32) {
    #[cfg(unix)]
    let mut command = {
        let mut command = Command::new("kill");
        command.arg(pid.to_string());
        command
    };

    #[cfg(windows)]
    let mut command = {
        let mut command = Command::new("taskkill");
        command.args(["/PID", &pid.to_string(), "/F"]);
        command
    };

    let stopped = command
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status()
        .expect("this system can stop a process");

    assert!(stopped.success(), "pid {pid} could not be stopped");
}

#[tokio::test]
async fn a_daemon_records_the_pid_that_holds_its_home() {
    let home = Home::new();
    let daemon = home.start().await;

    assert_eq!(
        home.locked_by(),
        Some(daemon.0.id()),
        "the lock file names the daemon that is running, so a second one can say who has the home"
    );
}

#[tokio::test]
async fn a_second_daemon_prints_the_endpoint_and_exits_successfully() {
    let home = Home::new();
    let _daemon = home.start().await;

    let second = home.run(&[]);

    // Not a failure: the caller asked for a running daemon for this home, and there is one.
    assert!(
        second.status.success(),
        "a second daemon exited with {} — {}",
        second.status,
        String::from_utf8_lossy(&second.stderr)
    );
    assert_eq!(
        String::from_utf8_lossy(&second.stdout).trim(),
        home.endpoint.to_string(),
        "it prints where the daemon that is running can be reached"
    );

    // And it left the first one alone, which is the half of this that would be worth catching: a
    // second daemon that bound anything, unlinked anything or migrated anything would show up here.
    assert!(
        Connection::connect(&home.endpoint).await.is_ok(),
        "the daemon that was already running is still listening"
    );
}

#[tokio::test]
async fn detaching_returns_only_once_the_daemon_answers() {
    let home = Home::new();

    let detached = home.run(&["--detach"]);

    assert!(
        detached.status.success(),
        "--detach exited with {} — {}",
        detached.status,
        String::from_utf8_lossy(&detached.stderr)
    );
    assert_eq!(
        String::from_utf8_lossy(&detached.stdout).trim(),
        home.endpoint.to_string(),
        "it prints the endpoint the daemon it started is listening on"
    );

    // No polling, deliberately. The contract is that a client which gets a zero exit status can
    // connect *now*, rather than retrying against a daemon that may still be migrating a database.
    let connected = Connection::connect(&home.endpoint).await;
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
    let daemon = home.start().await;

    let detached = home.run(&["--detach"]);

    assert!(
        detached.status.success(),
        "--detach exited with {} — {}",
        detached.status,
        String::from_utf8_lossy(&detached.stderr)
    );
    assert_eq!(
        String::from_utf8_lossy(&detached.stdout).trim(),
        home.endpoint.to_string(),
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

    std::fs::create_dir_all(home.paths.run()).expect("a run directory to lock in");

    let held = match lock::Lock::acquire(home.paths.lock_file()).expect("the lock can be asked for")
    {
        lock::Acquired::Held(held) => held,
        lock::Acquired::Taken(holder) => panic!("a brand new home was already locked by {holder}"),
    };

    let mut detaching = home.spawn(&["--detach"]);

    // Waited for rather than slept past: how long the child takes to start varies by more than a
    // second on Windows, and a test that guessed would be measuring Defender rather than the daemon.
    // This line is the child's last act before it exits, so the parent meets the case under test on
    // its very next poll.
    home.wait_until_log_says("a daemon is already running")
        .await;

    // Twenty turns of the parent's 50ms poll, and a fraction of its 30s ceiling: a parent still going
    // after this is one that is genuinely waiting, not one that has not got round to failing yet.
    tokio::time::sleep(Duration::from_millis(1000)).await;

    let waiting = detaching.still_running();

    // Released before the assertion, so a failure cannot leave a locked `TempDir` behind on Windows.
    drop(detaching);
    drop(held);

    assert!(
        waiting,
        "--detach gave up while the daemon holding {} had not begun listening yet",
        home.paths.lock_file().display()
    );
}

#[tokio::test]
async fn the_home_can_be_taken_over_once_the_daemon_holding_it_stops() {
    // The other half of the lock: it has to be *released*, and by a process that was killed rather
    // than asked nicely — which is the case a pid file could not survive and an open handle does.
    let home = Home::new();

    let first = home.start().await;
    let pid = first.0.id();
    drop(first); // kills it outright, without a chance to clean anything up

    home.wait_until_gone().await;

    let second = home.start().await;

    assert_ne!(home.locked_by(), Some(pid));
    assert_eq!(
        home.locked_by(),
        Some(second.0.id()),
        "the next daemon took the home over"
    );
}
