//! Starting a process, and deciding whether it may outlive the one that started it.
//!
//! Two opposite requests live here, and the pair is the whole of the module:
//!
//! - [`spawn_detached`] starts a process this one is letting go of. `mixengined --detach` and a
//!   client's autostart (roadmap task T10) are made of it.
//! - [`spawn_supervised`] starts a process this one owns and intends to outlive. Every managed
//!   service is made of it (roadmap task T13).
//!
//! A third relationship arrives with crash recovery and starts no process at all: [`Adopted`] is one
//! that survived the daemon which started it, taken over by the daemon running now on the strength
//! of its pid *and* its [`StartTime`] (roadmap task T18). It is the weakest of the three — liveness
//! and a stop, and nothing else — and its documentation says where that is paid for.
//!
//! **The difference is stated in the destructors, because that is where a reader will meet it**:
//! dropping a [`Detached`] deliberately does not stop the child, and dropping a [`Supervised`]
//! deliberately does.
//!
//! Detaching is the part every OS does differently. Surviving the parent is the easy half and comes
//! for free on both systems; what has to be arranged is that the child stops being *attached* to
//! whoever started it:
//!
//! - it must not be reached by a Ctrl-C meant for the terminal the parent was typed into,
//! - it must not write into that terminal hours later, and
//! - it must not keep the terminal's console alive on Windows.
//!
//! The stdio redirection is the one piece that is genuinely shared, so it is done here: all three
//! streams go to the null device. A daemon that has detached says everything it has to say in
//! `logs/daemon.log`, and inheriting the parent's handles is how a background process ends up
//! printing into a shell prompt somebody is using.
//!
//! Supervising is the part every OS does differently *and* does with different force. A supervised
//! child leads a process group of its own — a Job Object on Windows, a session on Unix — so that
//! stopping it means stopping everything it started, and so that a daemon which goes away takes it
//! along. How completely that last part is true depends on the system and is set out in
//! `.claude/decisions/0007-supervised-child-owns-a-process-group.md`: a kernel guarantee on Windows,
//! a guard that covers the immediate child on Linux, and nothing at all on macOS, where crash
//! recovery at the next boot (roadmap task T18) is what closes the gap.

use std::collections::BTreeMap;
use std::ffi::OsString;
use std::fmt;
use std::path::Path;
use std::process::{Child, ChildStderr, ChildStdout, Command, Stdio};

use crate::sys::process as sys;
use crate::{Error, Result};

/// Whether a supervised group can be *asked* to stop on this system, rather than only killed.
///
/// True on Unix, where [`Supervised::ask_to_stop`] sends `SIGTERM` to the group. **False on
/// Windows**, where there is no signal a daemon can send to a process it gave no console to — see
/// `.claude/decisions/0008-no-signal-stop-on-windows.md`.
///
/// A supervisor reads this before it starts a grace period: waiting out five seconds for a request
/// that was never sent is not patience, it is five seconds added to every stop. On a system that
/// says `false`, a service that has to shut down cleanly does it through a command of its own
/// (`StopBehaviour::Command`) and everything else is killed at once.
pub const CAN_ASK_TO_STOP: bool = sys::CAN_ASK_TO_STOP;

/// This process's standard handles, kept from every child started while it is held.
///
/// Returned by [`hide_stdio_from_children`]; puts them back when it drops.
#[derive(Debug)]
#[must_use = "the handles are passed on again the moment this is dropped"]
pub struct HiddenStdio(
    #[allow(
        dead_code,
        reason = "held for its Drop, which only Windows gives a body"
    )]
    sys::Detaching,
);

