//! `fakeservice` — the program supervision is tested against.
//!
//! `.claude/architecture/process-supervision.md` names it and says what it has to be able to do:
//! start slowly, never become ready, exit with a code after N ms, ignore a request to stop, or leave
//! a child behind that outlives it. Every one of those is a supervisor policy that only a real,
//! badly behaved process can exercise — and none of them should be exercised against real MariaDB,
//! which is slow and hides the races this is meant to expose.
//!
//! This module is the *caller's* half. The program itself is `src/bin/fakeservice.rs`, and the two
//! are deliberately separate: a supervisor test does not spawn it, it hands
//! [`program`](FakeService::program) and [`args`](FakeService::args) to a `ServiceSpec` and lets the
//! supervisor do the spawning. [`FakeService::spawn`] exists for the tests that are about the
//! fixture itself.

use std::ffi::OsString;
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Output, Stdio};
use std::sync::{Arc, Mutex};
use std::thread::JoinHandle;
use std::time::{Duration, Instant};

/// How `fakeservice` should misbehave this time.
///
/// Every option is off by default: a bare `FakeService::new()` starts, announces itself ready
/// immediately, and then does nothing until it is asked to stop — which is what a well-behaved
/// service looks like and is the baseline the misbehaving ones are compared against.
#[derive(Debug, Clone, Default)]
pub struct FakeService {
    args: Vec<OsString>,
}

/// What [`FakeService`] prints on its standard output once it considers itself ready.
///
/// A `ReadyCheck::LogPattern` in a test can match on this, and a test about a service that never
/// becomes ready can assert its absence.
///
/// It is also the only thing a test outside the process can see that says its *stop handlers are
/// installed*: the program registers them before it enters the loop that writes this line. A test
/// that signals a service it has just spawned must wait for it — see
/// [`Running::wait_for_stdout`].
pub const READY_LINE: &str = "fakeservice: ready";

impl FakeService {
    /// A service that behaves.
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// Take this long to announce itself ready.
    ///
    /// The slow-start case: MariaDB initialising a data directory is minutes of a process that is
    /// running and cannot be talked to yet.
    #[must_use]
    pub fn ready_after(self, millis: u64) -> Self {
        self.arg("--ready-after").arg(millis.to_string())
    }

    /// Never announce itself ready, however long it is given.
    ///
    /// Distinct from a very long [`ready_after`](Self::ready_after): what a ready *timeout* has to
    /// do is give up on a process that is otherwise perfectly healthy.
    #[must_use]
    pub fn never_ready(self) -> Self {
        self.arg("--never-ready")
    }

    /// Exit on its own after this long, with the status given to [`exit_code`](Self::exit_code).
    #[must_use]
    pub fn exit_after(self, millis: u64) -> Self {
        self.arg("--exit-after").arg(millis.to_string())
    }

    /// The status to exit with. Defaults to 0, which is a service that *stopped* rather than
    /// crashed — the distinction `RestartPolicy::OnFailure` turns on.
    #[must_use]
    pub fn exit_code(self, code: i32) -> Self {
        self.arg("--exit-code").arg(code.to_string())
    }

    /// Install the stop handlers and then ignore them.
    ///
    /// The process has to be killed. On Unix that is `SIGTERM` going unanswered until `SIGKILL`
    /// arrives, which is the grace period a `StopBehaviour` has to enforce; on Windows a console
    /// control event goes the same way.
    #[must_use]
    pub fn ignoring_stop(self) -> Self {
        self.arg("--ignore-stop")
    }

    /// Write its own pid to this path as soon as it starts.
    ///
    /// For the tests that have to find a process they are not the parent of — adoption after a
    /// daemon restart, and anything asserting that a process is *gone*.
    #[must_use]
    pub fn pid_file(self, path: impl AsRef<Path>) -> Self {
        self.arg("--pid-file").arg(path.as_ref())
    }

