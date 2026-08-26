//! A program that misbehaves on request, so supervision can be tested against something other than
//! MariaDB.
//!
//! `.claude/architecture/process-supervision.md` lists what it has to be able to do — start slowly,
//! never become ready, exit with a code after N ms, ignore a request to stop, leave a child behind —
//! and `mixengine_testkit::FakeService` is the caller's side of every flag below.
//!
//! It also stands in for the *daemon* in the supervision tests, which is what `--supervise`,
//! `--child` and `--hold-lock` are for: a process that owns a supervised child, a child that owns
//! nothing, and a beacon that says from outside whether a process is really still there. Only a
//! separate process can be killed the way a daemon is killed, and the test cannot be that process.
//!
//! **No `#[cfg]` anywhere in here**, which is the interesting constraint. Ignoring a request to stop,
//! leaving a detached child behind and owning a supervised one are all things the two families of
//! operating system do differently, and all of them are reached through `mixengine-platform` — the
//! same code paths the daemon uses, which means this fixture is also a second user of them.

use std::path::{Path, PathBuf};
use std::time::Duration;

use clap::Parser;
use mixengine_platform::lock::{Acquired, Lock};
use mixengine_platform::process::{self, Supervised};
use mixengine_platform::signal::Signals;
use mixengine_testkit::service::READY_LINE;
use tokio::time::{Instant, Interval, MissedTickBehavior, interval_at, sleep};

/// How long a child this program starts lives if nobody stops it.
///
/// Every one of them is meant to be ended by the test that asked for it — by outliving its parent
/// and being stopped, or by the process group it is in being killed — but a test that fails before
/// it gets there would otherwise leave a process running on the machine for as long as the machine
/// is up. A minute is far longer than any assertion about one needs and short enough that a CI
/// runner never notices.
const CHILD_LIFETIME: Duration = Duration::from_secs(60);

/// A service that behaves, unless told otherwise.
#[derive(Debug, Parser)]
#[command(
    name = "fakeservice",
    about = "A deliberately badly behaved process, for supervision tests."
)]
struct Args {
    /// Print a version line and exit zero, without becoming a service at all.
    ///
    /// **What makes this fixture usable as a runtime rather than only as a service.** An install's
    /// post-install check runs whichever flag prints a version — `-v` for PHP, `--version` for the
    /// other three — and a program that answered neither would make the one step a checksum cannot
    /// perform untestable. Both spellings, because both are what
    /// `mixengine_core::runtimes::smoke_test` really runs.
    ///
    /// `-v`/`--version` are free to be taken: `#[command(version)]` is deliberately not set on this
    /// binary, so clap reserves neither.
    #[arg(short = 'v', long = "version")]
    version: bool,

    /// Answer a configuration check the way php-fpm does, and exit without becoming a service.
    ///
    /// `--version`'s sibling, and here for the same reason it is: a pool's recipe validates the file
    /// it rendered by running `php-fpm --test --fpm-config <file>`, and a declared set fails
    /// **whole** when one row cannot be rendered — so a fixture PHP that refused this would take
    /// every `service.*` call after it down with it, over a question the suite is not asking.
    ///
    /// **The file is read rather than assumed**, so a staged path the validator got wrong still
    /// fails, which is the only thing running a check against a fixture is worth.
    #[arg(long = "test", requires = "fpm_config")]
    test_config: bool,

    /// The file `--test` reads. Named as php-fpm names it, because the recipe passes it by that name.
    #[arg(long = "fpm-config", value_name = "PATH")]
    fpm_config: Option<PathBuf>,

    /// Wait this many milliseconds before announcing readiness.
    #[arg(long, value_name = "MS", default_value_t = 0)]
    ready_after: u64,

    /// Never announce readiness at all.
    #[arg(long, conflicts_with = "ready_after")]
    never_ready: bool,

    /// Exit this many milliseconds after starting.
    #[arg(long, value_name = "MS")]
    exit_after: Option<u64>,

    /// The status to exit with when `--exit-after` arrives, or when `--touch` has done its work.
    #[arg(long, value_name = "CODE", default_value_t = 0)]
    exit_code: i32,

    /// Install the stop handlers and then ignore them, so only a kill ends this.
    #[arg(long)]
    ignore_stop: bool,

    /// Write this process's own id to this path, before anything else.
    #[arg(long, value_name = "PATH")]
    pid_file: Option<PathBuf>,

