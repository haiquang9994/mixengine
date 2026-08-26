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

use std::fs::File;
use std::io;
use std::os::unix::process::CommandExt as _;
use std::path::Path;
use std::process::{Child, Command, ExitStatus};

use crate::sys::process as sys;
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

/// What a program run for its exit status is started with: nothing.
///
/// The Windows counterpart has real work here — a console this child must not be given — and this
/// system has none of it. **No `setsid`, unlike both other spawns**, and that is a decision rather
/// than an omission: a session would make the one-shot unreachable by a `kill` to the pid the
/// caller holds, which is exactly what its deadline has to be able to reach. A probe or a shutdown
/// command lives for milliseconds and is not a service; it inherits this process's group and is
/// killed as the one process it is.
///
/// The empty body is the answer, and it takes an argument only so both systems present one shape.
pub(crate) fn arrange_one_shot(_command: &mut Command) {}

/// The process group a supervised child leads, and whatever this system caps it with.
///
/// The *group* itself needs nothing held, unlike the Windows counterpart's job object handle: after
/// `setsid` the group's id **is** the child's pid, so the caller already has it and there is no
/// kernel object whose lifetime anyone has to manage.
///
/// **The cap is the part that differs between the two Unixes, and it arrived with roadmap task
/// T68.** Linux has a cgroup to create, write and remove; macOS has nothing at all. So the field's
/// *type* comes from the OS module rather than being a field behind a `#[cfg(target_os = "linux")]`
/// in this shared file — the same reason `PR_SET_PDEATHSIG` lives in `linux/process.rs` and not
/// here. What this file knows is that there is an attachment and that it has three methods.
#[derive(Debug)]
pub(crate) struct Group {
    /// This system's mechanism for capping the group, already prepared.
    pub(crate) attachment: sys::Attachment,
}

/// Prepare whatever this system caps a service with, before there is a service to cap.
///
/// Fallible only because Windows's counterpart is: neither Unix has anything here that fails, and a
/// Linux machine that will lend no cgroup answers with an attachment that caps nothing rather than
/// with an error. The T68 design, D6.
pub(crate) fn group() -> Result<Group> {
    Ok(Group {
        attachment: sys::Attachment::prepare(),
    })
}

/// Whether a group can be *asked* to stop on this system, as opposed to being killed.
///
/// True here, and the mechanism is [`Group::request_stop`]. Windows says `false` and
/// `.claude/decisions/0008-no-signal-stop-on-windows.md` is why.
pub(crate) const CAN_ASK_TO_STOP: bool = true;

