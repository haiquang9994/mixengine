//! `setsid` from `unix/`, and an honest account of what this system does not offer.
//!
//! This module exists to say something rather than to do something. Its Linux counterpart adds
//! `PR_SET_PDEATHSIG` to a supervised spawn; macOS has no equivalent, so a supervised child here is
//! exactly `setsid` and nothing more:
//!
//! - a daemon that **exits** takes its services down, because dropping the supervisor handle kills
//!   the group;
//! - a daemon that is **killed** takes nothing down. The services keep running, and neither the
//!   kernel nor the child will notice.
//!
//! That is a real gap and it is deliberately not papered over — `mix doctor` and the GUI say which
//! of the three platforms they are on rather than repeating a guarantee only Windows keeps.
//! `.claude/decisions/0007-supervised-child-owns-a-process-group.md` records why the alternatives
//! lost: a watchdog inside the child only works for a child we wrote, and MixEngine supervises
//! `php-fpm`, `mariadbd` and `caddy`. What covers the gap instead is crash recovery at the next boot
//! (roadmap task T18) — pid *and* start time, reconciled — which has to exist on every platform
//! anyway for the machine that lost power.

use std::io;
use std::process::Command;

use crate::{Error, Result};

// The rest is POSIX and identical on both systems, so this module adds to `unix/` rather than
// replacing it. `Group` is re-exported by name, which is what carries its `adopt` and `terminate`
// along with it. Stopping a process this one did not start is POSIX too — the same `kill` to a
// negated pid — so both halves of it come from there as well; only *reading* when a process began
// is per-system, because this one has no `/proc`.
pub(crate) use crate::unix::process::{
    CAN_ASK_TO_STOP, CAN_SIGNAL, Detaching, Group, INHERITED_ENV, arrange_one_shot,
    ask_foreign_to_stop, detach, group, hand_over, hide_stdio, stop_foreign,
};

/// When the process with this id began, in microseconds since the epoch.
///
/// **This is the reading the whole of crash recovery rests on here**, because macOS is the system
/// that has no `PR_SET_PDEATHSIG` and no job object: a killed daemon leaves every service it was
/// supervising running, and the daemon that starts next has nothing but a pid and this number to
/// tell what it found. `.claude/decisions/0007-supervised-child-owns-a-process-group.md` is where
/// that gap is recorded, and roadmap task T18 is what closes it.
///
/// `proc_pidinfo` rather than the `sysctl` route to `kinfo_proc`: it asks for the one struct the
/// answer is in, it is the interface Apple documents for this question, and it does not reach a
/// process's start time through a union that means something else in the other half of its
/// lifetime.
///
/// Wall clock, like Windows and unlike Linux, so the number identifies a process across a reboot as
/// well as within one.
///
/// [`None`] for anything that is not a running process: no such pid (`ESRCH`), one this account may
/// not ask about (`EPERM`) — a process this daemon cannot query is not one it started, and answering
/// "no" is what keeps the caller from ever signalling it — and a zombie, which is a process that has
/// ended and whose parent has not reaped it.
///
/// # Errors
///
/// [`Error::Os`] when the OS refuses for any other reason, and when it answers with fewer bytes than
/// the struct it was asked for — a short read is a struct this build cannot trust the fields of.
pub(crate) fn started_at(pid: u32) -> Result<Option<i64>> {
    let mut info: libc::proc_bsdinfo = zeroed_because_every_field_is_a_number();
    let size = size_of::<libc::proc_bsdinfo>();

    #[expect(
        unsafe_code,
        reason = "the buffer is a local this frame owns and the length passed is that local's own \
                  size, so the kernel writes exactly the struct that is there"
    )]
    let written = unsafe {
        libc::proc_pidinfo(
            pid as libc::c_int,
            libc::PROC_PIDTBSDINFO,
            0,
            std::ptr::from_mut(&mut info).cast(),
            size as libc::c_int,
        )
    };

    if written <= 0 {
        let error = io::Error::last_os_error();

        return match error.raw_os_error() {
            Some(libc::ESRCH | libc::EPERM) => Ok(None),

            _ => Err(Error::Os {
                action: "ask the OS when a process began",
                source: error,
            }),
        };
    }

    if written < size as libc::c_int {
        return Err(Error::Os {
            action: "ask the OS when a process began",
            source: io::Error::new(
                io::ErrorKind::InvalidData,
                format!("the kernel answered with {written} bytes of a {size}-byte struct"),
            ),
        });
    }

    // A process that has ended and not been reaped is bookkeeping, not a service: the caller's
    // question is always "is what I recorded still there".
    if info.pbi_status == libc::SZOMB {
        return Ok(None);
    }

    // Microseconds, so two processes started in the same second are still told apart. Both halves
    // are `u64` in the struct and are seconds since the epoch and microseconds within one, neither
    // of which comes near the top of an `i64`.
    Ok(Some(
        info.pbi_start_tvsec as i64 * 1_000_000 + info.pbi_start_tvusec as i64,
    ))
}

/// An all-zero `proc_bsdinfo`, for the kernel to fill in.
///
/// The struct is `#[repr(C)]` and every field is an integer or an array of them, so all zeroes is a
/// valid value — and every one of them is about to be overwritten by a call that is given this
/// struct's own size.
#[expect(
    unsafe_code,
    reason = "zeroed is sound for a repr(C) struct of integers, and the alternative is naming \
              thirty fields whose values the kernel is about to supply"
)]
fn zeroed_because_every_field_is_a_number() -> libc::proc_bsdinfo {
    unsafe { std::mem::zeroed() }
}

/// Start a supervised child in a session of its own.
///
/// A separate function from `detach`'s use of the same call even though the body is identical: they
/// are two different requests that happen to have one answer on this system, and on the other two
/// they do not. Collapsing them would make the next person read Linux's version to find out what
/// macOS is missing.
pub(crate) fn arrange(command: &mut Command) {
    crate::unix::process::new_session(command);
}
