//! A program that misbehaves on request, so supervision can be tested against something other than
//! MariaDB.
//!
//! `.claude/architecture/process-supervision.md` lists what it has to be able to do — start slowly,
//! never become ready, exit with a code after N ms, ignore a request to stop, leave a child behind —
//! and `mixengine_testkit::FakeService` is the caller's side of every flag below.
//!
//! **No `#[cfg]` anywhere in here**, which is the interesting constraint. Ignoring a request to stop
//! and leaving a detached child behind are both things the two families of operating system do
//! differently, and both are reached through `mixengine-platform` — the same code paths the daemon
//! uses, which means this fixture is also a second user of them.

use std::path::PathBuf;
use std::time::Duration;

use clap::Parser;
use mixengine_platform::process;
use mixengine_platform::signal::Signals;
use mixengine_testkit::service::READY_LINE;
use tokio::time::{Instant, Interval, MissedTickBehavior, interval_at, sleep};

/// How long a child left behind by `--orphan` lives if nobody stops it.
///
/// It is meant to be stopped by the test that asked for it — the point of an orphan is that it
/// outlives its parent — but a test that fails before it gets there would otherwise leave a process
/// running on the machine for as long as the machine is up. A minute is far longer than any
/// assertion about it needs and short enough that a CI runner never notices.
const ORPHAN_LIFETIME: Duration = Duration::from_secs(60);

/// A service that behaves, unless told otherwise.
#[derive(Debug, Parser)]
#[command(
    name = "fakeservice",
    about = "A deliberately badly behaved process, for supervision tests."
)]
struct Args {
    /// Wait this many milliseconds before announcing readiness.
    #[arg(long, value_name = "MS", default_value_t = 0)]
    ready_after: u64,

    /// Never announce readiness at all.
    #[arg(long, conflicts_with = "ready_after")]
    never_ready: bool,

    /// Exit this many milliseconds after starting.
    #[arg(long, value_name = "MS")]
    exit_after: Option<u64>,

    /// The status to exit with when `--exit-after` arrives.
    #[arg(long, value_name = "CODE", default_value_t = 0)]
    exit_code: i32,

    /// Install the stop handlers and then ignore them, so only a kill ends this.
    #[arg(long)]
    ignore_stop: bool,

    /// Write this process's own id to this path, before anything else.
    #[arg(long, value_name = "PATH")]
    pid_file: Option<PathBuf>,

    /// Start a detached child that outlives this process, recording its pid at this path.
    #[arg(long, value_name = "PATH")]
    orphan: Option<PathBuf>,

    /// Write a numbered line this often.
    #[arg(long, value_name = "MS")]
    log_every: Option<u64>,

    /// Write those lines to stderr as well as stdout.
    #[arg(long)]
    log_to_stderr: bool,
}

/// A single thread is the whole runtime this needs, and it is what a fixture spawned a hundred times
/// in one test run should cost.
#[tokio::main(flavor = "current_thread")]
async fn main() {
    let args = Args::parse();

    if let Some(path) = &args.pid_file {
        std::fs::write(path, std::process::id().to_string())
            .unwrap_or_else(|error| panic!("fakeservice records its pid at {path:?}: {error}"));
    }

    if let Some(path) = &args.orphan {
        leave_an_orphan(path);
    }

    // Installed whether or not they are going to be honoured: a process that *ignores* a stop is one
    // that received it and did nothing, which is a different thing from one the OS killed outright
    // because no handler was there. Only the first is what a grace period has to survive.
    let mut signals = Signals::listen().expect("fakeservice can be asked to stop");

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
fn leave_an_orphan(pid_file: &std::path::Path) {
    let program = std::env::current_exe().expect("fakeservice knows where it is");
    let directory = std::env::temp_dir();
    let args = [
        "--exit-after".into(),
        ORPHAN_LIFETIME.as_millis().to_string().into(),
    ];

    let orphan = process::spawn_detached(&program, &args, &directory)
        .expect("fakeservice can start a copy of itself");

    std::fs::write(pid_file, orphan.pid().to_string())
        .unwrap_or_else(|error| panic!("fakeservice records the orphan at {pid_file:?}: {error}"));
}
