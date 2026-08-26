//! Capturing what a real process prints, against `fakeservice`.
//!
//! The unit tests beside `logs.rs` are about framing — where a line ends, what a stray byte costs —
//! and answer it from a slice. These are about the part a slice cannot show: two pipes, two threads,
//! a process that is still running, and the ordinary hazard of a pipe that fills up while nobody
//! reads it.
//!
//! Not `#[ignore]`d: the only thing touched is a child process this test starts and stops.

use std::collections::BTreeMap;
use std::path::Path;
use std::time::{Duration, Instant};

use mixengine_platform::process::{Limits, Supervised, spawn_supervised};
use mixengine_proto::{LogLine, LogPolicy, ServiceId, Stream};
use mixengine_supervisor::logs::{CURRENT_LOG_FILE_NAME, Capture};
use mixengine_testkit::FakeService;
use mixengine_testkit::service::READY_LINE;

/// How long a test waits for a process on the other side of the machine to say something.
///
/// Only ever waited out in full when something is wrong. Generous because a process start on a
/// loaded Windows runner is measured in seconds.
const EVENTUALLY: Duration = Duration::from_secs(20);

fn service() -> ServiceId {
    ServiceId::parse("fakeservice").expect("a valid id")
}

/// Start `fixture` supervised, the way the supervisor will.
fn supervised(fixture: FakeService) -> Supervised {
    spawn_supervised(
        &FakeService::program(),
        fixture.args(),
        &std::env::temp_dir(),
        &BTreeMap::new(),
        &Limits::default(),
    )
    .expect("a fakeservice can be supervised")
}

/// Wait until the ring holds a line the predicate accepts, and hand back everything in it.
///
/// # Panics
///
/// If no such line arrived within [`EVENTUALLY`].
fn wait_for(
    capture: &Capture,
    what: &str,
    mut accept: impl FnMut(&LogLine) -> bool,
) -> Vec<LogLine> {
    let deadline = Instant::now() + EVENTUALLY;

    loop {
        let lines = capture.recent(usize::MAX);

        if lines.iter().any(&mut accept) {
            return lines;
        }

        assert!(
            Instant::now() < deadline,
            "no {what} arrived within {EVENTUALLY:?}; what did: {lines:?}"
        );

        std::thread::sleep(Duration::from_millis(10));
    }
}

#[test]
fn what_a_service_prints_is_captured_from_both_streams() {
    let mut supervised = supervised(FakeService::new().log_every(20).log_to_stderr());
    let mut capture = Capture::start(&mut supervised, &service(), LogPolicy::default(), None);

    // A numbered line rather than any line: the ready line is written to both streams, so waiting
    // for "something on stderr" would return before the ticker had produced anything at all.
    let lines = wait_for(&capture, "numbered line on stderr", |line| {
        line.stream == Stream::Stderr && line.text.starts_with("fakeservice: line")
    });

    assert!(
        lines.iter().any(|line| line.text == READY_LINE),
        "the ready line was not captured: {lines:?}"
    );
    assert!(
        lines
            .iter()
            .any(|line| line.stream == Stream::Stdout && line.text.starts_with("fakeservice: line")),
        "nothing arrived on stdout: {lines:?}"
    );
    assert!(
        lines.iter().all(|line| !line.text.ends_with('\r')),
        "a CRLF terminator reached the captured text: {lines:?}"
    );

    supervised.stop().expect("the service can be stopped");
    assert!(
        capture.finish(EVENTUALLY),
        "the streams did not reach end of file after the service had gone"
    );
}

/// A subscriber is how a `ReadyCheck::LogPattern` waits for a line that has not been printed yet.
#[tokio::test]
async fn a_subscriber_sees_lines_as_they_arrive() {
    let mut supervised = supervised(FakeService::new().ready_after(50).log_every(20));
    let mut capture = Capture::start(&mut supervised, &service(), LogPolicy::default(), None);
    let mut lines = capture.subscribe();

    let announced = tokio::time::timeout(EVENTUALLY, async {
        loop {
            match lines.recv().await {
                Ok(line) if line.text == READY_LINE => return line,
                // A subscriber that fell behind keeps waiting: the pattern may have been in what it
                // missed, and the timeout is what ends the wait either way.
                Ok(_) | Err(tokio::sync::broadcast::error::RecvError::Lagged(_)) => {}
                Err(closed) => {
                    panic!("the capture ended before the service said anything: {closed}")
                }
            }
        }
    })
    .await
    .expect("the ready line arrives");

    assert_eq!(announced.stream, Stream::Stdout);

    supervised.stop().expect("the service can be stopped");
    assert!(
        capture.finish(EVENTUALLY),
        "the streams did not reach end of file after the service had gone"
    );
}