/// Keep this process's standard handles from reaching the children it starts, until the returned
/// guard is dropped.
///
/// **A caller that starts a detached process itself does not need this** — [`spawn_detached`] does
/// it already. What needs it is the caller one step further back: a client that starts
/// `mixengined --detach` and reads it to end-of-file. Inheritance on Windows is transitive, because
/// *inheritable* is a property of the handle and survives being inherited — so the client's stdout
/// reaches the middle process, and the middle process passes it on to the daemon before it has any
/// say in the matter. `spawn_detached` clears the flag on the handles the middle process owns, which
/// is one copy too late for the client's. The end-of-file the client is waiting for then arrives when
/// the *daemon* exits, days later.
///
/// Every process in a chain like that has to decline to pass its own handles on, and this is how one
/// says it: hold the guard across the spawn, drop it straight after. Held no longer than that, so a
/// program that goes on to start ordinary children — a `mix` running a hook, an editor — hands them
/// its stdio exactly as it would have.
///
/// Nothing to do on Unix, where only the three standard descriptors cross an `exec` and everything
/// else the standard library opens is `CLOEXEC`. The guard still exists there, so a caller has one
/// shape of code on all three systems.
pub fn hide_stdio_from_children() -> HiddenStdio {
    HiddenStdio(sys::hide_stdio())
}

/// A process that has been started and let go.
///
/// Dropping it does **not** stop the child — neither system kills a process when its parent lets go
/// of the handle, which is the entire point. What the handle is still good for is the question the
/// caller asks while it waits for the child to come up: is it still there?
///
/// **A caller that goes on running has to reap it.** [`Child`] does not wait on drop, so on Unix the
/// process started here stays this process's child and becomes a zombie the moment it exits, for as
/// long as this process lives. `mixengined --detach` exits straight after the spawn and never meets
/// that; `mix` and the GUI autostarting a daemon (roadmap task T10) will, and have to keep the
/// handle and go on asking [`exited`](Self::exited) rather than dropping it once the daemon answers.
#[derive(Debug)]
pub struct Detached {
    child: Child,
}

/// How a detached process ended.
///
/// Two questions in one answer, because the caller needs both and they come from the same syscall.
/// [`is_success`](Self::is_success) is what a decision is made on — a `--detach` whose child exited
/// *successfully* because another daemon already held the home has not failed and must keep waiting
/// for that daemon, while one whose child failed has nothing left to wait for. The [`Display`](fmt::Display)
/// form is for the message to a person, and is rendered rather than reduced to a code because the
/// two systems disagree about what one is: a Unix child that died on a signal has no exit code at
/// all, and reporting `None` for it would lose the only interesting thing about it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Exit {
    success: bool,
    code: Option<i32>,
    described: String,
}

impl Detached {
    /// The child's process id, for a message to a person and for the lock file to be checked
    /// against.
    #[must_use]
    pub fn pid(&self) -> u32 {
        self.child.id()
    }

    /// Whether it has already exited, and how it put it.
    ///
    /// Never waits. A caller polling for a daemon to come up needs this to tell "not ready yet"
    /// from "gone", which are otherwise the same silence — and the second one is the answer that
    /// must not be waited out.
    ///
    /// # Errors
    ///
    /// [`Error::Os`] if the OS will not say. Not the same as "still running",
    /// which is `Ok(None)`.
    pub fn exited(&mut self) -> Result<Option<Exit>> {
        exited(&mut self.child)
    }
}

/// A process this one owns, and does not intend to be survived by.
///
/// The mirror image of [`Detached`], and the contrast is the point of both types existing. A
/// supervised child leads a process group of its own, so it is stopped as a whole — the php-fpm
/// master and every worker it forked, the shell script and the `mariadbd` it started — rather than
/// one pid at a time.
///
/// **Dropping this stops the child.** That is the opposite of [`Detached`] and is deliberate: this
/// value *is* the group's ownership, so a supervisor handle going out of scope while its processes
/// kept running would be an orphan produced by the one module that exists to prevent them. A child
/// that has already exited is left alone, so the ordinary path — a service that stopped by itself
/// and was waited for — kills nothing.
///
/// # What survives this process being killed
///
/// Nothing on Windows: the job object takes the whole group down when the last handle to it closes,
/// which is a kernel guarantee and needs no code of ours to run. The immediate child on Linux, via
/// `PR_SET_PDEATHSIG`; anything *it* started is reparented and carries on. Everything on macOS,
/// which has neither mechanism. Crash recovery at the next boot (roadmap task T18) is what covers
/// the difference, and
/// `.claude/decisions/0007-supervised-child-owns-a-process-group.md` is where it is written down
/// rather than rounded up.
#[derive(Debug)]
pub struct Supervised {
    child: Child,

