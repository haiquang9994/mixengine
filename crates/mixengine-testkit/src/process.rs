//! Stopping a process this test is not the parent of.
//!
//! **The one `#[cfg]` outside `mixengine-platform` that decides what a program *does*.** Test
//! functions elsewhere are gated to the systems they can run on — `tests/fakeservice.rs` has two —
//! and that is a different thing: it says where a claim is checkable, not how it is answered. This
//! is a body that differs by OS. It is here rather than
//! behind a platform trait because nothing in the *product* stops a process by pid yet — that
//! arrives with the supervisor (roadmap task T15), and is where this belongs once it exists. A test
//! cannot wait for it: what several of them prove is precisely that a daemon somebody *else* stopped
//! shuts down properly, and a daemon a client autostarted is nobody's child to be killed through a
//! `std::process::Child`.
//!
//! It is affordable because this crate is a dev-dependency and ships to nobody. The rule it bends is
//! about the code that runs on a user's machine; obeying it here would mean adding a capability to
//! `mixengine-platform` that only tests call, which is the same `#[cfg]` in a worse place.

use std::process::{Command, Stdio};

/// Ask the process with this id to stop, the way this operating system asks. `false` if it was not
/// there to stop.
///
/// Unix gets `SIGTERM`, which is the graceful path. Windows gets `taskkill /F`, which is not: a
/// process started with `DETACHED_PROCESS` has no console for a control event to be delivered
/// through, so there is nothing gentler to send it from out here. What both prove is the part that
/// has to hold either way — the endpoint stops answering and the lock is released.
///
/// # Not a liveness check
///
/// `false` means "there was no pid here", which is not quite "there was no *process* here". On Unix
/// `kill` succeeds against a zombie, so a child that has exited and not been reaped still answers
/// yes. That is sound for what this exists for — a process this test is not the parent of, which
/// this test therefore cannot be leaving unreaped — and wrong for anything a test spawned itself and
/// is still holding, where [`Running::still_running`](crate::service::Running::still_running) is the
/// answer instead.
///
/// # Panics
///
/// If this system cannot be asked to stop a process at all, which is not a state a test can report
/// anything useful from.
#[must_use = "a process that was not there to stop is usually the thing under test"]
pub fn try_stop(pid: u32) -> bool {
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

    command
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status()
        .expect("this system can stop a process")
        .success()
}

/// [`try_stop`], for a caller that knows the process is there.
///
/// # Panics
///
/// If the process could not be stopped — including because it had already gone, which for a caller
/// holding a pid it just read is a fact worth failing on rather than tidying away. Use
/// [`try_stop`] where that is expected, and in a `Drop`, where a panic while unwinding aborts the
/// whole run.
pub fn stop(pid: u32) {
    assert!(try_stop(pid), "pid {pid} could not be stopped");
}
