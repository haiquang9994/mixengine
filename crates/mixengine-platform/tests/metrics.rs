//! What a group of processes is costing, measured against this machine rather than against a table
//! a test wrote.
//!
//! Roadmap task **T71** built the sampler and its unit tests drive [`Snapshot::aggregate`] from
//! invented rows, which is what makes the minute arithmetic testable. What no invented row can catch
//! is what the *collection* puts in front of it — and that is what this file is for.

use std::process;

use mixengine_platform::{GroupRoot, process::started_at};

/// This test process, identified the way every subject is: by pid and the moment it began.
fn me() -> GroupRoot {
    let pid = process::id();

    GroupRoot {
        pid,
        started: started_at(pid)
            .expect("this machine can be asked when a process began")
            .expect("this process began at some point"),
    }
}

/// **One process is one process, however many threads it is running** — roadmap task **T71a**'s
/// prerequisite, found by T72.
///
/// A test binary is a tokio runtime with a worker per core and nothing spawned under it, so its
/// group is exactly one process. On Linux `sysinfo` lists **threads alongside processes**, each
/// carrying its parent's parent pid and its parent's whole resident size — so a group walked without
/// filtering counts one process once per thread, and its `rss_bytes` comes out multiplied by the
/// thread count.
///
/// That is not a small error: a Caddy with ten threads read as 445 MB, and a php-fpm pool would have
/// been restarted by T71a's memory watchdog for a ceiling it was nowhere near. `processes` is the
/// sharper assertion of the two — it fails by a factor, where a memory figure needs a threshold
/// somebody has to choose.
#[test]
fn a_process_with_many_threads_is_still_one_process() {
    let readings = mixengine_platform::host()
        .process_metrics()
        .measure(&[me()]);

    assert_eq!(readings.len(), 1, "this process is measurable");

    assert_eq!(
        readings[0].processes, 1,
        "this test spawned nothing, so its group is one process — a larger number means threads \
         were counted as processes, and every one of them added the whole process's memory again"
    );
}

/// A reading of this process is a plausible size, which is the other half of the same bug.
///
/// **A ceiling rather than an exact figure**, because what a test binary holds is the test binary's
/// business: 2 GB is far above anything this could legitimately be and far below what the
/// thread-counting bug produced on a machine with enough cores.
#[test]
fn a_reading_of_this_process_is_a_plausible_size() {
    let readings = mixengine_platform::host()
        .process_metrics()
        .measure(&[me()]);

    assert!(
        readings[0].rss_bytes < 2 * 1024 * 1024 * 1024,
        "this process reads as {} bytes, which is not a size a test binary is",
        readings[0].rss_bytes
    );
}

/// A pid this machine handed to something else produces no reading at all.
///
/// The other half of what identity by pid *and* start time buys, asserted here against the real
/// collection rather than against a table: asking about this process at a moment it did not begin is
/// asking about a stranger.
#[test]
fn a_group_root_whose_start_time_does_not_match_is_not_measured() {
    let mut stranger = me();
    stranger.started = mixengine_platform::process::StartTime::from_stored(1);

    let readings = mixengine_platform::host()
        .process_metrics()
        .measure(&[stranger]);

    assert!(
        readings.is_empty(),
        "a pid whose start time does not match is a stranger, not a subject"
    );
}