    /// The group the child leads. Held for its `Drop` on Windows, where closing the job handle is
    /// what kills the group.
    group: sys::Group,

    /// Whether the group has already been killed through this handle.
    ///
    /// What it prevents is a second `kill(-pgid, …)` after the leader has been reaped, which is the
    /// one moment a pgid stops being reliably ours: a process group exists while any member does,
    /// including a zombie, so an *unreaped* leader keeps the number reserved and a reaped one does
    /// not. Terminating before waiting is therefore always sound, and terminating a second time
    /// afterwards is the residual race ADR 0007 accepts — worth not running into twice for free.
    stopped: bool,
}

impl Supervised {
    /// The child's process id, which on Unix is also its process group id.
    ///
    /// Recorded together with the process start time by whoever persists it: a pid on its own is not
    /// an identity, because the number is reused. That pairing is what makes adoption after a daemon
    /// restart sound (roadmap task T18) — see [`started_at`](Self::started_at) for the other half.
    #[must_use]
    pub fn pid(&self) -> u32 {
        self.child.id()
    }

    /// When this child began, for the row that has just recorded its pid.
    ///
    /// **Asked while the handle is held**, which is what makes the answer this child's rather than
    /// somebody else's: an unreaped child keeps its pid reserved on Unix, and on Windows this
    /// process holds a handle to it, so the number cannot have been given away between the spawn and
    /// this call.
    ///
    /// [`None`] for a child that has already ended — a service that failed in its first
    /// milliseconds — and that is the honest answer rather than a defect: what a null
    /// `pid_start_time` says is "this row cannot be identified later", and a row naming a process
    /// that is already gone is exactly one that must never be adopted.
    ///
    /// # Errors
    ///
    /// [`Error::Os`], or [`Error::Io`] on Linux, when the OS has the answer and would not give it.
    /// Not the same as "it has ended", which is `Ok(None)`.
    pub fn started_at(&self) -> Result<Option<StartTime>> {
        started_at(self.child.id())
    }

    /// Whether it has already exited, and how it put it. Never waits.
    ///
    /// Says nothing about the rest of the group: a service whose master process exited while a
    /// worker it forked is still running answers `Some` here. That is the honest answer for a
    /// restart policy, which is about the process it started, and it is why stopping goes through
    /// [`stop`](Self::stop) rather than through this.
    ///
    /// # Errors
    ///
    /// [`Error::Os`] if the OS will not say. Not the same as "still running", which is `Ok(None)`.
    pub fn exited(&mut self) -> Result<Option<Exit>> {
        exited(&mut self.child)
    }

    /// The child's standard output, taken once.
    ///
    /// Piped by [`spawn_supervised`] and **owed a reader**: a pipe holds tens of kilobytes, after
    /// which the service blocks on its next line and looks like it has hung. Log capture (roadmap
    /// task T16) is that reader; until it exists, a caller that does not take these streams should
    /// only supervise a process that says nothing.
    #[must_use]
    pub fn take_stdout(&mut self) -> Option<ChildStdout> {
        self.child.stdout.take()
    }

    /// The child's standard error, taken once. See [`take_stdout`](Self::take_stdout).
    #[must_use]
    pub fn take_stderr(&mut self) -> Option<ChildStderr> {
        self.child.stderr.take()
    }

