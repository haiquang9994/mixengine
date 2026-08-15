//! What a shim costs, and which half of that the budget is about.
//!
//! Roadmap task **T29**. [`../features/runtime-versions.md`](../features/runtime-versions.md) puts a
//! number on step 2 of the shim and on nothing else — *"Calls `resolve` (in-process, reading SQLite
//! read-only + walking for `mixengine.toml`) — **no IPC**, so it stays fast even when the daemon is
//! down. Target: **< 15 ms** overhead, enforced by a bench"* — and that sentence is the reason
//! `mixengine-shim` links `mixengine-core` at all. A promise nothing measures is a design decision
//! nothing is holding to account, so `resolving_a_version_stays_inside_the_budget` is the bench it
//! asks for.
//!
//! # Two measurements, and only one of them is a gate
//!
//! **The resolution is gated**, because it is what this workspace decides: opening the database
//! read-only, walking for a manifest, and looking up which file the artifact publishes. Anything
//! that would make it miss — a shim that asked the daemon after all, that parsed `config.toml`, that
//! fetched an index — is a regression this fails on, which is the whole job.
//!
//! **The wall clock is measured, reported, and gated on nothing**, and that is a decision rather
//! than an omission. What a person waits for when they type `php -v` is dominated by process
//! creation: on Unix the shim `exec`s and there is one process either way, but on Windows there is
//! no `exec`, so the design starts a child inside a Job Object and waits — a whole second process
//! creation that no amount of care here removes. What this file printed on the three CI runners,
//! at p50, with the gated resolution beside it:
//!
//! ```text
//!             program alone   through the shim   difference   resolution
//!   ubuntu          1.06 ms            3.26 ms      2.19 ms      0.74 ms
//!   macos           3.11 ms            7.66 ms      4.52 ms      0.58 ms
//!   windows         8.63 ms           23.54 ms     15.03 ms      1.71 ms
//! ```
//!
//! The two Unix rows are the shim's own image load and then an `exec`. The Windows row is that plus
//! an entire second process, and it lands **on** the 15 ms line — so a wall-clock budget would flap
//! across it from run to run while saying nothing about the resolution, which is nine times inside
//! its own budget on that same machine. A budget there would be a budget on the runner's process
//! model, and a pessimistic one on top: `fakeservice` is a one-megabyte binary where the `php.exe` a
//! real shim fronts is sixty, so this fixture overstates the shim image's share by a wide margin.
//! Where the Windows time goes was taken apart on a developer machine rather than in CI: a shim that
//! only loads and refuses to dispatch costs 16 ms of the 30 ms a full run took there, so it is the
//! two images and the two creations, not anything between them. The numbers are printed on every
//! run, so a regression in them is visible in the log, and `really_ran` is what keeps them
//! measurements of a shim that really became something.
//!
//! # The failure this file would otherwise have is a pass
//!
//! A shim that resolves nothing is far faster than one that hands over, so a broken home, an empty
//! `bin/`, a fixture that stopped being copied would each make these numbers *better*. Every run is
//! therefore checked — the wall clock against the program's own output, the resolution against the
//! path it answered. That guard has earned itself once already: it caught a stale
//! `target/release/fakeservice`, a binary from before `--version` existed, which
//! `cargo test -p mixengine-shim` does not rebuild. The CI job builds it explicitly because of that.
//!
//! # Release, and ignored
//!
//! `#[ignore]`d because these belong to the `bench` job and not to `test`: a hundred and fifty
//! processes and a number a loaded runner can move should not stand between a correctness suite and
//! its answer. The budget is asserted **only in a release build** — a debug SQLite is a different
//! program, and a number measured there would be about the profile rather than about the design. A
//! debug run still measures and still prints, so nothing here can rot unnoticed.
//!
//! **`--test-threads=1`, and it is not a nicety.** Cargo runs the two tests below in parallel by
//! default, and each of them spends its whole time creating processes — so each measures the other.
//! Run together the end-to-end difference reads 34 ms and run one at a time it reads 21, on the same
//! machine within the same minute. The CI step says so; a person running this by hand should too.
//!
//! They run on all three systems rather than on Linux alone. The gate is the same everywhere, but
//! what it runs over is not: SQLite's locking, the cost of opening a file and the walk from a
//! directory to the root are each per-OS, and the reported wall clock is the only place the
//! difference between `exec` and a Job Object child is ever written down as a number.