    /// Write every environment variable this process was given to this path, one `NAME=value` per
    /// line, and keep running.
    #[arg(long, value_name = "PATH")]
    dump_env: Option<PathBuf>,

    /// Create this file and exit, without becoming a service at all.
    ///
    /// The one-shot half of the fixture: this is what a `StopBehaviour::Command` or a
    /// `HealthProbe::Command` runs, and the file is how the test — and the service below — sees that
    /// it really ran.
    ///
    /// Straight away unless `--exit-after` says otherwise, and with `--exit-code`: a stop command
    /// that goes on running after delivering its instruction, and then fails, is how the window is
    /// staged in which a service stops *itself* while the command that asked it to is still going.
    #[arg(long, value_name = "PATH")]
    touch: Option<PathBuf>,

    /// Exit cleanly as soon as this file exists.
    ///
    /// The service half, and the pair is what makes a stop command provable: a service told to
    /// `--ignore-stop` cannot be ended politely by any signal, so a clean exit is evidence that the
    /// command ran and nothing else.
    #[arg(long, value_name = "PATH")]
    exit_when: Option<PathBuf>,

    /// Start a detached child that outlives this process, recording its pid at this path.
    #[arg(long, value_name = "PATH")]
    orphan: Option<PathBuf>,

    /// Hold an exclusive lock on this path for as long as this process lives.
    #[arg(long, value_name = "PATH")]
    hold_lock: Option<PathBuf>,

    /// Start a supervised child that holds a lock on this path, and own it.
    #[arg(long, value_name = "PATH")]
    supervise: Option<PathBuf>,

    /// Start an ordinary child that holds a lock on this path, and forget about it.
    #[arg(long, value_name = "PATH")]
    child: Option<PathBuf>,

    /// Leave a child holding this process's own streams open for this many milliseconds after it
    /// has exited.
    ///
    /// **What a one-shot behind a wrapper script really looks like**: the program a spec names exits
    /// with a status in milliseconds, and a helper it started still holds a copy of its stdout — so
    /// end of file on that pipe arrives long afterwards, or not at all. A caller that bounds its
    /// patience against the *pipe* rather than against the process reads that as a program which
    /// hung, and kills the database it had just asked to shut down cleanly. Combined with
    /// `--touch`, which returns before this program becomes a service at all.
    ///
    /// **A duration and not a file the test could create when it is done**, which was tried: the
    /// release would have to outlive the test's own temporary directory, because a runtime is
    /// dropped after every local in the test body and it is that drop which waits for the pipe. A
    /// release the tempdir takes with it is one the child never sees, and the run falls back to this
    /// ceiling instead — slower than the duration it replaced, for more moving parts.
    #[arg(long, value_name = "MS")]
    lingering_child: Option<u64>,

    /// Write these lines to stderr and exit, without becoming a service at all.
    ///
    /// **A configuration checker that says no, in the shape real ones say it.** Repeatable, because
    /// the interesting part is the order: `nginx -t` prints the reason and then a summary that names
    /// the file, and `caddy validate` prints a banner and then the reason — so which line a refusal
    /// is reported by is a decision this fixture can stage from both ends. Exits with
    /// `--exit-code`, like `--touch` does.
    #[arg(long, value_name = "LINE")]
    complain: Vec<String>,

    /// Write a numbered line this often.
    #[arg(long, value_name = "MS")]
    log_every: Option<u64>,

    /// Write those lines to stderr as well as stdout.
    #[arg(long)]
    log_to_stderr: bool,

    /// Take another bite of memory this big, this often, and never let go of any of it.
    ///
    /// **For proving that a memory ceiling binds** — roadmap task **T68**, whose acceptance criterion
    /// asks for "an integration test that allocates past it". A service walking into a real cap is
    /// the only thing that proves the cap by *outcome* rather than by reading a number back out of
    /// the mechanism it was written into.
    ///
    /// **Every byte is written, not merely reserved**, and that is what makes it work: a large `Vec`
    /// that is only allocated may cost no physical pages at all, so a cgroup counting charged pages
    /// would never see it and the test would hang instead of failing. The fill byte is what makes
    /// the memory real on both systems.
    #[arg(long, value_name = "MB")]
    eat_memory_mb: Option<usize>,

    /// How long to wait between bites.
    #[arg(long, value_name = "MS", default_value_t = 50)]
    eat_memory_every: u64,
}

