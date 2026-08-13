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
use std::path::PathBuf;
use std::process::Command;

use crate::{Error, Result};

// The rest is POSIX and identical on both systems, so this module adds to `unix/` rather than
// replacing it. `Group` is re-exported by name, which is what carries its `adopt` and `terminate`
// along with it. Stopping a process this one did not start is POSIX too — the same `kill` to a
// negated pid — so both halves of it come from there as well; only *reading* when a process began
// is per-system, because `/proc` is Linux's and macOS has no such file.
pub(crate) use crate::unix::process::{
    CAN_ASK_TO_STOP, Detaching, Group, INHERITED_ENV, arrange_one_shot, ask_foreign_to_stop,
    detach, group, hide_stdio, stop_foreign,
};

/// When the process with this id began, in clock ticks since the machine booted.
///
/// Field 22 of `/proc/<pid>/stat`, which is the number every Unix supervisor identifies a process
/// by: the kernel writes it once and it is the same for as long as the process lives, so a pid the
/// OS has handed out again carries a different one and fails the caller's comparison.
///
/// **Boot-relative, and that is the one weakness of the three systems.** Windows and macOS both
/// report a wall-clock moment, which cannot collide across a reboot; a tick count cannot tell a
/// process started 600 seconds into this boot from one started 600 seconds into the last. Nothing
/// here closes that, and the two ways of closing it both cost more than they save: `/proc/stat`'s
/// `btime` is recomputed from the monotonic-to-real offset and *moves* when the clock is stepped, so
/// building a wall-clock moment out of it would refuse to adopt a perfectly healthy service after an
/// NTP correction — trading a rare wrong identification for a rare killed database. The residual is
/// therefore accepted and written down, exactly as
/// `.claude/decisions/0007-supervised-child-owns-a-process-group.md` accepts its pid-recycling race:
/// a collision needs the same pid *and* the same centisecond of two different boots, and the only
/// pids ever compared are ones this product recorded for itself.
///
/// [`None`] for a process that is not running, which is two cases: no `/proc/<pid>` at all, and a
/// zombie — a process that has ended and whose parent has not reaped it. The second matters because
/// the caller's question is always "is what I recorded still there", and a zombie is a row's worth of
/// kernel bookkeeping rather than a service.
///
/// # Errors
///
/// [`Error::Io`] when `/proc/<pid>/stat` is there and cannot be read or does not parse. The second
/// is a kernel whose format this build does not know, and inventing an answer for it would mean
/// either signalling a process on a number nobody understood or reporting a running service as gone.
pub(crate) fn started_at(pid: u32) -> Result<Option<i64>> {
    let path = PathBuf::from(format!("/proc/{pid}/stat"));

    let stat = match std::fs::read_to_string(&path) {
        Ok(stat) => stat,

        // The ordinary answer for a pid that is not in use. `ESRCH` never appears here: a directory
        // that is not there is simply not found.
        Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(None),

        Err(source) => {
            return Err(Error::Io {
                action: "ask when a process began by reading",
                path,
                source,
            });
        }
    };

    // Split at the **last** `)` rather than tokenising from the start: field 2 is the executable
    // name in parentheses and may contain both spaces and parentheses of its own, which is the one
    // trap this file is famous for. Everything after it is space-separated and starts at field 3.
    let Some((_, rest)) = stat.rsplit_once(')') else {
        return Err(unreadable(path, "no `)` ends the second field"));
    };

    let fields: Vec<&str> = rest.split_whitespace().collect();

    // Field 3 is the state, and `Z` is a process that has ended.
    match fields.first() {
        Some(&"Z") => return Ok(None),
        Some(_) => {}
        None => return Err(unreadable(path, "there is nothing after the second field")),
    }

    // Field 22, counted from the start of the line, is the twentieth of what is left.
    let Some(started) = fields.get(19) else {
        return Err(unreadable(path, "it has fewer than 22 fields"));
    };

    started
        .parse::<i64>()
        .map(Some)
        .map_err(|_| unreadable_start_time(started))
}

/// A `/proc/<pid>/stat` this build cannot read, blamed on the file rather than on the caller.
fn unreadable(path: PathBuf, why: &str) -> Error {
    Error::Io {
        action: "ask when a process began by reading",
        path,
        source: io::Error::new(
            io::ErrorKind::InvalidData,
            format!("this is not a /proc stat line this build understands: {why}"),
        ),
    }
}

/// The same, for the one field that is read out of it.
fn unreadable_start_time(field: &str) -> Error {
    Error::Os {
        action: "read the start time of a process",
        source: io::Error::new(
            io::ErrorKind::InvalidData,
            format!("field 22 of its /proc stat line is `{field}`, which is not a number of ticks"),
        ),
    }
}

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
