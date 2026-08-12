//! `setsid` from `unix/`, plus the one thing only Linux can do about a parent that dies.
//!
//! Everything a detached child needs is POSIX and is next door. What is here is the *supervised*
//! side, and it is here rather than in `unix/` because `PR_SET_PDEATHSIG` is a Linux invention that
//! macOS has no equivalent of — the case the "anything the two do differently stays in their own
//! directory" rule exists for.
//!
//! **It is a guard, not a guarantee**, and the difference is written down in
//! `.claude/decisions/0007-supervised-child-owns-a-process-group.md` rather than quietly averaged
//! into the Windows promise. `PR_SET_PDEATHSIG` reaches the process we started and nothing it
//! starts, so a php-fpm master dies and its pool workers are reparented to init; it is keyed to the
//! parent *thread*, so in a daemon with a thread pool it arrives when whichever worker did the spawn
//! exits rather than when the daemon does; and it is cleared across a setuid `exec`. What covers the
//! rest is crash recovery at the next boot (roadmap task T18), which has to exist anyway for the
//! machine that lost power.

use std::io;
use std::os::unix::process::CommandExt as _;
use std::process::Command;

// The rest is POSIX and identical on both systems, so this module adds to `unix/` rather than
// replacing it. `Group` is re-exported by name, which is what carries its `adopt` and `terminate`
// along with it.
pub(crate) use crate::unix::process::{
    CAN_ASK_TO_STOP, Detaching, Group, INHERITED_ENV, detach, group, hide_stdio,
};

/// Start a supervised child in a session of its own, and ask the kernel to kill it if we die.
///
/// Both halves are registered on the same `Command` and run in that order in the child, which is the
/// order they have to be in: `setsid` cannot fail for a freshly forked child, while the second half
/// may decide the child should not exist at all.
pub(crate) fn arrange(command: &mut Command) {
    crate::unix::process::new_session(command);
    die_with_the_parent(command);
}

/// `SIGKILL` when the parent goes, with the race that comes with it closed.
///
/// The race is the reason this is longer than one call. `prctl` can only be asked for by the child,
/// after the fork — and a parent that dies in the window between the fork and that call has already
/// delivered whatever signal there was to deliver, so the child would simply never hear about it and
/// would run forever. The `getppid` check afterwards is what makes that window empty rather than
/// small: the parent's id was read before the fork, so a child that no longer has that parent knows
/// it has already missed the notification and ends itself.
///
/// Reparenting is what makes the check sound. A child whose parent has gone is handed to init or to
/// the nearest subreaper, so `getppid` returns something that is *not* the id we recorded — there is
/// no state in which a dead parent still answers.
///
/// Returning an error rather than calling `_exit` leaves the ending to the standard library, which
/// is already the thing that turns a failed `pre_exec` into a child that never becomes the program
/// and a parent whose `spawn` says why. That the parent is in no state to read it is the point.
fn die_with_the_parent(command: &mut Command) {
    let parent = std::process::id();

    #[expect(
        unsafe_code,
        reason = "both calls are async-signal-safe, take integers by value and touch no memory of \
                  ours, which is the whole of pre_exec's safety contract; `parent` is a u32 copied \
                  into the closure before the fork"
    )]
    unsafe {
        command.pre_exec(move || {
            if libc::prctl(libc::PR_SET_PDEATHSIG, libc::SIGKILL) == -1 {
                return Err(io::Error::last_os_error());
            }

            // Read after the request, never before: asking first and checking second is what makes
            // the window between them empty. A parent that dies after this line has already armed
            // the signal.
            if libc::getppid() != parent as libc::pid_t {
                return Err(io::Error::other(
                    "the supervising process was gone before the child could ask to be killed with \
                     it",
                ));
            }

            Ok(())
        });
    }
}