use std::path::{Path, PathBuf};
use std::process::{Command, Output};
use std::time::{Duration, Instant};

use mixengine_core::{Store, paths, resolve, runtimes};
use mixengine_proto::RuntimeKind;

mod harness;

use harness::{Home, published_at};

/// [`../features/runtime-versions.md`](../features/runtime-versions.md), restated where it is
/// enforced.
const BUDGET: Duration = Duration::from_millis(15);

/// Rounds kept. Odd, so the median is a measurement rather than the average of two.
const RUNS: usize = 31;

/// Rounds thrown away first: the first run of anything pays for a cold file cache, and on Windows
/// for a loader resolving a DLL set nothing has touched yet.
const WARMUP: usize = 5;

/// The version installed first, which is therefore the kind's default.
const DEFAULT_VERSION: &str = "8.1.30";

/// The version the manifest below pins — deliberately not the default, so a shape that resolved
/// nothing would reach the wrong program and be caught rather than timed.
const PINNED_VERSION: &str = "8.3.33";

/// **The budget.** What the shim decides, on the two shapes a directory can have.
///
/// Neither shape is obviously the more expensive, which is why both are held to the number rather
/// than one of them being declared the worst case: nothing pinned is the deepest walk — every
/// directory from here to the root is opened looking for a `mixengine.toml`, and then the kind's
/// default is read — while a manifest in the directory stops the walk at the first try and pays for
/// a TOML parse instead.
///
/// The three calls timed here are the three the shim's own `resolved` makes, in its order: open the
/// database read-only, resolve, then ask which file the artifact publishes. Timed together rather
/// than one by one, because the budget is on the answer and not on any step of it.
#[test]
#[ignore = "a performance budget: the `bench` job's, not the `test` job's"]
fn resolving_a_version_stays_inside_the_budget() {
    let home = a_machine_with_five_runtimes_on_it();
    let database = home.path().join(paths::DATABASE_FILE_NAME);

    let runtime = tokio::runtime::Builder::new_current_thread()
        // What the shim itself enables, and for the same reason: `sqlx` panics without a timer,
        // because the busy timeout and the pool's acquire deadline are both timers.
        .enable_time()
        .build()
        .expect("a runtime to measure in");

    for shape in shapes(&home) {
        let mut rounds = Vec::new();

        runtime.block_on(async {
            for round in 0..(WARMUP + RUNS) {
                let started = Instant::now();

                let store = Store::open_read_only(&database)
                    .await
                    .expect("the database a daemon left");

                let resolved = resolve::runtime(
                    &store,
                    &resolve::Question {
                        kind: RuntimeKind::Php,
                        cwd: Some(&shape.cwd),
                        explicit: None,
                    },
                )
                .await
                .expect("a version");

                let program =
                    runtimes::program(&store, RuntimeKind::Php, &resolved.runtime.version, "php")
                        .await
                        .expect("the file the artifact publishes");

                let elapsed = started.elapsed();
                store.close().await;

                // The same guard the wall clock has, one layer down: a resolution that answered the
                // wrong version would be a *faster* resolution, so the number is only kept once it
                // is known to be about the right one.
                assert_eq!(
                    program, shape.program,
                    "{}: resolved to a different program than the shim would run",
                    shape.called
                );

                if round >= WARMUP {
                    rounds.push(elapsed);
                }
            }
        });

        let measured = Spans::of(&rounds);
        println!(
            "resolving — {} ({RUNS} rounds, {WARMUP} discarded)\n  p50 {}, p90 {}, budget {}",
            shape.called,
            millis(measured.at(50)),
            millis(measured.at(90)),
            millis(BUDGET)
        );

        // Debug is not a build this number means anything in — said here rather than by skipping the
        // work above, so that everything except the judgement is exercised either way.
        if cfg!(debug_assertions) {
            println!("  (debug build: measured and printed, not judged)");
            continue;
        }

        assert!(
            measured.at(50) < BUDGET,
            "{}: resolving took {} and the budget is {}",
            shape.called,
            millis(measured.at(50)),
            millis(BUDGET)
        );
    }
}

