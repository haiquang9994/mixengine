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
//! [`hand_over`] is the fourth relationship and the strangest: a process that stops being itself.
//! The shim (roadmap task T25) resolves which PHP a directory uses and then has to *become* it —
//! same arguments, same streams, same terminal, same exit code. On Unix that is `exec` and there is
//! nothing left to describe; on Windows there is no such call, so it is a child in a Job Object with
//! the console events swallowed on the way past. See the function for what that costs.
//!
//! [`run_once`] is the fifth and the odd one out: a program run **to completion**, for its exit
//! status rather than for its running. `mariadb-admin ping` and `caddy stop` are what it is for
//! (roadmap task T15a) — a probe and a shutdown, both of which start a process the caller has no
//! interest in supervising. It is here rather than in the supervisor for the reason every spawn is:
//! a `Command` on Windows has to be told not to be given a console window, and that is a `#[cfg]` no
//! crate above this one is allowed to contain.
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
use std::time::Duration;

use tokio::io::{AsyncReadExt, AsyncWriteExt as _};

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

/// Whether a running process can be sent a signal on this system.
///
/// True on Unix, false on Windows, and for the reason [`CAN_ASK_TO_STOP`] is false there: a daemon
/// has no signal to send a process it gave no console to. Its own constant rather than a second
/// reading of that one, because they are two capabilities that happen to be absent together —
/// [`Supervised::ask_to_stop`] addresses a *group* and this addresses a *leader*, and a system that
/// gained one without the other would need to say so.
///
/// **A caller checks this before it waits**, not after. A reload that could never be delivered is a
/// line in the log at the moment it is asked for, rather than a patience spent on nothing.
pub const CAN_SIGNAL: bool = sys::CAN_SIGNAL;

/// A signal a running service can be sent.
///
/// This crate's own list rather than `mixengine_proto::ReloadSignal`: `mixengine-platform` depends
/// on no other crate in this workspace, and one enum is not a reason to open that edge — the daemon
/// holds both and maps one onto the other in three lines. The numbers stay inside `unix/process.rs`,
/// which is the only file in the workspace that may name a `libc` constant.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
#[non_exhaustive]
pub enum Signal {
    /// `SIGHUP`.
    Hup,

    /// `SIGUSR1`.
    Usr1,