    /// Ask the whole group to stop and give it a chance to tidy up.
    ///
    /// `SIGTERM` to `-pgid` on Unix. Returns as soon as the request is sent — how long the group is
    /// then given, and the [`stop`](Self::stop) that ends the wait, belong to the supervisor, which
    /// is the only thing that knows what the service's `StopBehaviour` asked for.
    ///
    /// **Check [`CAN_ASK_TO_STOP`] first.** On Windows there is no such request and this says so
    /// rather than succeeding quietly, because a grace period spent waiting for a message nobody
    /// sent is time added to every stop for nothing.
    ///
    /// Sent to the *group*, not to the leader, which is the whole point: the processes holding the
    /// port and the data directory are the workers, and a master that has already crashed cannot
    /// pass a signal on to them.
    ///
    /// # Errors
    ///
    /// [`Error::UnsupportedPlatform`] where the system has no way to ask, and [`Error::Os`] if it
    /// has one and refused. A group that has already gone is not a failure.
    pub fn ask_to_stop(&self) -> Result<()> {
        self.group.request_stop(self.child.id())
    }

    /// Kill the whole group, at once, without a chance to tidy up.
    ///
    /// `TerminateJobObject` on Windows, `SIGKILL` to `-pgid` on Unix. This is the ungraceful stop by
    /// construction: [`ask_to_stop`](Self::ask_to_stop) is the polite half, and this is what the
    /// grace period after it ends in.
    ///
    /// Reaps the child afterwards, so the pid is not left to a zombie on Unix — which also means the
    /// call returns only once the process this handle names is really gone. Other members of the
    /// group are not waited for, having never been this process's children.
    ///
    /// **The group is killed even when the leader has already exited**, and that is the correction
    /// T15 owed T13. A php-fpm master that crashed leaves its pool holding the port; a wrapper script
    /// that `exec`ed and died leaves the server it started. Skipping the kill because the process we
    /// named is gone would leave exactly the processes a restart is about to collide with — and
    /// "gone" is also the state a stop is *trying* to reach, so making it a precondition read the
    /// question backwards. What the old guard bought was not signalling a pgid the OS may have given
    /// away since; that window is the residual race
    /// `.claude/decisions/0007-supervised-child-owns-a-process-group.md` already accepts, and this
    /// handle remembers that it has killed so it cannot enter that window twice.
    ///
    /// # Errors
    ///
    /// [`Error::Os`] if the OS refuses to stop the group, or cannot be waited on. A group that had
    /// already gone is not a failure.
    pub fn stop(&mut self) -> Result<Exit> {
        if !self.stopped {
            self.group.terminate(self.child.id())?;
            self.stopped = true;
        }

        self.wait()
    }

    /// Wait for the child to end, however it ends.
    ///
    /// # Errors
    ///
    /// [`Error::Os`] if the OS cannot be waited on for a process this one started.
    pub fn wait(&mut self) -> Result<Exit> {
        self.child.wait().map(describe).map_err(|source| Error::Os {
            action: "wait for the process it is supervising",
            source,
        })
    }
}

impl Drop for Supervised {
    /// Stop what this handle owns, on the way out.
    ///
    /// Failures are dropped because there is nowhere to report them from and nothing left to do
    /// about them. A handle that has already been [`stop`](Self::stop)ped kills nothing a second
    /// time; one whose child merely *exited* still kills the group, because what a destructor is
    /// for is the processes nobody is left holding — see [`stop`](Self::stop).
    fn drop(&mut self) {
        let _ = self.stop();
    }
}