/// The flag whose value is a file of further flags, one per line.
///
/// **This is what makes a generated configuration load-bearing in a supervision test.** The
/// daemon's fixture recipe (roadmap task T30) renders a service's overrides into
/// `etc/<service-id>/fakeservice.args` and passes that path here, so a test that changes an override
/// and starts the service again has exercised the whole chain — the row, the merge, the template,
/// the diff, the install — and can see the difference in what the process *does*. A recipe that put
/// the same flags straight onto the command line would render a file nothing reads.
const ARGS_FILE: &str = "--args-file";

/// This process's command line, with any [`ARGS_FILE`] replaced by what is inside it.
///
/// Done before `clap` sees anything rather than as a flag it parses, because the alternative is
/// parsing twice and merging two `Args` — and the second parse is what would have to decide whether
/// a flag in the file beats the same flag on the command line. Splicing in place gives that answer
/// for free: the file's arguments sit exactly where its path did.
///
/// Blank lines and `#` comments are ignored, because the file is generated from a template with a
/// header on it.
fn command_line() -> Vec<std::ffi::OsString> {
    let mut spliced = Vec::new();
    let mut arguments = std::env::args_os();

    while let Some(argument) = arguments.next() {
        if argument != ARGS_FILE {
            spliced.push(argument);
            continue;
        }

        let path = arguments
            .next()
            .unwrap_or_else(|| panic!("{ARGS_FILE} names a file"));
        let text = std::fs::read_to_string(&path)
            .unwrap_or_else(|error| panic!("fakeservice reads {path:?}: {error}"));

        spliced.extend(
            text.lines()
                .map(str::trim)
                .filter(|line| !line.is_empty() && !line.starts_with('#'))
                .map(std::ffi::OsString::from),
        );
    }

    spliced
}

/// A single thread is the whole runtime this needs, and it is what a fixture spawned a hundred times
/// in one test run should cost.
#[tokio::main(flavor = "current_thread")]
async fn main() {
    let args = Args::parse_from(command_line());

    // First of everything, and before a pid file: this run is not a service and is not being
    // supervised — it is a freshly unpacked runtime being asked whether it starts on this machine.
    if args.version {
        println!("fakeservice {}", env!("CARGO_PKG_VERSION"));
        return;
    }

    // Beside it and for the same reason: this run is not a service either, it is a PHP being asked
    // whether a file parses. Nothing is written and nothing is supervised.
    if args.test_config {
        let path = args
            .fpm_config
            .as_deref()
            .expect("clap requires --fpm-config alongside --test");

        if let Err(error) = std::fs::read_to_string(path) {
            eprintln!("[fakeservice] cannot read {}: {error}", path.display());
            std::process::exit(1);
        }

        println!(
            "[fakeservice] configuration file {} test is successful",
            path.display()
        );
        return;
    }

    // Beside the two above, and not a service either: this run is a checker being handed a file it
    // is about to refuse.
    if !args.complain.is_empty() {
        for line in &args.complain {
            eprintln!("{line}");
        }

        std::process::exit(args.exit_code);
    }

    if let Some(path) = &args.pid_file {
        std::fs::write(path, std::process::id().to_string())
            .unwrap_or_else(|error| panic!("fakeservice records its pid at {path:?}: {error}"));
    }

    if let Some(path) = &args.dump_env {
        dump_env(path);
    }

    // Before the `--touch` below, because the whole point of the pair is a one-shot that has already
    // exited while something it started still holds its stdout.
    if let Some(millis) = args.lingering_child {
        leave_a_lingering_child(millis);
    }

    // Before the signal handlers and before anything is announced: this run is not a service, it is
    // the program a service's spec names for stopping or probing itself, and what it does is create
    // the file, wait as long as it was told to, and exit with the status it was given.
    if let Some(path) = &args.touch {
        std::fs::write(path, "asked")
            .unwrap_or_else(|error| panic!("fakeservice creates {path:?}: {error}"));

        // Nothing, unless a test asked for something — and not `end_of_life`, which waits for ever
        // when no time was named because that is what a *service* with no `--exit-after` should do.
        // A stop command that keeps running after it has delivered its instruction is what a real
        // one does — `mariadb-admin shutdown` returns once the server has *accepted* it — and it is
        // the only way to stage the window in which a service goes by itself while the command that
        // asked it to is still running.
        if let Some(millis) = args.exit_after {
            sleep(Duration::from_millis(millis)).await;
        }

        std::process::exit(args.exit_code);
    }

    if let Some(path) = &args.orphan {
        leave_an_orphan(path);
    }

    // Bound for the whole of `main`, both of them, and that is the entire mechanism. The lock is
    // released by the OS when this process ends however it ends, and the supervised child is killed
    // when the handle owning it drops — which a `return` from the loop below does and a `SIGKILL`
    // does not, and telling those two apart is what the supervision tests are for.
    let _lock = args.hold_lock.as_deref().map(hold);
    let _supervised = args.supervise.as_deref().map(supervise);

    if let Some(path) = &args.child {
        leave_an_ordinary_child(path);
    }

    // Installed whether or not they are going to be honoured: a process that *ignores* a stop is one
    // that received it and did nothing, which is a different thing from one the OS killed outright
    // because no handler was there. Only the first is what a grace period has to survive.
    let mut signals = Signals::listen().expect("fakeservice can be asked to stop");

    // Started here rather than in the loop below, because what it does is unbounded and the loop's
    // job is to stay responsive: a service that is eating its way towards a ceiling must still
    // answer a stop, still print its lines, and still be killed by its group.
    let _eating = args
        .eat_memory_mb
        .map(|mb| eat_memory(mb, args.eat_memory_every));

    let mut ready = Box::pin(announce_ready(&args));
    let mut announced = false;
    let mut ending = Box::pin(end_of_life(&args));
    let mut ticker = args.log_every.map(ticker);
    let mut line = 0_u64;

    loop {
        tokio::select! {
            () = &mut ready, if !announced => {
                announced = true;
                emit(READY_LINE, args.log_to_stderr);
            }

            () = &mut ending => {
                // The one exit that is not a stop: a service that ends by itself, which is what a
                // restart policy has to tell apart from one that was asked to go.
                std::process::exit(args.exit_code);
            }

            () = asked_by_file(&args) => {
                // A stop that arrived by a route no signal could take. Cleanly, and with a line, so
                // it reads in `current.log` the way a real shutdown command's does.
                emit("fakeservice: stopping, the file it watches appeared", args.log_to_stderr);
                return;
            }

            () = tick(&mut ticker) => {
                line += 1;
                emit(&format!("fakeservice: line {line}"), args.log_to_stderr);
            }

            stop = signals.stopped() => {
                if !args.ignore_stop {
                    emit(&format!("fakeservice: stopping on {stop}"), args.log_to_stderr);
                    return;
                }

                emit(&format!("fakeservice: ignoring {stop}"), args.log_to_stderr);
            }
        }
    }
}