    /// Write every environment variable it was given to this path, one `NAME=value` per line.
    ///
    /// What a supervised child's environment actually is can only be answered from inside it, and it
    /// is a claim `spawn_supervised` makes in prose: the spec's variables, plus a short per-OS floor,
    /// and nothing else this process happened to be holding. A file rather than the log, because the
    /// test reading it is usually also waiting on [`READY_LINE`].
    #[must_use]
    pub fn dump_env(self, path: impl AsRef<Path>) -> Self {
        self.arg("--dump-env").arg(path.as_ref())
    }

    /// Spawn a child that outlives it, recording that child's pid at this path.
    ///
    /// The orphan case, and the one worth being careful about in a test: on Windows a Job Object
    /// with `JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE` takes the child down with its parent, which is
    /// exactly the behaviour the supervisor is being asked to guarantee — so a test that spawns this
    /// without a supervisor must stop the orphan itself.
    #[must_use]
    pub fn orphan(self, pid_file: impl AsRef<Path>) -> Self {
        self.arg("--orphan").arg(pid_file.as_ref())
    }

    /// Hold an exclusive lock on this path for as long as the process lives.
    ///
    /// **The way a test asks whether a process is really gone.** A pid answers a question about a
    /// number — and on Unix goes on answering yes for a process that has exited and not been reaped
    /// — while a lock is released by the kernel when the process ends and by nothing else. So
    /// `Lock::acquire` succeeding is proof, and it is the assertion roadmap task T13 rests on, where
    /// [`try_stop`](crate::try_stop) would have proved nothing.
    ///
    /// The holder's pid is recorded in the lock file by the lock itself, so a test can find the
    /// process without a second file.
    #[must_use]
    pub fn hold_lock(self, path: impl AsRef<Path>) -> Self {
        self.arg("--hold-lock").arg(path.as_ref())
    }

    /// Own a supervised child that holds a lock on this path.
    ///
    /// This is the fixture standing in for the daemon: the child is started through
    /// `mixengine_platform::process::spawn_supervised`, so it is in a job object or a session of its
    /// own, and the handle owning it lives as long as this process does. Ending this process
    /// *gracefully* drops that handle and takes the child with it; killing this process is the case
    /// that separates the three platforms, and
    /// `.claude/decisions/0007-supervised-child-owns-a-process-group.md` says which does what.
    #[must_use]
    pub fn supervise(self, lock: impl AsRef<Path>) -> Self {
        self.arg("--supervise").arg(lock.as_ref())
    }

    /// Start an ordinary child that holds a lock on this path, and forget about it.
    ///
    /// The grandchild in "stopping a service stops what the service started": it does nothing to
    /// leave the job or the process group it was born into, which is what a php-fpm worker looks
    /// like from outside. Distinct from [`orphan`](Self::orphan), which deliberately *does* leave —
    /// and on Unix therefore survives a group being killed.
    #[must_use]
    pub fn child(self, lock: impl AsRef<Path>) -> Self {
        self.arg("--child").arg(lock.as_ref())
    }

    /// Write a numbered line to stdout this often, for the log capture to capture.
    #[must_use]
    pub fn log_every(self, millis: u64) -> Self {
        self.arg("--log-every").arg(millis.to_string())
    }

    /// Write those lines to stderr as well, so a test can tell the two streams apart.
    #[must_use]
    pub fn log_to_stderr(self) -> Self {
        self.arg("--log-to-stderr")
    }

    /// The program a `ServiceSpec` should name.
    ///
    /// # Panics
    ///
    /// If `fakeservice` has not been built. It is a binary of *this* package, and
    /// `CARGO_BIN_EXE_…` only reaches binaries of the package the test itself is in — so it is
    /// found next to the test binary instead, which is where `cargo test --workspace` puts both.
    #[must_use]
    pub fn program() -> PathBuf {
        let name = format!("fakeservice{}", std::env::consts::EXE_SUFFIX);
        let test = std::env::current_exe().expect("this test binary has a path");
        let directory = test.parent().expect("this test binary is in a directory");

        // `target/<profile>/deps/` first, then `target/<profile>/` above it: cargo builds
        // integration tests into the former and binaries into the latter, and a cargo that changes
        // its mind about either should still find this.
        let beside = directory.join(&name);
        if beside.is_file() {
            return beside;
        }

        let above = directory
            .parent()
            .expect("the deps directory is inside the profile directory")
            .join(&name);

        assert!(
            above.is_file(),
            "{} is not there — supervision tests drive a real process, so run \
             `cargo test --workspace` rather than `cargo test -p <one crate>`",
            above.display()
        );

        above
    }