/// Start `program` supervised by this process, with `directory` as its working directory and `env`
/// as — very nearly — its whole environment.
///
/// The child leads a process group of its own and is not meant to outlive this process. Both
/// streams are **piped**, because everything a managed service says has to reach
/// `logs/services/<id>/current.log` (roadmap task T16) — see
/// [`take_stdout`](Supervised::take_stdout) for the obligation that comes with them. `stdin` is the
/// null device: a service that reads from a terminal is one that hangs where nobody can see it.
///
/// `directory` is required rather than inherited for the same reason [`spawn_detached`] requires it
/// — a process's working directory is a reference the OS holds for the process's whole life.
///
/// # The environment is the caller's, not this process's
///
/// The opposite of [`spawn_detached`], which passes its own on. A `ServiceSpec` states its
/// environment in full, so a service behaves the same whether the daemon was started from a shell, a
/// login item or a scheduled task — and so that a variable the *user* exported, or one an installer
/// left behind, cannot change what a managed MariaDB does. `env` arrives already resolved: a
/// credential named by a spec has been fetched from the [`Keyring`](crate::Keyring) by whoever built
/// this map, and this function neither knows nor logs which of these values was one.
///
/// **A short per-OS floor is applied underneath it**, from this process's own environment and only
/// where this process has the variable: `PATH`, `HOME` and the locale on Unix, and on Windows the
/// eight or so names — `SystemRoot` first among them — without which a program cannot even load the
/// system DLLs it was linked against. They are the session's rather than the service's, nothing is
/// invented, and `env` overrides every one of them. `sys::INHERITED_ENV` is the list, per OS, with
/// the reasoning entry by entry.
///
/// # Errors
///
/// [`Error::Os`] if the process group cannot be created or the child cannot be put into it, and
/// [`Error::Io`] naming the program when it cannot be started at all. A child that was started and
/// could not then be adopted is killed **by pid** and waited for rather than returned or left
/// behind: it is a process nothing would own, and the group it was meant to belong to is not the
/// thing that can stop it — on Windows a failed adoption is exactly the case where the job is empty.
pub fn spawn_supervised(
    program: &Path,
    args: &[OsString],
    directory: &Path,
    env: &BTreeMap<String, String>,
) -> Result<Supervised> {
    let group = sys::group()?;

    let mut command = Command::new(program);
    command
        .args(args)
        .current_dir(directory)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());

    // Cleared first, so what follows is the whole of it. The floor goes on before the spec's own
    // entries, which is what makes a spec able to override one — on Windows that comparison is
    // case-insensitive inside `Command`, so a spec naming `Path` replaces the inherited `PATH`
    // rather than adding a second variable the child would see only one of.
    command.env_clear();
    for name in sys::INHERITED_ENV {
        if let Some(value) = std::env::var_os(name) {
            command.env(name, value);
        }
    }
    command.envs(env);

    sys::arrange(&mut command);

    // Held for the same reason `spawn_detached` holds it, and it is not only the detached child's
    // hazard: an inheritable handle this process was given is passed on to *every* child it starts,
    // so a service would keep a pipe open that somebody is reading to end-of-file.
    let hiding = hide_stdio_from_children();
    let spawned = command.spawn();
    drop(hiding);

    let child = spawned.map_err(|source| Error::Io {
        action: "start",
        path: program.to_path_buf(),
        source,
    })?;

    let mut supervised = Supervised {
        child,
        group,
        stopped: false,
    };

    // Adoption is the step that can fail on Windows, and the process it fails about is already
    // running — while being, by definition of the failure, *outside* the group. So this is the one
    // path the destructor cannot be left to: it would ask an empty job object to terminate, kill
    // nothing, and then wait for a child nothing had stopped, which for a service is forever. The
    // child is killed by pid instead, which is sound here for the reason it is not sound anywhere
    // else in this module — the pid was returned by a spawn a few lines above and has not been
    // reaped, so it is still this one process and cannot have been given away.
    //
    // Waited for rather than only signalled, so that what leaves this function is an error and not
    // also a zombie, and so the destructor below finds a child that has already ended and does
    // nothing at all.
    if let Err(error) = supervised.group.adopt(&supervised.child) {
        let _ = supervised.child.kill();
        let _ = supervised.child.wait();

        return Err(error);
    }

    Ok(supervised)
}