/// Whether a process can be signalled here. See [`crate::process::CAN_SIGNAL`].
pub(crate) const CAN_SIGNAL: bool = true;

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

    /// Write this service's ceilings into the cgroup it will be in.
    ///
    /// **Called before the child exists**, from [`crate::process::spawn_supervised`], because the
    /// cgroup has to be there for the child's `pre_exec` to write itself into — and again for every
    /// later change, with the same code, so a cap applied at start and one applied to a running
    /// service cannot drift apart. Rewriting `cpu.max` under live processes is exactly what cgroup
    /// v2 is for, so the second call needs nothing the first did not do.
    ///
    /// Infallible on this system: a machine that lends no cgroup has no attachment, and a controller
    /// it was not delegated is a file that will not open. Both are reported once, to a person,
    /// through [`ResourceControl`](crate::ResourceControl) — failing a start over either would turn a
    /// machine that cannot cap a service into one that cannot run one. The T68 design, D6.
    #[expect(
        clippy::unnecessary_wraps,
        reason = "the signature is Windows's, where writing a job object really can fail"
    )]
    pub(crate) fn set_limits(&self, limits: &crate::process::Limits) -> Result<()> {
        self.attachment.write_caps(limits);

        Ok(())
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
    pub(crate) fn adopt(&self, _child: &RawChild) -> Result<()> {
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
    /// `kind` rather than `signal`, which would shadow the function this calls.
    fn signal(&self, pid: u32, kind: libc::c_int, action: &'static str) -> Result<()> {
        signal(-(pid as libc::pid_t), kind, action).map(drop)
    }

    /// Send one signal to the process that *leads* the group, and forgive one that is not there.
    ///
    /// The pid is **not** negated, which is the entire difference from [`signal`](Self::signal): a
    /// positive target is one process. See [`crate::process::Supervised::signal`] for why a reload
    /// goes there rather than to everybody.
    ///
    /// `which` rather than `signal`, for the reason [`signal`](Self::signal) takes a `kind`.
    pub(crate) fn signal_leader(&self, pid: u32, which: crate::process::Signal) -> Result<()> {
        let (kind, action) = match which {
            crate::process::Signal::Hup => (libc::SIGHUP, "hang up a supervised process"),
            crate::process::Signal::Usr1 => (libc::SIGUSR1, "signal a supervised process"),
            crate::process::Signal::Usr2 => (libc::SIGUSR2, "signal a supervised process"),
        };

        signal(pid as libc::pid_t, kind, action).map(drop)
    }
}

/// `SIGKILL` to the group led by a process this one did not start, or to the process itself if it
/// leads none.
///
/// **The group first, and for the reason [`Group::terminate`] aims there**: a supervised child called
/// `setsid` before it became the program, so its pgid is its pid for as long as it lives, and a
/// survivor of a killed daemon (roadmap task T18) is by definition such a child — addressing the
/// group is addressing the workers it forked as well.
///
/// **The fallback is not decoration.** `kill(-pid, …)` fails with `ESRCH` when no *group* has that
/// id, which is indistinguishable, from here, from a group that has gone — and a process that leads
/// no group is still a process holding the port. Forgiving the first case and stopping there would
/// leave exactly the orphan adoption exists to clear, and a caller cannot tell the difference either,
/// because a successful `kill` is all it is told. So the group is tried, and the process alone is
/// tried when there was no group to reach.
///
/// Signalling a bare pid is sound *here* and nowhere else in this module: [`Adopted`] re-reads the
/// identity immediately before it calls this, so the number has been confirmed to be the process
/// that was recorded — see the shared `process.rs`.
///
/// [`Adopted`]: crate::process::Adopted
pub(crate) fn stop_foreign(pid: u32) -> Result<()> {
    reach(
        pid,
        libc::SIGKILL,
        "stop a process that outlived the daemon supervising it",
    )
}

/// `SIGTERM` to that group, or to that process — the polite half, which this system does have.
pub(crate) fn ask_foreign_to_stop(pid: u32) -> Result<()> {
    reach(
        pid,
        libc::SIGTERM,
        "ask a process that outlived the daemon supervising it to stop",
    )
}

/// Signal the group `pid` leads, falling back to the process itself when it leads none.
fn reach(pid: u32, kind: libc::c_int, action: &'static str) -> Result<()> {
    if signal(-(pid as libc::pid_t), kind, action)? {
        return Ok(());
    }

    signal(pid as libc::pid_t, kind, action).map(drop)
}

/// Send one signal, and say whether there was anything there to receive it.
///
/// A negative `target` is a process group and a positive one is a process; the negation is the entire
/// mechanism and is written in the two places that decide which they mean, rather than here.
///
/// `ESRCH` is `Ok(false)` rather than an error: there was nothing left to signal, which is what a
/// caller stopping something wanted — treating it as a failure would make a service that exited a
/// millisecond before the deadline look like one that could not be stopped. It is *reported* rather
/// than swallowed because for one caller it is not the end of the question.
fn signal(target: libc::pid_t, kind: libc::c_int, action: &'static str) -> Result<bool> {
    #[expect(
        unsafe_code,
        reason = "kill takes two integers by value, touches no memory of ours, and is the only \
                  way to signal a process group"
    )]
    let signalled = unsafe { libc::kill(target, kind) };

    if signalled == 0 {
        return Ok(true);
    }

    let error = io::Error::last_os_error();

    if error.raw_os_error() == Some(libc::ESRCH) {
        return Ok(false);
    }

    Err(Error::Os {
        action,
        source: error,
    })
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