    /// The arguments that go with [`program`](Self::program).
    #[must_use]
    pub fn args(&self) -> &[OsString] {
        &self.args
    }

    /// Start it, with both streams piped and being read from the moment it exists.
    ///
    /// The reading is why there are two threads here rather than a `wait_with_output` at the end,
    /// and the difference is not academic. `wait_with_output` drains the pipes, but only from the
    /// moment it is called; a test that *holds* a [`Running`] — polling
    /// [`still_running`](Running::still_running), waiting on a supervisor, doing anything at all
    /// before it finishes — is nobody draining them until then. A pipe holds tens of kilobytes
    /// before a write to it blocks, and the whole point of [`log_every`](Self::log_every) is a
    /// fixture that keeps writing: past that point the service stops on its next line and never
    /// reaches its [`exit_after`](Self::exit_after), which reads as a supervisor bug that is not
    /// one. Draining from the start makes the buffer's size stop mattering.
    ///
    /// # Panics
    ///
    /// If the binary cannot be started at all.
    #[must_use]
    pub fn spawn(&self) -> Running {
        let mut child = Command::new(Self::program())
            .args(&self.args)
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .expect("the fakeservice binary runs");

        let stdout = child.stdout.take().expect("stdout was piped above");
        let stderr = child.stderr.take().expect("stderr was piped above");

        Running {
            child: Some(child),
            drained: Some(Drained {
                stdout: drain(stdout),
                stderr: drain(stderr),
            }),
        }
    }

    fn arg(mut self, arg: impl Into<OsString>) -> Self {
        self.args.push(arg.into());
        self
    }
}

/// A `fakeservice` a test started and is holding.
///
/// Killed when it drops, so a test that fails half way through does not leave a process behind
/// holding a `TempDir` open. Every graceful path is something a test asks for deliberately.
///
/// The `Option`s are what let [`finish`](Self::finish) consume the child and its readers while
/// [`Drop`] still has something safe to do: a type with a destructor cannot be taken apart by value.
#[derive(Debug)]
pub struct Running {
    child: Option<Child>,
    drained: Option<Drained>,
}

/// The two threads emptying the child's pipes, and what they have read so far.
///
/// Each ends by itself when the child closes its end, so nothing has to tell them to stop — killing
/// the process is what releases them, which is exactly what [`Running`]'s [`Drop`] does.
#[derive(Debug)]
struct Drained {
    stdout: Reader,
    stderr: Reader,
}

/// One stream being read, and the bytes it has produced up to now.
///
/// The buffer is shared rather than returned at the end because a test may need to know what the
/// service has said *while it is still saying it* — see [`Running::wait_for_stdout`].
#[derive(Debug)]
struct Reader {
    read: Arc<Mutex<Vec<u8>>>,
    thread: JoinHandle<()>,
}

impl Reader {
    /// Whether this stream has carried this text so far.
    fn contains(&self, text: &str) -> bool {
        String::from_utf8_lossy(&self.lock()).contains(text)
    }

    /// Wait for the reader to finish and take everything it read.
    fn take(self) -> Vec<u8> {
        let Self { read, thread } = self;
        thread.join().expect("the reader did not fail");

        let mut read = read.lock().unwrap_or_else(|poisoned| poisoned.into_inner());
        std::mem::take(&mut read)
    }

    /// The buffer, whether or not a reader that is not supposed to panic did.
    ///
    /// A poisoned lock still holds every byte written before the panic, and losing the output is
    /// the one thing that would leave a failing test with nothing to be explained by.
    fn lock(&self) -> std::sync::MutexGuard<'_, Vec<u8>> {
        self.read
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
    }
}

