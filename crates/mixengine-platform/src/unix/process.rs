//! `setsid` between the fork and the exec, for both kinds of child.
//!
//! Surviving the parent needs nothing on Unix — an orphan is reparented to init and carries on. What
//! does need arranging is the *session*: a child inherits its parent's process group, and a terminal
//! sends `SIGINT` to the whole foreground group, so a daemon that stayed in it would die the next
//! time somebody pressed Ctrl-C in the window it was started from. `setsid` puts it in a new session
//! with no controlling terminal at all, which is the difference between a background process and a
//! detached one.
//!
//! # The same call, for the opposite reason
//!
//! A *supervised* child gets `setsid` too, and it is worth being clear that this buys something
//! narrower than the Windows counterpart. `setsid` makes the child a session and process-group
//! leader with `pgid == pid`, and its own children inherit that group — so one `kill` to `-pgid`
//! reaches a php-fpm master and every worker it forked, which is what stopping a service has to
//! mean. What it does **not** do is notice that the daemon has died: a session is a grouping, not a
//! lifetime, and a killed daemon leaves the whole group running. Linux answers that with
//! `PR_SET_PDEATHSIG` in `linux/process.rs`, which is why arranging a supervised spawn is the one
//! thing in this file the two systems do not share; macOS has no answer and says so.
//! `.claude/decisions/0007-supervised-child-owns-a-process-group.md` is where the three-way
//! difference is recorded.

use std::io;
use std::os::unix::process::CommandExt as _;
use std::process::{Child, Command};

use crate::{Error, Result};

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
/// The whole of it is [`new_session`], which the supervised path also uses — see there for why the
/// call has to happen between the fork and the exec, and what may be done in that window. Nothing is
/// left over on this side, hence the empty [`Detaching`].
pub(crate) fn detach(command: &mut Command) -> Detaching {
    new_session(command);

    Detaching
}

/// Arrange for the child to lead a session, and so a process group, of its own.
///
/// Shared by the detached and the supervised path because it is the same syscall for two different
/// purposes: the first wants the child out of the terminal's foreground group, the second wants a
/// group it can address as a whole. Registered rather than called — `pre_exec` runs in the child
/// after `fork` and before `exec`, which is the only moment either is possible.
///
/// That closure is the most constrained code in the crate: between those two calls the child has one
/// thread and whatever locks the parent's other threads happened to be holding, so only
/// async-signal-safe calls are allowed. `setsid` is one, takes no arguments, allocates nothing and
/// can only fail if the caller is already a process group leader — which a freshly forked child
/// never is.
pub(crate) fn new_session(command: &mut Command) {
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
}

/// The process group a supervised child leads.
///
/// Nothing to hold, unlike the Windows counterpart's job object handle: after `setsid` the group's
/// id *is* the child's pid, so the caller already has it and there is no kernel object whose
/// lifetime anyone has to manage. The type exists so the shared `process.rs` has one shape on both
/// systems — and so that the day this needs a cgroup (roadmap task T68, on Linux) there is
/// somewhere for it to go.
#[derive(Debug)]
pub(crate) struct Group;

/// There is nothing to create. Fallible only because Windows's counterpart is.
pub(crate) fn group() -> Result<Group> {
    Ok(Group)
}

/// Whether a group can be *asked* to stop on this system, as opposed to being killed.
///
/// True here, and the mechanism is [`Group::request_stop`]. Windows says `false` and
/// `.claude/decisions/0008-no-signal-stop-on-windows.md` is why.
pub(crate) const CAN_ASK_TO_STOP: bool = true;

/// The variables a child is given even though its spec did not name them.
///
/// The spec's environment is the *whole* environment (`.claude/architecture/process-supervision.md`:
/// "explicit; parent env is NOT inherited wholesale"), and this is the narrow exception: names whose
/// values belong to the session rather than to the service, inherited from this process **only when
/// it has them** and never invented. Nothing here is required by the kernel — a POSIX `exec` into an
/// empty environment is perfectly legal — so the list is short and every entry earns its place by
/// what a service does without it:
///
/// - `PATH`, because a service that shells out (a MariaDB init script, a php-fpm pool running
///   `sendmail`) finds nothing without one.
/// - `HOME`, because a program with no home directory writes its dotfiles into `/` or refuses to
///   start, and `mariadb` reads `~/.my.cnf`.
/// - `TMPDIR`, because the alternative is `/tmp` on a machine whose administrator moved it.
/// - `LANG`, `LC_ALL`, `TZ`, because a log line's timestamps and a database's collation should read
///   the way the rest of the user's machine does.
/// - `USER`, `LOGNAME`, `SHELL`, because a process that asks who it is running as should get the
///   same answer the daemon would.
///
/// A spec that names any of these overrides it, which is the point of applying them first.
pub(crate) const INHERITED_ENV: &[&str] = &[
    "PATH", "HOME", "TMPDIR", "LANG", "LC_ALL", "TZ", "USER", "LOGNAME", "SHELL",
];