/// Take `mb` megabytes every `every` milliseconds and hold on to all of it, for ever.
///
/// **On a thread of its own rather than in the `select!`**, so that a service walking into a ceiling
/// is still a *service*: it answers a stop, prints its lines, and is killed by its group like any
/// other. A future that allocated inside the loop would starve every other arm between bites.
///
/// The handle is returned and held by `main`, and is never joined: this thread does not end, and the
/// process ending is what stops it. What the caller keeps is the memory, not the thread.
fn eat_memory(mb: usize, every: u64) -> std::thread::JoinHandle<()> {
    std::thread::spawn(move || {
        let mut held: Vec<Vec<u8>> = Vec::new();

        loop {
            // `vec![byte; n]` writes every byte, which is the point — see the flag's own
            // documentation. A `Vec::with_capacity` here would reserve address space and touch no
            // page, and a cgroup counting charged pages would never notice this process at all.
            held.push(vec![0xA5_u8; mb * 1024 * 1024]);

            eprintln!("[fakeservice] holding {} MB", held.len() * mb);

            std::thread::sleep(Duration::from_millis(every));
        }
    })
}

/// Resolve once the service should call itself ready — or never.
async fn announce_ready(args: &Args) {
    if args.never_ready {
        std::future::pending().await
    } else {
        sleep(Duration::from_millis(args.ready_after)).await;
    }
}

/// Resolve once the service should exit of its own accord — or never.
async fn end_of_life(args: &Args) {
    match args.exit_after {
        Some(millis) => sleep(Duration::from_millis(millis)).await,
        None => std::future::pending().await,
    }
}

/// Resolve once the file this run watches for exists — or never, when it watches for none.
///
/// Polled rather than watched, because a filesystem notification API is a dependency this fixture
/// has no business carrying to answer a question a `stat` every 25 ms answers. Cancel safe by
/// construction: the state is the file, not the future, so a turn of the loop that served another
/// arm has not missed anything.
async fn asked_by_file(args: &Args) {
    let Some(path) = &args.exit_when else {
        return std::future::pending().await;
    };

    while !path.exists() {
        sleep(Duration::from_millis(25)).await;
    }
}