/// When a process began, in whatever this operating system counts such moments in.
///
/// **Opaque, and only ever compared for equality.** A `FILETIME` on Windows, microseconds since the
/// epoch on macOS, clock ticks since boot on Linux: three different units answering one question,
/// which is *is the process bearing this pid still the one I recorded*. Nothing may render it, do
/// arithmetic on it, or compare two of them for order — a value from one machine means nothing on
/// another, and on Linux a value from one boot means nothing in the next.
///
/// It is stored, so it crosses a process boundary as an integer and comes back as one: `services.pid_start_time`
/// holds exactly [`stored`](Self::stored), which `.claude/architecture/data-model.md` describes as a
/// column that "exists to be compared, never read".
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct StartTime(i64);

impl StartTime {
    /// The number to put in a column, and nothing a person should be shown.
    #[must_use]
    pub fn stored(self) -> i64 {
        self.0
    }

    /// A value read back out of one.
    ///
    /// Deliberately total: any integer is a start time this build might once have written, and a
    /// row that holds a value no live process has simply fails the comparison — which is the answer
    /// "that process is gone", arrived at without a special case.
    #[must_use]
    pub const fn from_stored(stored: i64) -> Self {
        Self(stored)
    }
}

/// When the process with this id began, or [`None`] if there is no such *running* process.
///
/// The other half of an identity, and the reason a pid alone is never enough: the OS reuses a pid
/// within minutes, so a supervisor that acted on one could ask an unrelated program to shut down.
/// Everything that is not a running process — a pid nobody holds, one this account may not ask
/// about, a process that has ended and not been reaped — is [`None`] rather than an error, because
/// they are one answer to the caller: *not the process you recorded*, and therefore not one to
/// signal.
///
/// # Errors
///
/// [`Error::Os`], or [`Error::Io`] on Linux, when the OS has the answer and would not give it — the
/// case that is neither "here it is" nor "there is no such process".
pub fn started_at(pid: u32) -> Result<Option<StartTime>> {
    sys::started_at(pid).map(|started| started.map(StartTime))
}

/// A process that survived the daemon which started it, taken over by the one running now.
///
/// **The third kind of relationship in this module, and the weakest.** A [`Supervised`] child is
/// this process's own — pipes, a group, a status to wait for; a [`Detached`] one is a process this
/// one let go of but still has a handle to. This is neither: the process was started by a daemon
/// that is gone, its pipes went with that daemon, and this process is not its parent. What is left
/// is exactly two things, and they are what roadmap task **T18** promises and nothing more:
///
/// - **whether it is still there**, asked by re-reading its start time rather than by trusting its
///   pid, and
/// - **that it can be stopped**, addressed as the process group it leads on Unix and as one process
///   on Windows, where the job object it belonged to went with its daemon.
///
/// What is not available is stated where it costs something: its output is not captured, because the
/// pipes belong to a process that no longer exists, so an adopted service's `current.log` stops at
/// the moment the daemon died and resumes when the service is next started properly. Its exit
/// **status** is not available either — see [`exited`](Self::exited).
///
/// Dropping this does nothing, unlike [`Supervised`], and the asymmetry is deliberate: this value is
/// not the group's ownership, it is a way of addressing something that was already running. A daemon
/// that goes away for a second time leaves the survivor exactly as it found it, for the next one to
/// adopt.
#[derive(Debug)]
pub struct Adopted {
    /// The process, and on Unix the group it leads.
    pid: u32,

    /// What was recorded when it was started, and what every later question is asked against.
    started: StartTime,
}

impl Adopted {
    /// Take over the process `pid` if it is still the one that began at `started`.
    ///
    /// **The pair is the whole check.** A pid that has been handed out again names a process created
    /// later, which carries a different start time and is refused here — and refusing is what keeps
    /// the one accident this product cannot have out of reach: signalling somebody else's program
    /// because a number was reused.
    ///
    /// [`None`] means the process is gone, in every sense the caller needs: nothing has that pid,
    /// something else does, or what does has ended. None of them is a failure — a daemon that was
    /// killed usually took its services with it, and that is the ordinary outcome of this call.
    ///
    /// # Errors
    ///
    /// As [`started_at`]: only the case where the OS has the answer and refuses to give it.
    pub fn identify(pid: u32, started: StartTime) -> Result<Option<Self>> {
        Ok(started_at(pid)?
            .filter(|running| *running == started)
            .map(|started| Self { pid, started }))
    }