/// The hazard this module exists to remove: a pipe holds tens of kilobytes and then the *service*
/// blocks on its next line, looking exactly like one that has hung.
///
/// A capture that stopped reading would leave this fixture stuck long before its `--exit-after`, so
/// a run that reaches the exit at all is the assertion.
#[test]
fn a_service_that_prints_more_than_a_pipe_holds_is_not_stalled_by_it() {
    let mut supervised = supervised(
        FakeService::new()
            .log_every(1)
            .log_to_stderr()
            .exit_after(1_500)
            .exit_code(0),
    );
    let mut capture = Capture::start(&mut supervised, &service(), LogPolicy::default(), None);

    let exit = supervised.wait().expect("the service can be waited for");
    assert!(
        capture.finish(EVENTUALLY),
        "the streams did not reach end of file after the service had gone"
    );

    assert!(
        exit.is_success(),
        "the service did not reach its own exit: {exit}"
    );

    let lines = capture.recent(usize::MAX);
    assert!(
        lines.len() > 100,
        "a service printing every millisecond for a second and a half produced {} lines, so \
         something stopped draining its pipes",
        lines.len()
    );
}

/// The ring is bounded by the policy, whatever the service does.
#[test]
fn the_ring_holds_only_what_the_policy_asked_for() {
    let policy = LogPolicy {
        ring_lines: 5,
        ..LogPolicy::default()
    };

    let mut supervised = supervised(FakeService::new().log_every(1).exit_after(500).exit_code(0));
    let mut capture = Capture::start(&mut supervised, &service(), policy, None);

    supervised.wait().expect("the service can be waited for");
    assert!(
        capture.finish(EVENTUALLY),
        "the streams did not reach end of file after the service had gone"
    );

    let lines = capture.recent(usize::MAX);

    assert_eq!(lines.len(), 5, "the ring outgrew its policy: {lines:?}");
    assert!(
        lines
            .last()
            .is_some_and(|line| line.text.starts_with("fakeservice: line")),
        "the lines kept are not the last ones: {lines:?}"
    );
}

/// Run a fixture to completion with its output going to `directory`, and hand back the capture.
fn logged_to(directory: &Path, policy: LogPolicy, fixture: FakeService) -> Capture {
    let mut supervised = supervised(fixture);
    let mut capture = Capture::start(&mut supervised, &service(), policy, Some(directory));

    supervised.wait().expect("the service can be waited for");
    assert!(
        capture.finish(EVENTUALLY),
        "the streams did not reach end of file after the service had gone"
    );

    capture
}

/// Whether a line in `current.log` is one the fixture printed rather than one of ours.
///
/// The file is the upstream program's output and nothing else, so this is the whole allowed
/// vocabulary: what `fakeservice` writes, the line it announces itself with, and the blank line a
/// text editor leaves at the end.
fn the_services_own(line: &str) -> bool {
    line.is_empty() || line.starts_with("fakeservice") || line == READY_LINE
}

/// The file is the third reader of one stream, not a second copy of it: everything the ring kept is
/// in it, from both streams, in the order it was printed.
#[test]
fn what_a_service_prints_is_written_to_its_log_file() {
    let home = tempfile::TempDir::new().expect("a temporary directory");
    // The shape `Paths::service_logs` builds, created here by the capture and not by the caller.
    let directory = home.path().join("services").join("fakeservice");

    let capture = logged_to(
        &directory,
        LogPolicy::default(),
        FakeService::new()
            .log_every(20)
            .log_to_stderr()
            .exit_after(500)
            .exit_code(0),
    );

    let written = std::fs::read_to_string(directory.join(CURRENT_LOG_FILE_NAME))
        .expect("the service's log file was opened when it started");
    let on_disk: Vec<&str> = written.lines().collect();
    let kept = capture.recent(usize::MAX);

    assert!(
        kept.len() > 4,
        "the fixture printed too little to prove anything: {kept:?}"
    );
    for line in &kept {
        assert!(
            on_disk.contains(&line.text.as_str()),
            "a line the ring kept never reached the file: {line:?}"
        );
    }
    assert!(
        kept.iter().any(|line| line.stream == Stream::Stdout)
            && kept.iter().any(|line| line.stream == Stream::Stderr),
        "the fixture did not use both streams, so one file for both is untested: {kept:?}"
    );

    // Plain text and nothing of ours: no timestamp, no `[stderr]`, and no CR to make a Windows line
    // differ from the same line on Linux.
    assert!(!written.contains('\r'), "{written:?}");
    assert!(
        on_disk.iter().copied().all(the_services_own),
        "something that is not the service's own output reached its log file: {on_disk:?}"
    );
}

