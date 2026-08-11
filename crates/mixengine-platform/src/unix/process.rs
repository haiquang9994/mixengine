//! `setsid` between the fork and the exec.
//!
//! Surviving the parent needs nothing on Unix — an orphan is reparented to init and carries on. What
//! does need arranging is the *session*: a child inherits its parent's process group, and a terminal
//! sends `SIGINT` to the whole foreground group, so a daemon that stayed in it would die the next
//! time somebody pressed Ctrl-C in the window it was started from. `setsid` puts it in a new session
//! with no controlling terminal at all, which is the difference between a background process and a
//! detached one.

use std::io;
use std::os::unix::process::CommandExt as _;
use std::process::Command;

/// Nothing this process has to undo once the spawn is over.
///
/// Detaching is arranged entirely inside the child here, so unlike Windows — where it means clearing
/// an inheritance flag on the *parent's* own handles for the length of the spawn — there is no state
/// to put back. The type exists so `process.rs` has one shape to hold across the spawn on both
/// systems.
#[derive(Debug)]
pub(crate) struct Detaching;

/// Arrange for the child to leave this process's session.
///
/// `pre_exec` runs in the child after `fork` and before `exec`, which is the only moment this can
/// be done and the most constrained code in the crate: between those two calls the child has one
/// thread and whatever locks the parent's other threads happened to be holding, so only
/// async-signal-safe calls are allowed. `setsid` is one, takes no arguments, allocates nothing and
/// can only fail if the caller is already a process group leader — which a freshly forked child
/// never is.
pub(crate) fn detach(command: &mut Command) -> Detaching {
    #[expect(
        unsafe_code,
        reason = "the closure calls one async-signal-safe libc function and touches no memory, \
                  which is the whole of pre_exec's safety contract"
    )]
    unsafe {
        command.pre_exec(|| {
            if libc::setsid() == -1 {
                return Err(io::Error::last_os_error());
            }

            Ok(())
        });
    }

    Detaching
}