impl Running {
    /// Its pid, for a test that wants to look at it from outside.
    ///
    /// # Panics
    ///
    /// Never in practice: the child is only taken away by [`finish`](Self::finish), which consumes
    /// the handle.
    #[must_use]
    pub fn id(&self) -> u32 {
        self.child
            .as_ref()
            .expect("the child is held until finish takes it")
            .id()
    }

    /// Whether it is still going, without waiting for it either way.
    ///
    /// # Panics
    ///
    /// If this system cannot be asked about a process it started.
    #[must_use]
    pub fn still_running(&mut self) -> bool {
        self.child
            .as_mut()
            .expect("the child is held until finish takes it")
            .try_wait()
            .expect("this system can be asked about a process it started")
            .is_none()
    }

    /// Wait until the service has written this on its standard output. `false` if it had not within
    /// `patience`.
    ///
    /// What a test needs this for is not the line itself but what having written it proves about
    /// the process: [`READY_LINE`] means the stop handlers are installed, and a test that sends a
    /// signal before then is not testing what it thinks it is — the default disposition ends the
    /// process, and a service told to ignore a stop appears to have honoured one instead. A spawn
    /// returns as soon as the OS has a process, which is well before that process has parsed its
    /// arguments and registered anything.
    ///
    /// # Panics
    ///
    /// If the readers have been taken away by [`finish`](Self::finish), which consumes the handle.
    #[must_use]
    pub fn wait_for_stdout(&self, line: &str, patience: Duration) -> bool {
        let stdout = &self
            .drained
            .as_ref()
            .expect("the readers are held until finish takes them")
            .stdout;

        let deadline = Instant::now() + patience;
        loop {
            if stdout.contains(line) {
                return true;
            }

            if Instant::now() >= deadline {
                return false;
            }

            std::thread::sleep(Duration::from_millis(10));
        }
    }

    /// Wait for it to end and take everything it wrote.
    ///
    /// Returns once the process has gone *and* both readers have seen end-of-file, which is the
    /// property the orphan test rests on: a child still holding a copy of this process's stdout
    /// keeps the pipe open, and that shows up here as a wait rather than as a short read.
    ///
    /// # Panics
    ///
    /// If this system cannot be waited on for a process it started, or if a reader thread panicked
    /// — which would mean losing output a failing test is about to be explained by.
    #[must_use]
    pub fn finish(mut self) -> Output {
        let status = self
            .child
            .take()
            .expect("the child is held until finish takes it")
            .wait()
            .expect("this system can wait for a process it started");

        let drained = self
            .drained
            .take()
            .expect("the readers are held until finish takes them");

        Output {
            status,
            stdout: drained.stdout.take(),
            stderr: drained.stderr.take(),
        }
    }
}

impl Drop for Running {
    fn drop(&mut self) {
        if let Some(mut child) = self.child.take() {
            let _ = child.kill();
            let _ = child.wait();
        }

        // The readers are left to finish on their own. Killing the child above closed the only write
        // ends, so both are at end-of-file already; joining them would only be waiting for a thread
        // that is about to return a `Vec` nobody asked for.
    }
}

/// Read one of the child's streams to end-of-file on a thread of its own.
///
/// A block at a time into a shared buffer rather than a `read_to_end` into a local one, and the
/// difference is what [`Running::wait_for_stdout`] rests on: `read_to_end` hands nothing over until
/// the stream closes, which for a service that is still running is never.
///
/// A read that fails keeps whatever arrived before it rather than panicking: this thread outlives
/// the test's control flow, and a panic in here would be reported against no assertion at all.
fn drain(mut stream: impl std::io::Read + Send + 'static) -> Reader {
    let read = Arc::new(Mutex::new(Vec::new()));
    let into = Arc::clone(&read);

    let thread = std::thread::spawn(move || {
        let mut block = [0_u8; 4096];
        loop {
            match stream.read(&mut block) {
                // End of file, or a stream this thread can no longer read: either way there will be
                // no more of it, and what arrived already is in the buffer.
                Ok(0) | Err(_) => return,
                Ok(bytes) => {
                    let mut read = into.lock().unwrap_or_else(|poisoned| poisoned.into_inner());
                    read.extend_from_slice(&block[..bytes]);
                }
            }
        }
    });

    Reader { read, thread }
}