/// The policy bounds the disk as well as the memory, and by the same rule `daemon.log` follows.
#[test]
fn the_log_file_rotates_and_keeps_only_what_the_policy_asked_for() {
    let home = tempfile::TempDir::new().expect("a temporary directory");
    let directory = home.path().join("fakeservice");
    let policy = LogPolicy {
        // Tiny on purpose: rotation is about crossing a boundary, and a test that writes ten
        // megabytes to reach one proves nothing extra.
        max_file_bytes: 128,
        max_files: 2,
        ..LogPolicy::default()
    };

    logged_to(
        &directory,
        policy,
        FakeService::new().log_every(1).exit_after(500).exit_code(0),
    );

    let live = directory.join(CURRENT_LOG_FILE_NAME);
    let numbered = |index: u8| directory.join(format!("{CURRENT_LOG_FILE_NAME}.{index}"));

    let length = std::fs::metadata(&live).expect("the live file").len();
    assert!(
        length <= policy.max_file_bytes + 128,
        "the live file grew to {length} bytes with a {} byte limit",
        policy.max_file_bytes
    );
    assert!(numbered(1).is_file(), "nothing was rotated aside");
    assert!(
        numbered(2).is_file(),
        "the history is shorter than `max_files`"
    );
    assert!(
        !numbered(3).exists(),
        "the history grew past `max_files` copies"
    );
}

/// The rule the architecture states from the other side: a rotation that cannot happen costs no log
/// lines *and* leaves nothing of MixEngine's in the service's own file.
///
/// `daemon.log` is given that note in `log.format`'s shape, and the daemon proves it in its own
/// tests. Here the same `RotatingFile` must stay silent, because this file is met by whoever greps
/// MariaDB's or Caddy's log for the program's own messages.
#[test]
fn a_rotation_that_cannot_happen_leaves_nothing_of_ours_in_the_file() {
    let home = tempfile::TempDir::new().expect("a temporary directory");
    let directory = home.path().join("fakeservice");
    std::fs::create_dir_all(&directory).expect("the log directory can be created");
    // A directory where the rotated copy has to go: `rename` cannot replace one with a file on any
    // of the three platforms, and it is the closest portable stand-in for the real cause — a file
    // another process is holding open, which only Windows would refuse.
    std::fs::create_dir(directory.join(format!("{CURRENT_LOG_FILE_NAME}.1")))
        .expect("the blocking directory can be created");

    let policy = LogPolicy {
        max_file_bytes: 128,
        // A history of one, so the very first rename a rotation attempts is the one that fails.
        max_files: 1,
        ..LogPolicy::default()
    };

    logged_to(
        &directory,
        policy,
        FakeService::new().log_every(1).exit_after(500).exit_code(0),
    );

    let live = directory.join(CURRENT_LOG_FILE_NAME);
    let written = std::fs::read_to_string(&live).expect("the service's log file");

    assert!(
        written.len() as u64 > policy.max_file_bytes,
        "the file never reached its limit, so no rotation was ever attempted: {written:?}"
    );
    assert!(
        written.lines().all(the_services_own),
        "a sentence of ours was written into the service's own log: {written:?}"
    );
}

/// A log file that cannot be opened costs the file and nothing else — the service keeps running and
/// stays captured, which is the trade `Capture::start` documents.
#[test]
fn a_log_file_that_cannot_be_opened_does_not_stop_the_capture() {
    let home = tempfile::TempDir::new().expect("a temporary directory");
    let blocked = home.path().join("logs");
    // A file where the directory has to go: `create_dir_all` refuses on all three platforms, and it
    // stands in for the real causes — a full disk, a `[paths] logs` override onto a read-only mount.
    std::fs::write(&blocked, b"not a directory").expect("the blocking file can be written");

    let capture = logged_to(
        &blocked.join("fakeservice"),
        LogPolicy::default(),
        FakeService::new()
            .log_every(20)
            .exit_after(300)
            .exit_code(0),
    );

    assert!(
        !capture.recent(usize::MAX).is_empty(),
        "the capture stopped reading because the file could not be opened"
    );
}