/// Replace this process with `command`, keeping everything the caller was given.
///
/// `exec` and nothing else, which is the whole of the Unix side: the pid, the open descriptors, the
/// controlling terminal, the process group and every signal disposition survive, because there is
/// no new process for them to have to be copied to. A `SIGINT` from the terminal reaches the
/// program for the same reason — it is in the foreground process group already, having never left
/// it.
///
/// **Deliberately not `new_session`**, unlike both other spawns in this module. A shim that put the
/// program in a session of its own would take it out of the terminal's foreground group, so Ctrl-C
/// would reach nothing and a program reading from the terminal would be stopped with `SIGTTIN`.
///
/// The return type is Windows's: on this system the only way out of here is the error, and
/// `CommandExt::exec` is typed to say so.
pub(crate) fn hand_over(mut command: Command, program: &Path) -> Result<i32> {
    let source = command.exec();

    Err(Error::Io {
        action: "run",
        path: program.to_path_buf(),
        source,
    })
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

/// A supervised child, as this system makes one: the standard library's, unchanged.
///
/// The type exists so `process.rs` has one shape on both systems — see the Windows counterpart,
/// which is not a [`Child`] at all and cannot be, because `CreateProcessAsUserW` hands back a raw
/// handle and no [`Child`] can be built from one.
#[derive(Debug)]
pub(crate) struct RawChild(Child);

impl RawChild {
    /// The child's process id, which after `setsid` is also its process group id.
    pub(crate) fn id(&self) -> u32 {
        self.0.id()
    }

    /// Whether it has ended, without waiting for it to.
    pub(crate) fn try_wait(&mut self) -> io::Result<Option<ExitStatus>> {
        self.0.try_wait()
    }

    /// Wait for it to end, however it ends.
    pub(crate) fn wait(&mut self) -> io::Result<ExitStatus> {
        self.0.wait()
    }

    /// `SIGKILL` to this process alone — the group is the caller's business.
    pub(crate) fn kill(&mut self) -> io::Result<()> {
        self.0.kill()
    }

    /// Its standard output, taken once.
    pub(crate) fn take_stdout(&mut self) -> Option<crate::process::OutputPipe> {
        self.0.stdout.take().map(|pipe| {
            crate::process::OutputPipe::new(File::from(std::os::fd::OwnedFd::from(pipe)))
        })
    }

    /// Its standard error, taken once.
    pub(crate) fn take_stderr(&mut self) -> Option<crate::process::OutputPipe> {
        self.0.stderr.take().map(|pipe| {
            crate::process::OutputPipe::new(File::from(std::os::fd::OwnedFd::from(pipe)))
        })
    }
}

/// Start a supervised child: both streams piped, standard input the null device.
///
/// The environment arrives already composed — see
/// [`whole_environment`](crate::process::whole_environment) — because the Windows counterpart has no
/// [`Command`] to put it on and the rule may only be stated once.
///
/// # Errors
///
/// [`Error::Io`] naming the program when it cannot be started at all.
pub(crate) fn spawn_child(
    program: &Path,
    args: &[std::ffi::OsString],
    directory: &Path,
    env: &std::collections::BTreeMap<String, String>,
    // The child writes itself into this group's cgroup between `fork` and `exec` — see
    // [`join_cgroup`]. Nothing on macOS, where there is no cgroup to write into.
    group: &Group,
) -> Result<RawChild> {
    let mut command = Command::new(program);
    command
        .args(args)
        .current_dir(directory)
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped())
        .env_clear()
        .envs(crate::process::whole_environment(env));

    crate::sys::process::arrange(&mut command, group);

    // Held for the same reason `spawn_detached` holds it, and it is not only the detached child's
    // hazard: an inheritable handle this process was given is passed on to *every* child it starts,
    // so a service would keep a pipe open that somebody is reading to end-of-file. A no-op on this
    // system, and called here so that both systems hide the same thing at the same moment.
    let hiding = hide_stdio();
    let spawned = command.spawn();
    drop(hiding);

    spawned.map(RawChild).map_err(|source| Error::Io {
        action: "start",
        path: program.to_path_buf(),
        source,
    })
}

/// Put the child into `cgroup.procs` **itself**, between the fork and the exec.
///
/// **The child does it, and that is what makes the cap sound.** Writing the child's pid from the
/// parent after the spawn leaves a window in which the child may already have forked — and for
/// php-fpm, whose first act is to fork a pool, that window is not theoretical: the workers would land
/// outside the cap while the master sat inside it, and the service would look capped while being
/// uncapped. A process that joins before `exec` cannot have children yet, so the window is not
/// narrowed but removed.
///
/// **`0` rather than a pid**, which is what keeps this inside `pre_exec`'s contract: the kernel reads
/// `0` in `cgroup.procs` as "the process doing the writing", so there is no number to format, nothing
/// to allocate, and no call here that is not async-signal-safe. The descriptor was opened in the
/// parent, for the same reason — opening a file by path in that window is a great deal more than a
/// `write`.
///
/// A failed write is **not** a failed spawn. A cgroup this machine would not lend is reported once,
/// to a person, and the service runs uncapped — the T68 design, D6. Returning an error here would
/// refuse the start instead.
#[cfg(target_os = "linux")]
pub(crate) fn join_cgroup(command: &mut Command, fd: std::os::fd::RawFd) {
    #[expect(
        unsafe_code,
        reason = "the closure calls one async-signal-safe libc function on a descriptor the parent \
                  owns for the length of the spawn, and touches no memory of its own"
    )]
    unsafe {
        command.pre_exec(move || {
            libc::write(fd, c"0\n".as_ptr().cast(), 2);

            Ok(())
        });
    }
}

/// Ask the scheduler to prefer everything else, for the whole group.
///
/// `PRIO_PGRP` rather than `PRIO_PROCESS`, and the negated-pid reasoning of [`Group::terminate`]
/// applies unchanged: after `setsid` the child's pgid is its pid, so one call reaches the php-fpm
/// master *and* every worker it forked. A priority that covered the master alone would be a priority
/// on the one process in the service that does no work.
///
/// **Lowering never needs privilege**, and `Normal` → `Background` is the only direction this goes;
/// putting one back is a return to 0, which the same account that lowered it may always do. So the
/// failure is logged by nobody and dropped here: there is no configuration in which this is the
/// reason a service should not run.
pub(crate) fn set_priority(pid: u32, priority: crate::process::Priority) {
    let nice = match priority {
        crate::process::Priority::Normal => 0,
        crate::process::Priority::Background => 10,
    };

    #[expect(
        unsafe_code,
        reason = "setpriority takes three integers, borrows nothing, and cannot reach memory"
    )]
    unsafe {
        libc::setpriority(libc::PRIO_PGRP, pid, nice);
    }
}
