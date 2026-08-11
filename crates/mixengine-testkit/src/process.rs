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

/// End the process with this id the way a crash ends one: no handler runs, nothing is tidied up.
/// `false` if it was not there.
///
/// The difference from [`try_stop`] is the whole of what several supervision tests are about. A
/// daemon that *stops* runs its destructors, and one of them takes every supervised child down —
/// so a test using `try_stop` on Unix would prove the destructor works and say nothing about the
/// case that matters. What has to be simulated is a daemon that gets no chance: `SIGKILL`, which no
/// process can catch. Windows has only ever had this one, which is why `try_stop` is already a
/// `taskkill /F` there and this is its twin rather than something stronger.
///
/// # Not a liveness check
///
/// Same caveat as [`try_stop`]: `false` means there was no pid here. Whether a *process* is gone is
/// asked with a lock — see [`FakeService::hold_lock`](crate::FakeService::hold_lock).
///
/// # Panics
///
/// If this system cannot be asked to kill a process at all.
#[must_use = "a process that was not there to kill is usually the thing under test"]
pub fn try_kill(pid: u32) -> bool {
    #[cfg(unix)]
    let mut command = {
        let mut command = Command::new("kill");
        command.args(["-KILL", &pid.to_string()]);
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
        .expect("this system can kill a process")
        .success()
}

/// [`try_kill`], for a caller that knows the process is there.
///
/// # Panics
///
/// If the process could not be killed, including because it had already gone. See [`stop`] for why
/// that is worth failing on rather than tidying away, and for where it is not.
pub fn kill(pid: u32) {
    assert!(try_kill(pid), "pid {pid} could not be killed");
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