    /// The process this handle addresses.
    #[must_use]
    pub fn pid(&self) -> u32 {
        self.pid
    }

    /// Whether it has ended, and — as far as this process can tell — how.
    ///
    /// Never waits, like [`Supervised::exited`], and answers the same shape so that a supervisor
    /// watching an adopted service can be the same loop. What it does *not* share is where the
    /// answer comes from: there is no status to reap for a process this one did not start, so the
    /// question is asked by re-reading the identity — the process is there while its pid still
    /// carries the start time that was recorded, and gone the moment it does not.
    ///
    /// **The [`Exit`] it hands back therefore carries no code on any platform**, and says so when it
    /// is rendered. Windows would in fact give one, through a handle this could keep open; it is not
    /// used, because a restart policy that behaved differently on one system for a service that
    /// merely disappeared would be a difference nobody could act on. It is reported as
    /// *unsuccessful* for the same reason the code is absent: nothing here saw the process end, so a
    /// clean exit cannot be claimed, and the safer of the two readings for a restart policy that
    /// only puts back what *failed* is the one that puts the service back.
    ///
    /// # Errors
    ///
    /// As [`started_at`].
    pub fn exited(&self) -> Result<Option<Exit>> {
        let still_there = self.still_the_one_recorded()?;

        Ok((!still_there).then(|| Exit {
            success: false,
            code: None,
            described: "gone (it outlived the daemon that started it, so no status was read)"
                .to_owned(),
        }))
    }

    /// Ask the whole group to stop and give it a chance to tidy up.
    ///
    /// `SIGTERM` to `-pgid` on Unix, exactly as [`Supervised::ask_to_stop`] sends it and resting on
    /// the same fact: the survivor called `setsid` before it became the program, so its pgid is its
    /// pid for as long as it lives.
    ///
    /// **Check [`CAN_ASK_TO_STOP`] first**, which is `false` on Windows for the reason it is false
    /// there for a child of our own — with even less of a console to reach this one through.
    ///
    /// # Errors
    ///
    /// [`Error::UnsupportedPlatform`] where the system has no way to ask, and [`Error::Os`] if it
    /// has one and refused — including when the identity could not be re-read, which is the one
    /// state this must not act through. A group that has already gone is not a failure.
    pub fn ask_to_stop(&self) -> Result<()> {
        if !self.still_the_one_recorded()? {
            return Ok(());
        }

        sys::ask_foreign_to_stop(self.pid)
    }

    /// Stop it, without a chance to tidy up.
    ///
    /// `SIGKILL` to `-pgid` on Unix; `TerminateProcess` on Windows, where the job object that would
    /// have taken the whole group went with the daemon that created it.
    ///
    /// **Returns as soon as the request is made, and cannot wait**: this process is not the
    /// survivor's parent, so there is no status to reap and nothing to block on. A caller that needs
    /// to know it has gone polls [`exited`](Self::exited), which is the same thing the identity check
    /// answers everywhere else.
    ///
    /// # Errors
    ///
    /// [`Error::Os`] if the OS refuses, or if the identity could not be re-read — which is deliberate
    /// and is the one case a stop must not push through. A process that had already gone is not a
    /// failure.
    pub fn stop(&self) -> Result<()> {
        if !self.still_the_one_recorded()? {
            return Ok(());
        }

        sys::stop_foreign(self.pid)
    }

