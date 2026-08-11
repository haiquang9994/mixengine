//! Being asked to stop, against the real OS.
//!
//! **Unix only, and the gap is deliberate.** Proving this means sending the process a real stop
//! request, and on Unix `raise` sends one to *this* process and nothing else. Windows has no
//! equivalent: `GenerateConsoleCtrlEvent` addresses a process group, so the console event would
//! reach `cargo test` itself and every other test binary sharing that console — a test that
//! terminates the runner proves nothing about the daemon. What covers the Windows half instead is
//! `mixengine-daemon/tests/lifecycle.rs`, which stops a real daemon in a process of its own.
//!
//! One test, not several: the handlers are process-global, so two of these running at the same time
//! under `cargo test`'s thread pool would be one test's signal arriving in another's receiver.

#![cfg(unix)]

use std::time::Duration;

use mixengine_platform::signal::{Signals, Stop};

#[tokio::test]
async fn sigterm_is_a_request_to_stop() {
    // Registered *before* the signal is raised, which is what makes this safe to run inside a test
    // binary: until tokio installs its handler the default action for SIGTERM is to kill the
    // process, and the process here is the test runner.
    let mut signals = Signals::listen().expect("this system installs signal handlers");

    #[expect(
        unsafe_code,
        reason = "raise takes a signal number and touches no memory; there is no safe binding for \
                  it, and sending a real signal is the entire content of this test"
    )]
    let raised = unsafe { libc::raise(libc::SIGTERM) };
    assert_eq!(raised, 0, "the signal was sent");

    // A timeout rather than a bare await: a handler that never fires would otherwise hang the whole
    // test binary instead of failing this test.
    let stop = tokio::time::timeout(Duration::from_secs(5), signals.stopped())
        .await
        .expect("the signal reached the handler");

    assert_eq!(stop, Stop::Terminate);
    // The daemon puts this in the log line that explains a shutdown hours later.
    assert_eq!(stop.to_string(), "a request to stop");
}