/// **What it costs end to end**, which is the number a person actually waits for.
///
/// Reported rather than gated, for the reason this file's header gives: it is process creation
/// nearly all the way down, and on Windows it includes a second process the design cannot avoid. It
/// is here because the alternative is a claim about `php -v` that nothing in the workspace has ever
/// timed — and because a shim that stopped handing over, or a hand-over that grew a sleep in it,
/// would show up here and nowhere else.
///
/// Each round runs the shim and then runs **the very program that shim resolves to**, with the same
/// argument from the same directory. The two are interleaved rather than run as two blocks, because
/// a runner that gets slower halfway through would otherwise charge the whole of that drift to
/// whichever side went second.
#[test]
#[ignore = "a measurement rather than a gate: the `bench` job's"]
fn what_a_shim_costs_in_front_of_a_program() {
    let home = a_machine_with_five_runtimes_on_it();
    let shim = home.shim("php");

    for shape in shapes(&home) {
        for _ in 0..WARMUP {
            shape.through(&shim, &home);
            shape.directly(&home);
        }

        let mut through = Vec::new();
        let mut directly = Vec::new();
        let mut overhead = Vec::new();

        for _ in 0..RUNS {
            let shimmed = shape.through(&shim, &home);
            let bare = shape.directly(&home);

            through.push(shimmed);
            directly.push(bare);
            // Saturating, because a pair whose bare run happened to be descheduled comes out
            // negative, and what is wanted from these is a shape rather than a signed statistic.
            overhead.push(shimmed.saturating_sub(bare));
        }

        let through = Spans::of(&through);
        let directly = Spans::of(&directly);
        let overhead = Spans::of(&overhead);

        println!(
            "end to end — {} ({RUNS} pairs, {WARMUP} discarded)\n  \
             p50 shim {}, program {}, difference {}\n  \
             p90 shim {}, program {}, difference {}",
            shape.called,
            millis(through.at(50)),
            millis(directly.at(50)),
            millis(overhead.at(50)),
            millis(through.at(90)),
            millis(directly.at(90)),
            millis(overhead.at(90)),
        );
    }
}

/// A home shaped like one somebody has been using: three PHPs and two other languages, so the tables
/// the shim reads have the rows a real machine's do.
fn a_machine_with_five_runtimes_on_it() -> Home {
    let home = Home::with(&[DEFAULT_VERSION, "8.2.29", PINNED_VERSION]);

    home.install(
        RuntimeKind::Node,
        "22.23.2",
        [(
            "node".to_owned(),
            format!("bin/node{}", std::env::consts::EXE_SUFFIX),
        )]
        .into_iter()
        .collect(),
    );
    home.install(
        RuntimeKind::Python,
        "3.13.15",
        [(
            "python".to_owned(),
            format!("bin/python3.13{}", std::env::consts::EXE_SUFFIX),
        )]
        .into_iter()
        .collect(),
    );

    home
}

