//! Capturing what a real process prints, against `fakeservice`.
//!
//! The unit tests beside `logs.rs` are about framing — where a line ends, what a stray byte costs —
//! and answer it from a slice. These are about the part a slice cannot show: two pipes, two threads,
//! a process that is still running, and the ordinary hazard of a pipe that fills up while nobody
//! reads it.
//!
//! Not `#[ignore]`d: the only thing touched is a child process this test starts and stops.

use std::collections::BTreeMap;
use std::time::{Duration, Instant};

use mixengine_platform::process::{Supervised, spawn_supervised};
use mixengine_proto::{LogPolicy, ServiceId};
use mixengine_supervisor::logs::{Capture, Stream};
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
    mut accept: impl FnMut(&mixengine_supervisor::LogLine) -> bool,
) -> Vec<mixengine_supervisor::LogLine> {
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
    let mut capture = Capture::start(&mut supervised, &service(), LogPolicy::default());

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
    let mut capture = Capture::start(&mut supervised, &service(), LogPolicy::default());
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
    let mut capture = Capture::start(&mut supervised, &service(), LogPolicy::default());

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
    let mut capture = Capture::start(&mut supervised, &service(), policy);

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