/// A ticker that does not try to catch up.
///
/// `MissedTickBehavior::Delay` rather than the default: a fixture asked for a line every 10ms on a
/// loaded CI runner would otherwise answer a stall with a burst of lines all bearing the same
/// instant, which is not what any log the supervisor is capturing looks like. The first tick is a
/// full interval away for the same reason — `interval` fires immediately, and a line before the
/// service is ready would land in the wrong half of every test that reads them.
fn ticker(millis: u64) -> Interval {
    let period = Duration::from_millis(millis);
    let mut ticker = interval_at(Instant::now() + period, period);
    ticker.set_missed_tick_behavior(MissedTickBehavior::Delay);
    ticker
}

/// Wait for the next tick, or forever when there is nothing ticking.
///
/// Cancel safe on both arms, which is what a `select!` needs of it: `Interval::tick` keeps its state
/// in the interval rather than in the future, so a turn of the loop that served another arm has not
/// swallowed a tick.
async fn tick(ticker: &mut Option<Interval>) {
    match ticker {
        Some(ticker) => {
            ticker.tick().await;
        }
        None => std::future::pending().await,
    }
}

/// Write one line, to the streams this run was asked for.
///
/// Flushed by hand rather than trusted to line buffering: the whole point of these lines is that
/// something else is reading them through a pipe while this process is still running.
///
/// The write failing is ignored rather than fatal, which `println!` would have made it. A supervisor
/// that lets go of the read end while the service is still up — capture stopping before the process
/// does — is a thing the supervisor is allowed to do, and a real service goes on running through it.
/// A panic here would end the fixture with status 101 in the middle of the very test that is
/// measuring how it ends.
fn emit(line: &str, also_stderr: bool) {
    use std::io::Write as _;

    let mut stdout = std::io::stdout().lock();
    let _ = writeln!(stdout, "{line}");
    let _ = stdout.flush();

    if also_stderr {
        let mut stderr = std::io::stderr().lock();
        let _ = writeln!(stderr, "{line}");
        let _ = stderr.flush();
    }
}

/// Start a detached copy of this program and record its pid, then forget about it.
///
/// `spawn_detached` rather than a plain `Command`, and that is the part worth spelling out: a child
/// holding a copy of this process's stdout keeps the pipe open after this process is gone, so a test
/// reading it to end-of-file would wait for the orphan rather than for the service. That is the same
/// hazard `mixengined --detach` met (roadmap tasks T9 and T10), and it is fixed here by using the
/// same code rather than by a second answer to it.
///
/// The pid is written by *this* process rather than by the child, so it is on disk by the time this
/// function returns and a test never has to race a child's first write.
///
/// The working directory is the system temporary directory rather than the one `pid_file` is in, and
/// that is the warning [`spawn_detached`](process::spawn_detached) gives being taken: a process's
/// working directory is a reference the OS holds for its whole life, so an orphan parked in the
/// caller's `TempDir` would stop that directory from being removed on Windows — a fixture quietly
/// leaking a home per test run.
fn leave_an_orphan(pid_file: &Path) {
    let args = [
        "--exit-after".into(),
        CHILD_LIFETIME.as_millis().to_string().into(),
    ];

    let orphan = process::spawn_detached(&myself(), &args, &std::env::temp_dir())
        .expect("fakeservice can start a copy of itself");

    std::fs::write(pid_file, orphan.pid().to_string())
        .unwrap_or_else(|error| panic!("fakeservice records the orphan at {pid_file:?}: {error}"));
}

/// Take the lock at `path` and hold it for the rest of this process's life.
///
/// **The beacon the supervision tests read.** A pid says whether a *number* is in use and, on Unix,
/// goes on saying yes for a process that has exited and not been reaped; a lock is released by the
/// kernel when the process really ends, and by nothing else. So "the lock at this path can be taken"
/// is the one statement outside a process that means it is gone — which is exactly the claim
/// roadmap task T13 has to make about a supervised child, and the reason `try_stop` could not make
/// it.
///
/// The lock file records the holder's pid on the way in, so a test can find this process without a
/// second flag and a second file.
///
/// A lock somebody else already holds is fatal here. A test that reuses a path while another
/// fixture still has it is asking a question about the wrong process, and answering it quietly is
/// how that becomes an afternoon.
fn hold(path: &Path) -> mixengine_platform::lock::Lock {
    let acquired = Lock::acquire(path)
        .unwrap_or_else(|error| panic!("fakeservice can take the lock at {path:?}: {error}"));

    match acquired {
        Acquired::Held(lock) => lock,
        Acquired::Taken(holder) => {
            panic!("the lock at {path:?} is already held by {holder}")
        }
    }
}

