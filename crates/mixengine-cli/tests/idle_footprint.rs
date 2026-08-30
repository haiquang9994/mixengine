//! What MixEngine costs when nobody is using it — roadmap task **T72**, measured.
//!
//! `../features/resource-isolation.md` publishes a number: *"Idle footprint (daemon + Caddy, nothing
//! else running): target < 60 MB RSS, ~0 % CPU."* Nothing in this repository measured it until this
//! file, and it is exactly the kind of number that decays quietly: no single commit makes a daemon
//! fat.
//!
//! # What is measured, and through what
//!
//! The sum of `rss_bytes` over every subject `mix metrics --json` reports — which is the daemon's own
//! sampler from T71, and therefore the same number a person reads off their own machine. Not a
//! reading of this machine taken beside the daemon: one mechanism, one answer, and this is the one
//! the feature document is about.
//!
//! **The subject set is asserted before the number is.** A home where Caddy failed to start reports
//! one subject and a very good total, and the budget alone would call that a pass — which is this
//! measurement's failure that reads as success.
//!
//! # What this measurement honestly is not
//!
//! It is not *"after 30 idle minutes"*. The daemon being read has just installed a package, rendered
//! configuration and walked a start plan, and its RSS carries the high-water mark of all of it — an
//! allocator returns that to the operating system slowly, or not at all. A daemon that has been up
//! for an afternoon holds less.
//!
//! **That makes the number worse than a real idle machine's, which is why it is acceptable**:
//! passing at 60 MB here means passing comfortably there. A budget that errs strict is a budget
//! doing its job.
//!
//! Restarting the daemon and letting the next one adopt Caddy would be much closer to an idle
//! machine, and cannot be done on two of the three systems: per [ADR
//! 0007](../../../.claude/decisions/0007-supervised-child-owns-a-process-group.md), a daemon leaving
//! takes its whole job down on Windows and its immediate children on Linux, so there would be
//! nothing left to adopt and this would measure a daemon standing alone.
//!
//! # Release, ignored
//!
//! `#[ignore]`d because this belongs to the `bench` job rather than to `test`, on `warm_start.rs`'s
//! reasoning: a number a loaded runner can move should not stand between a correctness suite and its
//! answer. The budget is asserted **only in a release build** — a debug daemon is a different
//! program, and a number measured there is about the profile rather than about the design. A debug
//! run still measures and still prints.

mod harness;

use std::time::Duration;

use harness::frontend::{CADDY, declared};
use harness::json;

/// The budget `features/resource-isolation.md` publishes, in bytes.
const BUDGET: u64 = 60 * 1024 * 1024;

/// How long the home is left alone before the first reading.
///
/// Thirty seconds: the scaled-down version of the promise's *thirty minutes*, long enough that the
/// install and the start walk have returned what they are going to return, short enough that a bench
/// job can afford it. See the module note for what the difference between thirty seconds and thirty
/// minutes costs, and why it costs it in the safe direction.
const SETTLE: Duration = Duration::from_secs(30);

/// How many readings the median is taken over, a second apart.
///
/// Answering a snapshot walks the process table — about 10 ms on Windows, measured at T71 — and the
/// daemon is one of the subjects, so each reading perturbs what it reads a little. A median is what
/// buys that off, and it is what both other budgets in this job take.
const READINGS: usize = 5;

/// What one `mix metrics --json` said: which subjects it named, and what they add up to.
#[derive(Debug)]
struct Reading {
    /// The wire spelling of each subject, sorted: `daemon`, `service:<id>`.
    subjects: Vec<String>,

    /// Their `rss_bytes`, summed.
    total: u64,
}

/// One snapshot, through the shipped client.
///
/// **Through `mix` rather than through the socket**, because the number this gates should be the
/// number a person can read for themselves — a suite reaching past the client could gate a figure no
/// command prints.
fn reading(home: &harness::Home) -> Reading {
    let frame = json(&home.mix(&["metrics", "--json"]));

    let samples = frame["samples"]
        .as_array()
        .unwrap_or_else(|| panic!("a snapshot carries samples: {frame}"))
        .clone();

    let mut subjects: Vec<String> = samples
        .iter()
        .map(|sample| sample["subject"].as_str().unwrap_or_default().to_owned())
        .collect();
    subjects.sort();

    let total = samples
        .iter()
        .map(|sample| sample["rss_bytes"].as_u64().unwrap_or_default())
        .sum();

    Reading { subjects, total }
}

/// **A home running nothing but the daemon and its web server costs less than we publish.**
#[tokio::test(flavor = "multi_thread")]
#[ignore = "a budget, measured by the bench job — see the module note and ci.yml"]
async fn a_home_with_nothing_but_the_daemon_and_the_web_server_stays_inside_its_budget() {
    let (home, _daemon, _registry, _site, _control) = declared(&CADDY).await;

    let started = home.mix(&["service", "start", CADDY.package, "--json"]);
    assert!(
        started.status.success(),
        "the web server has to be running for this to be the footprint we publish: {}\n{}",
        harness::stderr(&started),
        home.daemon_log()
    );

    tokio::time::sleep(SETTLE).await;

    let mut totals = Vec::with_capacity(READINGS);

    for round in 0..READINGS {
        let taken = reading(&home);

        // **Every round, not once.** A Caddy that died between the settle and the last reading
        // would otherwise leave a very good number behind it.
        assert_eq!(
            taken.subjects,
            vec!["daemon".to_owned(), format!("service:{}", CADDY.package)],
            "round {round} measured the wrong set of processes, so its total is about something \
             else: {taken:?}"
        );

        totals.push(taken.total);

        tokio::time::sleep(Duration::from_secs(1)).await;
    }

    totals.sort_unstable();
    let median = totals[totals.len() / 2];

    println!(
        "\n[t72] idle footprint (daemon + {}), median of {READINGS}: {:.1} MB\n[t72]   each: {:?} \
         MB\n[t72]   budget: {:.0} MB\n",
        CADDY.package,
        as_mb(median),
        totals.iter().copied().map(as_mb).collect::<Vec<f64>>(),
        as_mb(BUDGET),
    );

    // **Release only**, on `warm_start.rs`'s rule: a debug daemon is a different program — it
    // measured 90 MB on the machine where this was written — and a number taken there is about the
    // profile rather than about the design.
    if !cfg!(debug_assertions) {
        assert!(
            median <= BUDGET,
            "the idle footprint is {:.1} MB, over the {:.0} MB this project publishes",
            as_mb(median),
            as_mb(BUDGET),
        );
    }
}

/// Bytes as megabytes, for the one line a person reads.
fn as_mb(bytes: u64) -> f64 {
    bytes as f64 / (1024.0 * 1024.0)
}