/// The two directories both tests measure from.
fn shapes(home: &Home) -> [Shape; 2] {
    let unpinned = home.project("scratch", None);
    // Four levels below the project root, which is an ordinary place to be typing `php` from and is
    // what makes the walk cost something.
    let deep = unpinned.join("src").join("app").join("http").join("routes");
    std::fs::create_dir_all(&deep).expect("a directory to run from");

    let pinned = home.project(
        "blog",
        Some(&format!("[runtimes]\nphp = \"{PINNED_VERSION}\"\n")),
    );

    [
        Shape {
            called: "nothing pins it, so the walk reaches the root",
            cwd: deep,
            program: home.runtime_directory(DEFAULT_VERSION).join(published_at()),
        },
        Shape {
            called: "a manifest in the directory pins it",
            cwd: pinned,
            program: home.runtime_directory(PINNED_VERSION).join(published_at()),
        },
    ]
}

/// One directory to type `php --version` in, and the program a correct resolution ends at.
#[derive(Debug)]
struct Shape {
    /// What this shape is, for the log line and the failure message.
    called: &'static str,

    /// Where the command is run from, which is what the resolution is about.
    cwd: PathBuf,

    /// The file the shim should end up handing over to — and, run directly, the baseline.
    program: PathBuf,
}

impl Shape {
    /// `bin/php --version`: a shim that resolves and then becomes the program.
    fn through(&self, shim: &Path, home: &Home) -> Duration {
        let (elapsed, output) = timed(shim, &self.cwd, home.path());
        really_ran(&output, "the shim", self.called);
        elapsed
    }

    /// The same program with the same argument from the same directory, started by this process.
    fn directly(&self, home: &Home) -> Duration {
        let (elapsed, output) = timed(&self.program, &self.cwd, home.path());
        really_ran(&output, "the program itself", self.called);
        elapsed
    }
}

/// A set of timings, sorted, answering at a percentile.
///
/// The mean is deliberately not among them: over thirty-one runs on a machine shared with other jobs
/// it is decided by the one run that was descheduled. Nor is the minimum, which answers "how fast
/// could this be on an idle machine" and is not what was promised to anybody. The median is what a
/// person meets, and the p90 is reported beside it so a number that is drifting says so in the log
/// before the build that finally fails.
#[derive(Debug)]
struct Spans(Vec<Duration>);

impl Spans {
    fn of(runs: &[Duration]) -> Self {
        let mut sorted = runs.to_vec();
        sorted.sort_unstable();
        Self(sorted)
    }

    fn at(&self, percentile: usize) -> Duration {
        let last = self.0.len().saturating_sub(1);
        self.0[(self.0.len() * percentile / 100).min(last)]
    }
}

/// Run something and hold on to how long the whole invocation took.
///
/// `--version` because it is the shortest thing any of these runtimes does and the first thing a
/// person types — and because `fakeservice` answers it by printing a line and returning, so what is
/// timed on either side is process creation plus one `println!`.
fn timed(program: &Path, cwd: &Path, home: &Path) -> (Duration, Output) {
    let mut command = Command::new(program);
    command
        .arg("--version")
        .current_dir(cwd)
        // On the child and not on this process, and on **both** sides: an environment block is
        // copied into every process created, so giving one side a variable the other lacks would put
        // a difference into the thing being measured.
        .env("MIXENGINE_HOME", home);

    let started = Instant::now();
    let output = command
        .output()
        .unwrap_or_else(|error| panic!("run {}: {error}", program.display()));

    (started.elapsed(), output)
}

/// That the run being timed was a run which did the work.
fn really_ran(output: &Output, which: &str, shape: &str) {
    let said = String::from_utf8_lossy(&output.stdout).into_owned();

    assert!(
        output.status.success(),
        "{shape}: {which} did not run: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert!(
        said.contains("fakeservice"),
        "{shape}: {which} printed something else, so it is not the program this measures: {said}"
    );
}

/// A duration as a person reads one, to two decimal places of a millisecond.
fn millis(span: Duration) -> String {
    format!("{:.2} ms", span.as_secs_f64() * 1_000.0)
}