/// Write down every variable this process was started with.
///
/// A file rather than stdout, because the question it answers — *what is a supervised child's
/// environment* — is asked by a test that is also reading that stream for a ready line, and because
/// a value may contain anything including a newline. Written once, at startup, before the program
/// does anything that could add to it.
///
/// Sorted, so a failure prints a diff a person can read rather than the hash order of the day.
fn dump_env(path: &Path) {
    let mut variables: Vec<String> = std::env::vars_os()
        .map(|(name, value)| {
            format!(
                "{}={}",
                name.to_string_lossy(),
                value.to_string_lossy().escape_debug()
            )
        })
        .collect();
    variables.sort();

    std::fs::write(path, variables.join("\n"))
        .unwrap_or_else(|error| panic!("fakeservice records its environment at {path:?}: {error}"));
}

/// Start a supervised copy of this program, holding a lock at `path`, and own it.
///
/// The handle is returned rather than dropped, because dropping it is what kills the child — this
/// program is standing in for the daemon, and the whole question the test asks is what happens to
/// the child when this process ends one way or the other.
///
/// `spawn_supervised` rather than a plain `Command`, for the same reason `--orphan` uses
/// `spawn_detached`: the fixture exercises the daemon's own code path instead of a second answer to
/// it.
fn supervise(path: &Path) -> Supervised {
    // No environment of its own: this fixture is a stand-in for the daemon, and what a spec would
    // put here is nothing a copy of `fakeservice` reads. The per-OS floor `spawn_supervised` applies
    // underneath is what lets the child load its own libraries and find its temporary directory.
    process::spawn_supervised(
        &myself(),
        &holding(path),
        &std::env::temp_dir(),
        &std::collections::BTreeMap::new(),
        &process::Limits::default(),
    )
    .expect("fakeservice can start a supervised copy of itself")
}

/// Start an ordinary copy of this program, holding a lock at `path`, and forget about it.
///
/// The grandchild in "stopping a service stops what the service started": it inherits this
/// process's job on Windows and its process group on Unix, because it does nothing to leave either.
/// That is what a php-fpm worker or a `mariadbd` behind a wrapper script looks like from outside.
///
/// Its streams go to the null device. Inheriting this process's stdout would hand it a copy of the
/// pipe a test is reading to end-of-file, which is the hazard `spawn_detached`'s documentation is
/// mostly about — one process further out, and just as effective at making a test hang.
#[expect(
    clippy::zombie_processes,
    reason = "not reaping it is the fixture: this process must be killable while the child is \
              still running, and a `wait` here would block until the child's own lifetime ran out. \
              The zombie lasts as long as this process does, which is until the test kills it"
)]
fn leave_an_ordinary_child(path: &Path) {
    std::process::Command::new(myself())
        .args(holding(path))
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .spawn()
        .expect("fakeservice can start a plain copy of itself");
}

/// Start an ordinary copy of this program that keeps this process's streams open after it has gone.
///
/// The exact opposite of [`leave_an_ordinary_child`], which sends its child's streams to the null
/// device precisely so that it cannot do this — here holding the pipe open *is* the fixture, so the
/// streams are inherited and nothing else about the child matters. `--never-ready` so it says
/// nothing at all: everything it wrote would arrive on its parent's stdout, in the middle of what a
/// test is reading there.
#[expect(
    clippy::zombie_processes,
    reason = "not reaping it is the fixture: this process must exit at once and leave the child \
              behind holding its streams, and a `wait` here would do the opposite"
)]
fn leave_a_lingering_child(millis: u64) {
    std::process::Command::new(myself())
        .args(["--never-ready".to_owned()])
        .args(["--exit-after".to_owned(), millis.to_string()])
        .stdin(std::process::Stdio::null())
        .spawn()
        .expect("fakeservice can start a plain copy of itself");
}

/// This program, so it can start another of itself.
fn myself() -> PathBuf {
    std::env::current_exe().expect("fakeservice knows where it is")
}

/// The arguments for a copy of this program that exists only to hold a lock and be killed.
fn holding(path: &Path) -> [std::ffi::OsString; 4] {
    [
        "--hold-lock".into(),
        path.into(),
        "--exit-after".into(),
        CHILD_LIFETIME.as_millis().to_string().into(),
    ]
}
