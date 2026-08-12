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

use std::process::Command;

// The rest is POSIX and identical on both systems, so this module adds to `unix/` rather than
// replacing it. `Group` is re-exported by name, which is what carries its `adopt` and `terminate`
// along with it.
pub(crate) use crate::unix::process::{
    CAN_ASK_TO_STOP, Detaching, Group, INHERITED_ENV, detach, group, hide_stdio,
};

/// Start a supervised child in a session of its own.
///
/// A separate function from `detach`'s use of the same call even though the body is identical: they
/// are two different requests that happen to have one answer on this system, and on the other two
/// they do not. Collapsing them would make the next person read Linux's version to find out what
/// macOS is missing.
pub(crate) fn arrange(command: &mut Command) {
    crate::unix::process::new_session(command);
}