impl Group {
    /// `SIGTERM` to the whole group — the polite half of stopping one.
    ///
    /// The same negated pid as [`terminate`](Self::terminate), and the same reasoning about `ESRCH`:
    /// a group that has already gone is what the caller wanted. What differs is only the signal, and
    /// so what the members are allowed to do about it — flush a buffer, close a socket, remove a
    /// pidfile. The grace period that follows, and the [`terminate`](Self::terminate) that ends it,
    /// belong to the supervisor.
    ///
    /// **Sent to the group and not to the leader**, because the workers are the processes holding
    /// the port and the data directory: a php-fpm master that has already crashed cannot pass a
    /// signal on to the pool it forked.
    pub(crate) fn request_stop(&self, pid: u32) -> Result<()> {
        self.signal(pid, libc::SIGTERM, "ask a supervised process group to stop")
    }

    /// Nothing to do: the child joined its group by calling `setsid` itself, before it was even the
    /// program the caller asked for.
    ///
    /// The Windows counterpart has real work here and a race to go with it. This is the half of the
    /// two-step that Unix gets for free.
    #[expect(
        clippy::unnecessary_wraps,
        reason = "the signature is Windows's, where assigning a process to a job really can fail"
    )]
    pub(crate) fn adopt(&self, _child: &Child) -> Result<()> {
        Ok(())
    }

    /// `SIGKILL` to the whole group.
    ///
    /// The negated pid is the entire mechanism: `kill(-pgid, …)` addresses every process in the
    /// group, which after `setsid` is the child and everything it started — the php-fpm workers, the
    /// `mariadbd` that a wrapper script `exec`ed into, all of it.
    ///
    /// `SIGKILL` rather than `SIGTERM` because this is the ungraceful stop by construction; the
    /// grace period a `StopBehaviour` asks for is the supervisor's to run (roadmap task T15), and it
    /// ends here when it runs out.
    ///
    /// A group that has already gone is success, not failure. `ESRCH` means there was nothing left
    /// to kill, which is what the caller wanted; treating it as an error would make a service that
    /// exited a millisecond before the deadline look like one that could not be stopped.
    pub(crate) fn terminate(&self, pid: u32) -> Result<()> {
        self.signal(pid, libc::SIGKILL, "stop a supervised process group")
    }

    /// Send one signal to the whole group, and forgive a group that is not there.
    ///
    /// The negated pid is the entire mechanism and is shared by both callers, so it is written once:
    /// getting it wrong in one of them would signal a single process while the code around it talked
    /// about a group.
    fn signal(&self, pid: u32, signal: libc::c_int, action: &'static str) -> Result<()> {
        #[expect(
            unsafe_code,
            reason = "kill takes two integers by value, touches no memory of ours, and is the only \
                      way to signal a process group"
        )]
        let signalled = unsafe { libc::kill(-(pid as libc::pid_t), signal) };

        if signalled == 0 {
            return Ok(());
        }

        let error = io::Error::last_os_error();

        if error.raw_os_error() == Some(libc::ESRCH) {
            return Ok(());
        }

        Err(Error::Os {
            action,
            source: error,
        })
    }
}

/// Nothing to undo, but the drop still happens.
///
/// The Windows counterpart restores an inheritance flag here, and `process.rs` drops the guard at the
/// one point where that has to happen. Implementing `Drop` on this side too is the rest of the one
/// shape this type exists for: without it the shared `drop(detaching)` is a no-op the compiler can
/// see through, and `clippy::drop_non_drop` says so on this system alone.
impl Drop for Detaching {
    fn drop(&mut self) {}
}

/// Nothing to arrange: a descriptor is not handed to a child here unless somebody asks for it.
///
/// The Windows counterpart of this exists because inheritance there is a property of the handle and
/// survives being inherited, so a process passes on what it was given without meaning to. On Unix
/// only the three standard descriptors cross an `exec` by default — everything the standard library
/// opens is `CLOEXEC` — and a spawn that redirects those three has already said everything there is
/// to say.
pub(crate) fn hide_stdio() -> Detaching {
    Detaching
}