    /// `SIGUSR2`.
    Usr2,
}

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

    /// Send `signal` to the process this handle names.
    ///
    /// **To the leader and not to the group**, which is the whole difference between this and
    /// [`ask_to_stop`](Self::ask_to_stop). A stop is addressed at every process holding the port,
    /// because a master that has already crashed cannot pass one on. A reload is addressed at the
    /// master precisely because it has not crashed: php-fpm's `SIGUSR2` is an instruction to
    /// *replace the workers*, and the same signal delivered to a worker mid-request is that request
    /// dropped.
    ///
    /// **Check [`CAN_SIGNAL`] first.** On Windows there is no such thing and this says so rather
    /// than succeeding quietly, because a caller that believed it would go on waiting for an effect
    /// nothing was ever asked to produce.
    ///
    /// # Errors
    ///
    /// [`Error::UnsupportedPlatform`] where the system has no signals, and [`Error::Os`] if it has
    /// them and refused. A process that has already gone is not a failure.
    pub fn signal(&self, signal: Signal) -> Result<()> {
        self.group.signal_leader(self.child.id(), signal)
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

    state_the_whole_environment(&mut command, env);

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

/// Give this child the environment `env` states, over the short per-OS floor and nothing else.
///
/// Shared by [`spawn_supervised`] and [`run_once`], and it has to be: a `mariadb-admin ping` that
/// saw a different environment from the `mariadbd` it is asking about would be asking about a
/// different server — a different socket, a different data directory, a different credential.
/// Duplicating the rule would let those two drift apart one edit at a time.
///
/// Cleared first, so what follows is the whole of it. The floor goes on before the caller's own
/// entries, which is what makes a spec able to override one — on Windows that comparison is
/// case-insensitive inside `Command`, so a spec naming `Path` replaces the inherited `PATH` rather
/// than adding a second variable the child would see only one of.
fn state_the_whole_environment(command: &mut Command, env: &BTreeMap<String, String>) {
    command.env_clear();

    for name in sys::INHERITED_ENV {
        if let Some(value) = std::env::var_os(name) {
            command.env(name, value);
        }
    }

    command.envs(env);
}

/// How long the last words of a one-shot are waited for once the process itself has ended.
///
/// Bounded because end of file on a pipe is not the process exiting but the *last holder of that
/// pipe* exiting, and a one-shot that left a descendant behind holds its own stdout open through
/// that descendant for as long as it lives. An unbounded wait here would hang [`run_once`] at the
/// one moment it has an answer to give.
///
/// Short because there is nothing left to wait for: the process has gone, so anything still in the
/// pipe is already written and only has to be read. Two seconds is the supervisor's number for the
/// same wait on a service's last log lines.
const LAST_WORDS: Duration = Duration::from_secs(2);

/// A program that was run to completion — or was still running when its patience ran out.
///
/// What [`run_once`] hands back, and it carries the output as well as the status because the two
/// callers that exist both need it for the same reason: a health probe that failed and a shutdown
/// command that failed are each a line in `daemon.log` that is worth nothing without the sentence
/// the program printed. `ERROR 1045: Access denied` is the whole of what a user has to act on, and
/// the exit code alone is `1`.
#[derive(Debug)]
pub struct Ran {
    /// How it ended, or [`None`] for one that was killed at its deadline.
    exit: Option<Exit>,

    /// What it printed, lossily decoded and trimmed. Whatever had arrived by the end for a run that
    /// timed out, which is usually all of it — the streams are read while the program runs, so a
    /// deadline cuts off a program that is still talking rather than one nobody was listening to.
    stdout: String,

    /// The same, for the stream a program complains on.
    stderr: String,
}

impl Ran {
    /// How it ended, or [`None`] if it was still running when the deadline passed and was killed.
    #[must_use]
    pub fn exit(&self) -> Option<&Exit> {
        self.exit.as_ref()
    }

    /// Whether it ran out of time.
    ///
    /// Distinct from failing, and the distinction is the caller's to act on: a `mariadb-admin ping`
    /// that answers "no" in 20 ms is a database refusing queries, and one that never answers is a
    /// database that has stopped listening — the second is also what a health check with no deadline
    /// would have read as *healthy* for as long as it stayed broken.
    #[must_use]
    pub fn timed_out(&self) -> bool {
        self.exit.is_none()
    }

    /// Whether it exited the way a program that did its job exits.
    #[must_use]
    pub fn succeeded(&self) -> bool {
        self.exit.as_ref().is_some_and(Exit::is_success)
    }

    /// The one line to put in a log about a run that did not go well.
    ///
    /// The last line the program printed, from `stderr` if it said anything there and from `stdout`
    /// otherwise — programs of this kind disagree about which stream a complaint belongs on, and the
    /// *last* line rather than the first because a usage error follows the banner it printed above
    /// it. [`None`] when the program said nothing at all, which is what the caller wants to know
    /// before it writes an empty field into a log line.
    #[must_use]
    pub fn complaint(&self) -> Option<&str> {
        [&self.stderr, &self.stdout]
            .into_iter()
            .find_map(|stream| stream.lines().next_back())
    }
}

/// Run `program` to completion and report how it ended, killing it if `patience` runs out first.
///
/// **The one-shot, and the whole of what separates it from [`spawn_supervised`] is intent**: the
/// caller wants an exit status, not a service. There is no process group, no ready check and no
/// restart — a program run for its answer that started children of its own would be a program doing
/// something this call is not for. `stdin` is the null device and both other streams are captured,
/// so a probe that decides to ask an interactive question is a timeout rather than a hang.
///
/// The environment and the working directory are the *service's*, applied by the same rule
/// [`spawn_supervised`] uses and by the same code: a probe that saw a different environment from the
/// server it is asking about would be asking about a different server.
///
/// **On Windows it is given no console window**, which is the reason this function is here at all
/// and not in the crate that wants it: a daemon has no console, so a console-subsystem child is
/// handed a *new* one, and on Windows 11 that is a terminal window appearing on the user's desktop —
/// every ten seconds, for a health probe. `windows/process.rs` has the whole of that story, measured
/// rather than reasoned about.
///
/// # What running out of patience means
///
/// The process is killed and the run answers [`Ran::timed_out`]. Only the process this call started:
/// a one-shot that forked is out of scope, deliberately, because the alternative is a session and a
/// group for something whose whole life is meant to be a few milliseconds — and no probe or
/// shutdown command any service documents behaves that way. What is guaranteed is that no process
/// outlives this call unnoticed, which the kill covers.
///
/// **`patience` bounds the process ending, not its pipes closing**, and the two are only usually the
/// same moment. End of file on a pipe arrives when the *last holder* of it exits, and a one-shot
/// that leaves a descendant behind has handed that descendant its stdout: a `mariadb-admin shutdown`
/// of that shape would be reported as having timed out long after it exited `0`, and the caller
/// would kill the database it had just asked to shut down cleanly — every ten seconds, for a probe.
/// So the deadline is read off [`tokio::process::Child::wait`] alone, and the last of the output is
/// then waited for separately and briefly (`LAST_WORDS`). The supervisor bounds the same wait, for
/// the same reason, for a service's last log lines.
///
/// What a timed-out program printed is kept rather than thrown away — the streams are drained
/// alongside the wait, so by the time the deadline passes it is already in hand. That draining is
/// not only for the log: a pipe holds a page or two and a program that fills one blocks on its own
/// write until somebody reads, so a probe with a screen of complaint to make would otherwise stop at
/// that write and be timed out as if it had hung.
///
/// # Errors
///
/// [`Error::Io`] naming the program when it cannot be started at all — a probe whose binary is not
/// installed, which is a spec to fix rather than a service to degrade — and [`Error::Os`] if the OS
/// will not say how a process it started ended.
pub async fn run_once(
    program: &Path,
    args: &[OsString],
    directory: &Path,
    env: &BTreeMap<String, String>,
    patience: Duration,
) -> Result<Ran> {
    run(program, args, directory, env, patience, None).await
}

/// [`run_once`], with `input` written to the program's standard input, which is then closed.
///
/// **For a program whose whole instruction is what it reads** — `mariadbd --bootstrap`, which takes
/// its SQL there and listens on no port and no socket while it does. The alternative for that case
/// is a temporary file, which for a statement that sets a root password would be a plaintext
/// credential on disk.
///
/// # The size of `input` is the caller's to bound, and has to be
///
/// The write happens *before* the wait, so an input larger than the kernel will take in one go
/// would block here rather than being timed out. That is affordable because the only caller writes
/// a handful of SQL statements and a pipe buffer is a page or more — and it is stated rather than
/// guarded, because a guard would be a second task and a second failure path for a case nothing in
/// this workspace has.
///
/// # Errors
///
/// As [`run_once`], and [`Error::Io`] naming the program when its standard input cannot be written
/// to at all — a program that closed it and exited, which is the same class of failure as one that
/// would not start.
pub async fn run_once_with_input(
    program: &Path,
    args: &[OsString],
    directory: &Path,
    env: &BTreeMap<String, String>,
    patience: Duration,
    input: &str,
) -> Result<Ran> {
    run(program, args, directory, env, patience, Some(input)).await
}

/// The body both of the above are, differing only in what the child is given to read.
async fn run(
    program: &Path,
    args: &[OsString],
    directory: &Path,
    env: &BTreeMap<String, String>,
    patience: Duration,
    input: Option<&str>,
) -> Result<Ran> {
    let mut command = tokio::process::Command::new(program);
    command
        .args(args)
        .current_dir(directory)
        .stdin(match input {
            None => Stdio::null(),
            Some(_) => Stdio::piped(),
        })
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        // What makes the timeout below a promise rather than a hope: the future being dropped is
        // what kills the process, so a caller that gives up on this call — a health loop cancelled
        // by a daemon shutting down, say — does not leave the probe behind it.
        .kill_on_drop(true);

    state_the_whole_environment(command.as_std_mut(), env);
    sys::arrange_one_shot(command.as_std_mut());

    // Held for the length of the spawn, for the reason `spawn_supervised` holds it: an inheritable
    // handle this process was given is passed on to every child it starts, and a probe running every
    // ten seconds is ten seconds of another process's pipe being held open, for ever.
    let hiding = hide_stdio_from_children();
    let spawned = command.spawn();
    drop(hiding);

    let mut child = spawned.map_err(|source| Error::Io {
        action: "start",
        path: program.to_path_buf(),
        source,
    })?;

    // Written and closed before anything waits: the end of file is what tells the program its
    // instruction is complete, and a pipe left open is a `mariadbd --bootstrap` sitting there for a
    // statement that will never come, until this call's patience runs out.
    if let Some(input) = input {
        let mut sink = child.stdin.take().ok_or_else(|| Error::Io {
            action: "write to the standard input of",
            path: program.to_path_buf(),
            source: std::io::Error::other("the child was given no standard input to write to"),
        })?;

        sink.write_all(input.as_bytes())
            .await
            .map_err(|source| Error::Io {
                action: "write to the standard input of",
                path: program.to_path_buf(),
                source,
            })?;

        // Dropping the handle is what closes the pipe; flushed first so nothing is left buffered.
        let _ = sink.shutdown().await;
        drop(sink);
    }

    // Taken out of the child so that what the deadline below bounds is the process ending and
    // nothing else. `wait_with_output` would have waited on these too, which is the hazard this
    // function's documentation describes: end of file is the last holder of the pipe exiting.
    let mut out = child.stdout.take();
    let mut err = child.stderr.take();

    let mut stdout = Vec::new();
    let mut stderr = Vec::new();

    let waited = {
        // Both streams at once, and alongside the wait rather than after it — one at a time is not
        // enough, because whichever went unread would be the one the program filled and blocked on.
        let mut saying = std::pin::pin!(async {
            tokio::join!(
                drain(out.as_mut(), &mut stdout),
                drain(err.as_mut(), &mut stderr),
            );
        });
        let mut ending = std::pin::pin!(child.wait());
        let mut quiet = false;

        let waited = tokio::time::timeout(patience, async {
            loop {
                tokio::select! {
                    // The only branch that ends this: the deadline is the process's.
                    status = &mut ending => break status,

                    // Guarded because a future that has finished must not be polled again, and this
                    // one usually finishes first — a well-behaved program closes its pipes as it
                    // exits, and most of them have already been closed for it by the kernel.
                    () = &mut saying, if !quiet => quiet = true,
                }
            }
        })
        .await;

        // It has ended; its pipes may not have, and there is no telling how long they will take.
        if waited.is_ok() && !quiet {
            let _ = tokio::time::timeout(LAST_WORDS, saying).await;
        }

        waited
    };

    // Killed at the deadline by the drop at the end of this function, which `kill_on_drop` arranged.
    // What it managed to say on the way is kept: it was read while it ran, so it costs nothing here.
    let Ok(status) = waited else {
        return Ok(Ran {
            exit: None,
            stdout: said(&stdout),
            stderr: said(&stderr),
        });
    };

    let status = status.map_err(|source| Error::Os {
        action: "wait for a program it ran",
        source,
    })?;

    Ok(Ran {
        exit: Some(describe(status)),
        stdout: said(&stdout),
        stderr: said(&stderr),
    })
}

/// Read everything a program says on one of its pipes, keeping whatever arrived before any error.
///
/// The [`Option`] is the child's own — [`tokio::process::Child`] holds each stream in one, and it is
/// [`None`] for a stream that was taken already.
///
/// A read error is dropped rather than raised, because what this call exists to produce is an exit
/// status: a short read on a pipe is no reason to withhold one, and the bytes that did arrive are
/// still the program's complaint.
async fn drain<R>(pipe: Option<&mut R>, into: &mut Vec<u8>)
where
    R: tokio::io::AsyncRead + Unpin,
{
    if let Some(pipe) = pipe {
        let _ = pipe.read_to_end(into).await;
    }
}

/// What a program printed, as something that can go in a log line.
///
/// Lossy on purpose: a probe's complaint is read by a person, and refusing to show it because a
/// database wrote a byte in the machine's own eight-bit encoding would lose the only evidence there
/// is. Trimmed because the last thing a program writes is a newline and a log line does not want it.
fn said(stream: &[u8]) -> String {
    String::from_utf8_lossy(stream).trim().to_owned()
}

/// Become `program`: run it in this process's place and answer with the status it ended on.
///
/// **What a shim is made of.** A process the user invoked as `php` has worked out which PHP this
/// directory means and now has to get out of the way of it — with the same arguments, the same
/// standard streams, the same terminal and, at the end, the same exit code. `env` is applied *over*
/// this process's own environment rather than replacing it, which is the opposite of
/// [`spawn_supervised`] and for the opposite reason: a service's environment is declared in full by
/// its spec, and a shim is standing in the middle of somebody's shell session, where everything they
/// exported has to arrive intact.
///
/// # Unix: there is nothing to describe
///
/// `execve`. The process image is replaced, so the pid, the streams, the terminal, the process group
/// and every signal disposition are the ones the user's shell set up — Ctrl-C reaches the program
/// because the program *is* this process. **It returns only on failure**, which is why the `i32` in
/// the signature is Windows's answer and not a value any Unix caller will see.
///
/// # Windows: a child, and two things arranged around it
///
/// There is no `exec`, so the program is a child of a process that then does nothing but wait.
///
/// - **A Job Object with `KILL_ON_JOB_CLOSE`**, so a shim that is killed does not leave the program
///   it fronted running: `taskkill` on a `php -S` would otherwise take the shim and leave the
///   server holding the port, with nothing on the machine still naming it.
/// - **Ctrl-C and Ctrl-Break are swallowed by this process**, in `windows/process.rs`.
///   A console event goes to *every* process attached to the console, so the child already has its
///   own copy; the default handling would end the shim first, close the job, and kill the child
///   before it could act on the interrupt it had just been sent. Closing the window, signing out and
///   shutting down are deliberately left alone — those are the cases where the child *should* go
///   down with this process.
///
/// **A child that exits before it can be put in the job is not a failure here**, which is where this
/// departs from [`spawn_supervised`]. Windows will not assign an ended process to a job and reports
/// that as `ERROR_ACCESS_DENIED`, indistinguishable from a real refusal — and for a shim the case is
/// not exotic but the common one, since `php -v` is over in a few milliseconds. Failing the run, or
/// killing the child, would turn the ordinary invocation into an error; so the assignment is
/// attempted, and the wait happens either way. What is lost when it does fail is the guarantee
/// above, for a program that has already finished.
///
/// # Errors
///
/// [`Error::Io`] naming the program when it cannot be started at all — which for a shim means an
/// install whose directory has been emptied — and [`Error::Os`] when Windows will not create the job
/// object, will not let this process handle its own console events, or cannot be waited on.
pub fn hand_over(
    program: &Path,
    args: &[OsString],
    env: &BTreeMap<String, OsString>,
) -> Result<i32> {
    let mut command = Command::new(program);

    // Everything else — the streams, the working directory, the rest of the environment — is
    // inherited by saying nothing about it, which is exactly what standing in for a program means.
    command.args(args).envs(env);

    sys::hand_over(command, program)
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