    /// Whether the pid this holds still carries the start time it was identified by.
    ///
    /// **Asked again immediately before every signal**, not only when this value was made. The
    /// alternative is a handle that was right when it was built and is acted on later — the window
    /// between the two being the whole of a boot's reconciliation, or the days a service runs for —
    /// and what fills that window is the process ending and its number being handed to somebody else.
    /// This narrows the race to the two instructions between the check and the `kill`, which is the
    /// same residual `.claude/decisions/0007-supervised-child-owns-a-process-group.md` already
    /// accepts for a `Supervised`.
    ///
    /// It is also what makes Unix's fallback to signalling the bare pid defensible: a group id could
    /// only ever have been ours, where a pid is only ours because this said so a moment ago.
    fn still_the_one_recorded(&self) -> Result<bool> {
        Ok(started_at(self.pid)?.is_some_and(|running| running == self.started))
    }
}

/// Whether a child has exited, and how it put it.
///
/// Shared by both handles: the question is the same one whether this process is going to outlive the
/// child or the other way round.
fn exited(child: &mut Child) -> Result<Option<Exit>> {
    child
        .try_wait()
        .map(|status| status.map(describe))
        .map_err(|source| Error::Os {
            action: "check on the process it started",
            source,
        })
}

/// Render an exit status into the answers a caller needs from it.
fn describe(status: std::process::ExitStatus) -> Exit {
    Exit {
        success: status.success(),
        code: status.code(),
        described: status.to_string(),
    }
}

impl Exit {
    /// Whether it ended the way a process that did its job ends.
    #[must_use]
    pub fn is_success(&self) -> bool {
        self.success
    }

    /// The status it exited with, where the OS reports one.
    ///
    /// `None` for a Unix process killed by a signal, which has no exit code at all — reporting `0`
    /// for one would say "clean exit" about a crash, and that is why the wire type
    /// (`StateReason::Exited`) carries an `Option` too rather than flattening it. The
    /// [`Display`](fmt::Display) form is what to show a person; this is what a policy branches on.
    #[must_use]
    pub fn code(&self) -> Option<i32> {
        self.code
    }
}

impl fmt::Display for Exit {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.described)
    }
}

/// Start `program` detached from this process and from its terminal, with `directory` as its
/// working directory.
///
/// The environment is inherited, which is deliberate: `MIXENGINE_LOG_FORMAT` is set by a log
/// collector that wraps a command it did not write, and a child that dropped it would stop being
/// collected halfway through a start.
///
/// **`directory` is required rather than inherited**, and the caller is expected to name something
/// it is happy to have held for the child's whole life: a process's working directory is a reference
/// the OS keeps, so a daemon that kept its caller's would stop that directory from being renamed or
/// deleted on Windows and stop its filesystem from being unmounted on Unix. A client autostarting a
/// daemon is typically run from a project folder the person is working in, which is the last
/// directory worth pinning for days. The daemon's own home is the obvious answer.
///
/// # What this does to the calling process on Windows
///
/// It clears `HANDLE_FLAG_INHERIT` on **this process's** standard handles for the length of the
/// spawn, because there is no narrower way to keep the child from inheriting a pipe its caller is
/// reading to end-of-file — `windows/process.rs` has the whole of why. They are put back before this
/// returns, so a caller that goes on to start other children passes its stdio on to them as usual;
/// what remains is that a `CreateProcessW` running *concurrently* on another thread may not inherit
/// them, which is a window `bInheritHandles` already opens for every spawn in the program.
///
/// # Errors
///
/// [`Error::Io`] naming the program when it cannot be started — the usual reasons
/// being a binary that has been moved since this process was launched, or one an antivirus is
/// holding.
pub fn spawn_detached(program: &Path, args: &[OsString], directory: &Path) -> Result<Detached> {
    let mut command = Command::new(program);

    command
        .args(args)
        .current_dir(directory)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null());

    // Held across the spawn and no further: on Windows detaching a child means turning something
    // off on *this* process, and the drop below is what turns it back on — including when the spawn
    // is the thing that failed.
    let detaching = sys::detach(&mut command);
    let spawned = command.spawn();
    drop(detaching);

    spawned
        .map(|child| Detached { child })
        .map_err(|source| Error::Io {
            action: "start",
            path: program.to_path_buf(),
            source,
        })
}
