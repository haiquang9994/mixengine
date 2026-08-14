//! `DETACHED_PROCESS`, a process group of the child's own, and no inherited handles.
//!
//! A console is inherited on Windows, not merely written to: a child started from a terminal joins
//! that terminal's console, keeps it alive, and receives every control event sent to it.
//! `DETACHED_PROCESS` is the flag that says do none of that — the child gets no console, so closing
//! the window it was started from cannot reach it and it has nothing to print into.
//!
//! `CREATE_NEW_PROCESS_GROUP` on top of it is about `GenerateConsoleCtrlEvent`, which addresses a
//! process *group*: without it, a Ctrl-Break aimed at the group the parent belongs to would still be
//! delivered to a child that has no console of its own.
//!
//! The consequence is deliberate and is written down in `windows/signal.rs`: a daemon started this
//! way can never receive a console control event, because there is no console to send it one.
//!
//! **The third thing is the one that had to be found rather than designed.** `CreateProcessW` is
//! called by the standard library with `bInheritHandles = TRUE`, which it needs for the child's
//! stdio, and *inheritable* is a property that survives inheritance: a handle this process was
//! itself handed by its parent arrives still marked inheritable and is passed on again. So a
//! `mixengined --detach` started with its stdout on a pipe — which is exactly what a client
//! autostarting a daemon does, and what `std::process::Command::output` does — gave the daemon a
//! copy of that pipe. Redirecting the child's own stdio to the null device does not help: the extra
//! copy is not the child's stdout, it is simply a handle the child holds, and the writing end of a
//! pipe stays open while anybody holds one. The caller reading that pipe then waits for an
//! end-of-file that arrives when the daemon exits, days later. Reproduced before this was written,
//! as a `--detach` that returned promptly and a parent that never did.
//!
//! Clearing that flag is the only way to say it — `bInheritHandles` is per `CreateProcessW` and the
//! flag is per handle, so there is no third place to put "this one child gets nothing". What can be
//! narrowed is *how long* it is said for: the flag is put back as soon as the spawn returns, so a
//! caller that goes on to start other children — `mix`, the GUI at roadmap task T10 — passes its
//! stdio on to them exactly as it would have. The window that remains is the spawn itself, and it is
//! the same window `bInheritHandles` already makes process-wide for every concurrent `CreateProcessW`
//! in the program; this makes it no wider.
//!
//! # The other half: a child that must *not* outlive us
//!
//! Everything above is about letting go of a process. [`group`] is the opposite request, and on this
//! system it is the easy one: a Job Object with `JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE` takes its whole
//! membership down when the last handle to it closes, which a killed daemon does exactly as reliably
//! as an exiting one. No code of ours runs, and grandchildren are covered — neither of which is true
//! of the Unix side, and
//! `.claude/decisions/0007-supervised-child-owns-a-process-group.md` is where that difference is
//! written down rather than averaged out.

use std::io;
use std::os::windows::io::AsRawHandle as _;
use std::os::windows::process::CommandExt as _;
use std::path::Path;
use std::process::{Child, Command};
use std::sync::{Mutex, PoisonError};

use windows_sys::Win32::Foundation::{
    CloseHandle, ERROR_ACCESS_DENIED, ERROR_INVALID_PARAMETER, FILETIME, GetHandleInformation,
    HANDLE, HANDLE_FLAG_INHERIT, INVALID_HANDLE_VALUE, SetHandleInformation,
};
use windows_sys::Win32::System::Console::{
    CTRL_BREAK_EVENT, CTRL_C_EVENT, GetStdHandle, STD_ERROR_HANDLE, STD_INPUT_HANDLE,
    STD_OUTPUT_HANDLE, SetConsoleCtrlHandler,
};
use windows_sys::Win32::System::JobObjects::{
    AssignProcessToJobObject, CreateJobObjectW, JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE,
    JOBOBJECT_EXTENDED_LIMIT_INFORMATION, JobObjectExtendedLimitInformation,
    SetInformationJobObject, TerminateJobObject,
};
use windows_sys::Win32::System::Threading::{
    CREATE_NEW_PROCESS_GROUP, CREATE_NO_WINDOW, DETACHED_PROCESS, GetProcessTimes, OpenProcess,
    PROCESS_QUERY_LIMITED_INFORMATION, PROCESS_TERMINATE, TerminateProcess,
};
use windows_sys::core::BOOL;

use crate::{Error, Result};

/// One holder's share of "this process is not handing its standard handles on".
///
/// Held by [`detach`]'s caller across the spawn and dropped straight after it. Restoring in `Drop`
/// rather than in a function the caller has to remember is the point: a spawn that fails must put
/// the flags back too, and there is no path out of `spawn_detached` that skips a drop.
///
/// **A share and not the state itself, because the state is the process's.** The flag lives on a
/// handle this whole program shares, so two threads asking for it at once — a daemon starting a
/// service while a health probe runs its ten-second command, which is now the ordinary case — are
/// two holders of one thing rather than two independent guards. Held independently, the second
/// caller would find the flags already cleared and record nothing to restore, and the *first* one
/// dropping would hand the handles back out while the second was still spawning; the long-lived
/// service started in that window inherits the daemon's stdio and holds a client's pipe open for
/// days, which is the exact bug this module exists to prevent, made common instead of rare. So the
/// count is kept in [`HIDING`] and only the last holder out puts the flags back.
///
/// The private field carries no information and is not optional: without it this is a unit struct,
/// which anything in the crate can write for itself. Such a value counts as a holder when it drops
/// and never was one — the count goes below what is live, and in a release build it wraps to
/// `usize::MAX`, where the last real holder leaving finds a number above zero and puts the flags
/// back never. The only way to hold one is to have asked [`hide_stdio`] for it.
#[derive(Debug)]
pub(crate) struct Detaching(());

/// How many [`Detaching`] guards are alive, and what the first of them turned off.
///
/// Process-wide because the thing it describes is: `HANDLE_FLAG_INHERIT` is per handle, and these
/// three handles belong to the program rather than to any thread of it.
static HIDING: Mutex<Hiding> = Mutex::new(Hiding {
    holders: 0,
    restore: Vec::new(),
});

/// The state behind [`HIDING`].
#[derive(Debug)]
struct Hiding {
    /// Live guards. The flags stay off while this is above zero.
    holders: usize,

    /// Only the handles the *first* guard actually changed, as `usize` because a raw pointer is not
    /// `Send` and this outlives every thread that touches it. A handle that was already
    /// non-inheritable is left out, so nothing here can hand out an inheritance the process did not
    /// have to begin with.
    restore: Vec<usize>,
}

/// Arrange for the child to have no console, no group in common with this process, and none of the
/// handles this process was given.
pub(crate) fn detach(command: &mut Command) -> Detaching {
    command.creation_flags(DETACHED_PROCESS | CREATE_NEW_PROCESS_GROUP);

    hide_stdio()
}

/// The handle half of [`detach`], on its own.
///
/// Separate because the hazard is not the detached child's alone. Inheritance is transitive: a
/// client that starts `mixengined --detach` and reads it to end-of-file hands *its* standard handles
/// to that middle process, which hands them on again to the daemon — so the daemon ends up holding a
/// pipe two processes away, and the one reading it waits forever. Clearing the flag inside
/// `spawn_detached` cannot help there, because by then the copy has already been made. Every process
/// in a chain like that has to decline to pass its own handles on, which is what this is for.
/// Reentrant: the first caller clears the flags, every caller after it joins the one that is already
/// in force, and the last one out restores them. See [`Detaching`] for what goes wrong without that.
pub(crate) fn hide_stdio() -> Detaching {
    let mut hiding = HIDING.lock().unwrap_or_else(PoisonError::into_inner);

    if hiding.holders == 0 {
        hiding.restore = stop_handing_on_the_standard_handles();
    }

    hiding.holders += 1;

    Detaching(())
}

impl Drop for Detaching {
    fn drop(&mut self) {
        let mut hiding = HIDING.lock().unwrap_or_else(PoisonError::into_inner);

        hiding.holders -= 1;

        if hiding.holders > 0 {
            return;
        }

        for handle in std::mem::take(&mut hiding.restore) {
            #[expect(
                unsafe_code,
                reason = "SetHandleInformation only sets a flag on the handle it is given, which is \
                          one this process fetched from GetStdHandle and has not closed"
            )]
            unsafe {
                SetHandleInformation(handle as HANDLE, HANDLE_FLAG_INHERIT, HANDLE_FLAG_INHERIT);
            }
        }
    }
}

/// A Job Object holding one supervised service and everything it starts.
///
/// `JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE` is the whole mechanism: when the last handle to the job
/// closes, the kernel terminates every process in it. A daemon that *exits* closes this handle, and
/// so does a daemon that is killed — there is no cleanup code that has to run for the guarantee to
/// hold, which is why Windows is the strongest of the three platforms in
/// `.claude/decisions/0007-supervised-child-owns-a-process-group.md`.
///
/// One job per service rather than one for the daemon, so `TerminateJobObject` means "stop this
/// service and its children" and not "stop everything". It is also the object Phase 7 hangs CPU and
/// memory caps on (roadmap task T68), which per-daemon jobs would have had to be rebuilt to allow.
#[derive(Debug)]
pub(crate) struct Group {
    /// Owned. Closing it is what kills the group, so it is closed in exactly one place — [`Drop`].
    job: HANDLE,
}

/// A kernel handle is process-wide and not tied to the thread that opened it, so a `Group` may be
/// moved and shared like the pgid its Unix counterpart holds. Without this the supervisor could not
/// keep a `Supervised` across an `await`, and the two platforms would have different API shapes for
/// no reason the caller could act on.
#[expect(
    unsafe_code,
    reason = "a job object handle is valid from any thread of this process and this type owns it, \
              so nothing here can be closed twice or used after closing"
)]
unsafe impl Send for Group {}

#[expect(
    unsafe_code,
    reason = "every call this type makes on the handle is atomic in the kernel and none of them \
              mutate the Rust value, so a shared reference from two threads is sound"
)]
unsafe impl Sync for Group {}

/// Create the job a supervised child will be put into, before there is a child to put into it.
///
/// Anonymous and with default security: nothing else has any business finding it by name, and a job
/// nobody can open by name cannot be joined by a process we did not start.
pub(crate) fn group() -> Result<Group> {
    #[expect(
        unsafe_code,
        reason = "CreateJobObjectW is given two null pointers, which is how it is asked for an \
                  anonymous job with default security; it borrows nothing"
    )]
    let job = unsafe { CreateJobObjectW(std::ptr::null(), std::ptr::null()) };

    if job.is_null() {
        return Err(Error::Os {
            action: "create a job object for a supervised process",
            source: io::Error::last_os_error(),
        });
    }

    // Owned from here on, so the failure below closes it rather than leaking it.
    let group = Group { job };
    group.kill_everything_in_it_when_this_handle_closes()?;

    Ok(group)
}

/// What a supervised child is started with.
///
/// `CREATE_NO_WINDOW` and nothing else. A daemon has no console of its own, so a console subsystem
/// child — `php-fpm.exe`, `mariadbd.exe` — would otherwise be given a *new* console, which is a
/// black window appearing on the user's desktop every time a service starts. The flag suppresses the
/// window without touching the child's standard handles, which are the pipes `spawn_supervised` gave
/// it.
///
/// Deliberately **not** `CREATE_NEW_PROCESS_GROUP`, which the detached path does use: its only
/// purpose is to make the child addressable by `GenerateConsoleCtrlEvent`, and a daemon with no
/// console cannot send one. T15 answered that question and left the flag off:
/// `.claude/decisions/0008-no-signal-stop-on-windows.md` sets out why reaching a console event from
/// here would mean detaching the daemon's own console and disabling its control handler for the
/// length of the call, and what a service that needs a graceful stop uses instead.
///
/// A free function taking no group, because on this system the group is joined after the spawn and
/// on the other one it is joined by the child itself — neither has anything to say at this point
/// beyond what goes on the `Command`.
pub(crate) fn arrange(command: &mut Command) {
    without_a_window(command);
}

/// What a program run for its exit status is started with.
///
/// The same `CREATE_NO_WINDOW` a supervised child gets, for a reason that bites harder here: a
/// health probe runs every ten seconds for as long as the service is up, so a console handed out per
/// run is a window opening on the user's desktop six times a minute, for ever. Deliberately not
/// `DETACHED_PROCESS`, which would take the child's console away *and* its inherited standard
/// handles — and this call is made for the output those handles carry.
pub(crate) fn arrange_one_shot(command: &mut Command) {
    without_a_window(command);
}

/// Start this child without a console window, wherever in the platform layer it is started from.
///
/// **Every `Command` this crate runs on Windows has to say this, not only the supervised ones.** A
/// process that has no console — a detached `mixengined`, and so every daemon a client autostarts —
/// gives a console subsystem child nothing to inherit, and Windows answers that by creating a
/// console for it. On Windows 11 a new console is handed to the *default terminal application*,
/// which opens a window of its own; with the default setting of "let Windows decide" that is
/// Windows Terminal. So the eight `icacls` calls that make a home private became eight terminal
/// windows on the desktop, one per call, every time a daemon started. Measured, not reasoned about:
/// one `mixengined --detach` produced nine of them.
///
/// `CREATE_NO_WINDOW` is the answer for a child whose output we read: the console is still created,
/// so `.output()` gets its pipes as usual, and no window is ever handed out for it.
pub(crate) fn without_a_window(command: &mut Command) {
    command.creation_flags(CREATE_NO_WINDOW);
}

/// Run `command` in this process's place, as close to an `exec` as this system gets.
///
/// The three steps are in the order they have to be in, and each is why the one before it is not
/// enough on its own:
///
/// 1. **The job first**, before the child exists, so there is something to put it into the moment it
///    does. [`group`]'s `KILL_ON_JOB_CLOSE` is what keeps a killed shim from leaving a `php -S`
///    holding a port.
/// 2. **The console handler before the spawn**, because the window between them is one where a
///    Ctrl-C would end this process by default and take the child down with the job.
/// 3. **The assignment after the spawn**, which is the one thing that cannot be done in the right
///    order — see [`Group::adopt`]. A failure is *not* propagated here, unlike everywhere else in
///    this module: a child that has already exited cannot be assigned and Windows says
///    `ERROR_ACCESS_DENIED` for it, which for a shim in front of `php -v` is the ordinary case
///    rather than an exotic one.
///
/// The standard handles are inherited rather than hidden — the opposite of every other spawn here,
/// and the point: this child *is* the program the user ran, so it writes to their terminal and reads
/// from their pipe.
pub(crate) fn hand_over(mut command: Command, program: &Path) -> Result<i32> {
    let group = group()?;
    ignore_console_interrupts()?;

    let mut child = command.spawn().map_err(|source| Error::Io {
        action: "run",
        path: program.to_path_buf(),
        source,
    })?;

    let _ = group.adopt(&child);

    let status = child.wait().map_err(|source| Error::Os {
        action: "wait for the program it handed over to",
        source,
    })?;

    // `code` is `None` only for a process ended by something that is not an exit status, which on
    // this system means it was terminated — `TerminateProcess`, or the job being killed. 1 is what
    // a shell reads as "it did not work", and the alternative is inventing a zero for a program that
    // was killed.
    Ok(status.code().unwrap_or(1))
}

/// Stop this process from being ended by a Ctrl-C or a Ctrl-Break meant for the program it started.
///
/// **A console control event is broadcast, not routed.** Every process attached to the console gets
/// its own copy, so the child has already been told; what this prevents is *this* process acting on
/// its copy, since the default action would end the shim, close the job handle, and kill the child
/// in the same moment it was deciding what to do about the interrupt. A shell that has just had
/// Ctrl-C pressed in it would then see the prompt come back while the program it was running died
/// half way through writing a file.
///
/// **Only those two.** The handler answers `FALSE` for `CTRL_CLOSE_EVENT`, `CTRL_LOGOFF_EVENT` and
/// `CTRL_SHUTDOWN_EVENT`, which passes them to the default handler and ends this process — and that
/// is right: the window is gone, and a child that survived it would be exactly the orphan the job
/// object exists to prevent.
fn ignore_console_interrupts() -> Result<()> {
    /// Says "handled, and I am doing nothing about it" for the two events the child gets a copy of.
    ///
    /// Async-signal-safety has no Windows equivalent, but the constraint is the same in spirit: this
    /// runs on a thread the OS creates inside this process, so it touches nothing and allocates
    /// nothing.
    #[expect(
        unsafe_code,
        reason = "the unsafety is the signature Windows calls this through and nothing in the body, \
                  which reads one integer argument and returns another"
    )]
    unsafe extern "system" fn ignore(event: u32) -> BOOL {
        BOOL::from(matches!(event, CTRL_C_EVENT | CTRL_BREAK_EVENT))
    }

    #[expect(
        unsafe_code,
        reason = "the routine is a function of this module with the signature Windows documents, \
                  and adding a handler only appends to a per-process list"
    )]
    let registered = unsafe { SetConsoleCtrlHandler(Some(ignore), 1) };

    if registered == 0 {
        return Err(Error::Os {
            action: "keep a Ctrl-C from ending the shim before the program it started",
            source: io::Error::last_os_error(),
        });
    }

    Ok(())
}

/// Whether a group can be *asked* to stop on this system, as opposed to being killed.
///
/// **False**, and this is the honest answer rather than a missing feature. A console control event
/// is the only signal-shaped thing Windows has, and it travels through a *console*: a supervised
/// child is started with `CREATE_NO_WINDOW` and the daemon that started it has no console at all, so
/// sending one would mean `FreeConsole`, `AttachConsole(child)`, disabling this process's own control
/// handler, `GenerateConsoleCtrlEvent`, and putting all three back — process-wide state, changed
/// from one thread of a daemon that is supervising other services on the others. The services that
/// need a graceful stop here have a command for it (`mariadb-admin shutdown`), which is what
/// `StopBehaviour::Command` is for. `.claude/decisions/0008-no-signal-stop-on-windows.md` has the
/// alternatives that lost.
pub(crate) const CAN_ASK_TO_STOP: bool = false;

/// The variables a child is given even though its spec did not name them.
///
/// **The list is long here because Windows programs do not survive an empty environment.** A cleared
/// block costs a service `SystemRoot`, and everything that loads a system DLL by relative path —
/// Winsock, the crypto providers, the C runtime — fails to initialise with an error that names none
/// of this. On Unix the same list is nine entries and none of them is load-bearing; here the first
/// eight are, which is why the two are written out per OS rather than merged into one list with
/// `#[cfg]`s through it.
///
/// Everything after the loader's needs is the same session-not-service rule Unix uses: a temporary
/// directory, who the user is, where their profile lives. Inherited only when this process has them
/// and never invented, and a spec that names one overrides it.
pub(crate) const INHERITED_ENV: &[&str] = &[
    // The loader's, and not optional.
    "SystemRoot",
    "SystemDrive",
    "windir",
    "COMSPEC",
    "PATH",
    "PATHEXT",
    "PROCESSOR_ARCHITECTURE",
    "NUMBER_OF_PROCESSORS",
    // The session's.
    "TEMP",
    "TMP",
    "USERPROFILE",
    "LOCALAPPDATA",
    "APPDATA",
    "PROGRAMDATA",
    "ALLUSERSPROFILE",
    "ProgramFiles",
    "ProgramFiles(x86)",
    "ProgramW6432",
    "PUBLIC",
    "HOMEDRIVE",
    "HOMEPATH",
    "USERNAME",
    "USERDOMAIN",
    "COMPUTERNAME",
    "OS",
];

impl Group {
    /// There is no way to ask; see [`CAN_ASK_TO_STOP`].
    ///
    /// Reached only by a caller that ignored that constant, so it says what it is rather than
    /// pretending to have asked — a silent success here would be a grace period spent waiting for a
    /// message nobody sent.
    pub(crate) fn request_stop(&self, _pid: u32) -> Result<()> {
        Err(Error::UnsupportedPlatform {
            capability: "asking a supervised process group to stop",
            reason: "Windows has no signal a daemon can send to a process it did not give a console \
                     to — a service that needs to shut down cleanly is stopped with its own command \
                     instead"
                .to_owned(),
        })
    }

    /// Put the child into the job, now that it exists.
    ///
    /// **After the spawn, and that is the one weak point in the Windows story.** Assigning before
    /// the child runs means `CREATE_SUSPENDED` plus a `ResumeThread` on the child's initial thread,
    /// and `std::process::Child` hands out a process handle but never a thread one — so doing it in
    /// the right order means replacing `Command` with a hand-rolled `CreateProcessW`. A daemon
    /// killed between those two calls therefore leaves the child behind; the window is one call
    /// wide, and crash recovery (roadmap task T18) covers exactly that case. ADR 0007 records the
    /// trade.
    ///
    /// A child that has already exited cannot be assigned, and Windows says so with
    /// `ERROR_ACCESS_DENIED` — indistinguishable from the real thing here, so it is reported as the
    /// failure it is rather than guessed at.
    pub(crate) fn adopt(&self, child: &Child) -> Result<()> {
        #[expect(
            unsafe_code,
            reason = "both handles are owned elsewhere and outlive the call — the job by this \
                      value, the process by the Child the caller is holding — and the call closes \
                      neither"
        )]
        let assigned = unsafe { AssignProcessToJobObject(self.job, child.as_raw_handle().cast()) };

        if assigned == 0 {
            return Err(Error::Os {
                action: "put a supervised process into its job object",
                source: io::Error::last_os_error(),
            });
        }

        Ok(())
    }

    /// Kill the job: every process in it, at once, without a chance to tidy up.
    ///
    /// The pid is what the Unix counterpart addresses and is unused here — the job knows its own
    /// members, including the ones the service started after we stopped looking.
    ///
    /// Exit code 1, so a service killed this way is not mistaken for one that stopped successfully
    /// by whatever reads the status afterwards.
    pub(crate) fn terminate(&self, _pid: u32) -> Result<()> {
        #[expect(
            unsafe_code,
            reason = "the handle is owned by this value and is not closed by the call; the exit \
                      code is passed by value"
        )]
        let terminated = unsafe { TerminateJobObject(self.job, 1) };

        if terminated == 0 {
            return Err(Error::Os {
                action: "stop a supervised process group",
                source: io::Error::last_os_error(),
            });
        }

        Ok(())
    }

    /// Set the one limit this job exists for.
    ///
    /// `JOBOBJECT_EXTENDED_LIMIT_INFORMATION` rather than the basic struct, because
    /// `KILL_ON_JOB_CLOSE` is only accepted through the extended one — and because it is the struct
    /// T68's memory caps go into, so the shape here is already the shape that grows.
    fn kill_everything_in_it_when_this_handle_closes(&self) -> Result<()> {
        let mut limits: JOBOBJECT_EXTENDED_LIMIT_INFORMATION =
            unsafe_zeroed_because_every_field_is_a_number();
        limits.BasicLimitInformation.LimitFlags = JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE;

        #[expect(
            unsafe_code,
            reason = "the pointer is to a local this frame owns and the length is that local's own \
                      size, so the kernel reads exactly the struct that is there"
        )]
        let set = unsafe {
            SetInformationJobObject(
                self.job,
                JobObjectExtendedLimitInformation,
                std::ptr::from_ref(&limits).cast(),
                size_of::<JOBOBJECT_EXTENDED_LIMIT_INFORMATION>() as u32,
            )
        };

        if set == 0 {
            return Err(Error::Os {
                action: "arrange for a supervised process group to be killed with the daemon",
                source: io::Error::last_os_error(),
            });
        }

        Ok(())
    }
}

/// When the process with this id began, as a `FILETIME` — 100-nanosecond ticks since 1601.
///
/// **Wall clock, so it identifies a process across a reboot as well as within one.** `GetProcessTimes`
/// reports the creation time the kernel recorded, which is the same number for the same process for
/// as long as it lives; a pid the OS has handed out again names a process created later and is
/// refused by the comparison the caller makes.
///
/// [`None`] rather than an error for every way of *not* being a running process, because that is the
/// answer the caller acts on and they are otherwise three different failures:
///
/// - the pid names nothing, which Windows reports as `ERROR_INVALID_PARAMETER` from `OpenProcess`;
/// - the pid names something this account may not ask about (`ERROR_ACCESS_DENIED`) — a service
///   account's process that was given a pid MixEngine once used. A process this daemon cannot even
///   query is not one it started, and answering "no" here is what keeps the caller from ever
///   signalling it;
/// - the process has ended and its object is still there because somebody holds a handle to it, in
///   which case the exit time is set. Windows keeps such an object openable by pid, so the exit time
///   is the only thing separating "it is running" from "it ran".
///
/// The handle is opened with `PROCESS_QUERY_LIMITED_INFORMATION`, which is the narrowest right that
/// answers the question and — unlike `PROCESS_QUERY_INFORMATION` — is granted for processes at a
/// higher integrity level.
pub(crate) fn started_at(pid: u32) -> Result<Option<i64>> {
    #[expect(
        unsafe_code,
        reason = "OpenProcess takes three integers and returns a handle this function owns and \
                  closes on every path"
    )]
    let process = unsafe { OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, 0, pid) };

    if process.is_null() {
        let error = io::Error::last_os_error();

        return match error.raw_os_error().map(|code| code as u32) {
            Some(ERROR_INVALID_PARAMETER | ERROR_ACCESS_DENIED) => Ok(None),

            _ => Err(Error::Os {
                action: "ask the OS when a process began",
                source: error,
            }),
        };
    }

    let mut created = FILETIME {
        dwLowDateTime: 0,
        dwHighDateTime: 0,
    };
    let mut ended = created;
    let mut kernel = created;
    let mut user = created;

    #[expect(
        unsafe_code,
        reason = "the four pointers are to locals this frame owns and the handle is the one opened \
                  above; the call writes exactly four FILETIMEs and closes nothing"
    )]
    let asked = unsafe {
        GetProcessTimes(
            process,
            &raw mut created,
            &raw mut ended,
            &raw mut kernel,
            &raw mut user,
        )
    };

    let failure = (asked == 0).then(io::Error::last_os_error);

    #[expect(
        unsafe_code,
        reason = "the handle was opened by this function, is not used after this, and is closed \
                  exactly once"
    )]
    unsafe {
        CloseHandle(process);
    }

    if let Some(source) = failure {
        return Err(Error::Os {
            action: "ask the OS when a process began",
            source,
        });
    }

    // A process that is still running has no exit time at all, which is how a handle-kept corpse is
    // told from the process the caller is asking about.
    if ticks(ended) != 0 {
        return Ok(None);
    }

    // A `FILETIME` is under 2^63 for any date this side of the year 30000, so the sign is never in
    // question and the value is stored as it is.
    Ok(Some(ticks(created) as i64))
}

/// The two halves of a `FILETIME` as the one number it is.
fn ticks(time: FILETIME) -> u64 {
    (u64::from(time.dwHighDateTime) << 32) | u64::from(time.dwLowDateTime)
}

/// Stop a process this one did not start, and is not the parent of.
///
/// **This process only, not a group**, and that is the honest half of adoption on Windows: the job
/// object that made a service's group killable belonged to the daemon that created it and went with
/// it. A survivor here exists at all only through the one-call-wide window between `CreateProcessW`
/// and `AssignProcessToJobObject` that
/// `.claude/decisions/0007-supervised-child-owns-a-process-group.md` accepts, so it is a process
/// that had not yet been assigned to anything and, in every case that has ever been reproduced, has
/// not started children of its own either.
///
/// Exit code 1, for the same reason [`Group::terminate`] uses it: a process killed this way must not
/// be read afterwards as one that finished successfully.
///
/// A process that has already gone is success, not failure — the same rule Unix's `ESRCH` gets, and
/// what the caller wanted either way.
pub(crate) fn stop_foreign(pid: u32) -> Result<()> {
    #[expect(
        unsafe_code,
        reason = "OpenProcess takes three integers and returns a handle this function owns and \
                  closes on every path"
    )]
    let process = unsafe { OpenProcess(PROCESS_TERMINATE, 0, pid) };

    if process.is_null() {
        let error = io::Error::last_os_error();

        return match error.raw_os_error().map(|code| code as u32) {
            Some(ERROR_INVALID_PARAMETER) => Ok(()),

            _ => Err(Error::Os {
                action: "stop a process that outlived the daemon supervising it",
                source: error,
            }),
        };
    }

    #[expect(
        unsafe_code,
        reason = "the handle is the one opened above and the exit code is passed by value"
    )]
    let terminated = unsafe { TerminateProcess(process, 1) };

    let failure = (terminated == 0).then(io::Error::last_os_error);

    #[expect(
        unsafe_code,
        reason = "the handle was opened by this function, is not used after this, and is closed \
                  exactly once"
    )]
    unsafe {
        CloseHandle(process);
    }

    match failure {
        None => Ok(()),

        Some(source) => Err(Error::Os {
            action: "stop a process that outlived the daemon supervising it",
            source,
        }),
    }
}

/// There is no way to ask a process with no console to stop; see [`CAN_ASK_TO_STOP`].
///
/// Adoption inherits that answer rather than working around it: a survivor was started with
/// `CREATE_NO_WINDOW` by a daemon that is now gone, so there is even less of a console to reach it
/// through than there was before.
pub(crate) fn ask_foreign_to_stop(_pid: u32) -> Result<()> {
    Err(Error::UnsupportedPlatform {
        capability: "asking a process that outlived its supervisor to stop",
        reason: "Windows has no signal a daemon can send to a process it did not give a console to"
            .to_owned(),
    })
}

/// An all-zero `JOBOBJECT_EXTENDED_LIMIT_INFORMATION`.
///
/// The struct is `#[repr(C)]` and every field is an integer or another such struct — there is no
/// reference, no pointer and no enum in it — so all zeroes is a valid value and means "no limits
/// set", which is what every field this code does not fill in has to say.
#[expect(
    unsafe_code,
    reason = "zeroed is sound for a repr(C) struct of integers, and the alternative is naming ten \
              fields whose only correct value is 0"
)]
fn unsafe_zeroed_because_every_field_is_a_number() -> JOBOBJECT_EXTENDED_LIMIT_INFORMATION {
    unsafe { std::mem::zeroed() }
}

impl Drop for Group {
    /// Close the handle, which is what kills the group.
    ///
    /// Failure is unreportable and uninteresting: a handle this value owns and never hands out
    /// cannot already be closed, and nothing useful is left to do if the kernel disagrees.
    fn drop(&mut self) {
        #[expect(
            unsafe_code,
            reason = "the handle was created by this type, has not been closed before, and is not \
                      used again after this"
        )]
        unsafe {
            CloseHandle(self.job);
        }
    }
}

/// Clear `HANDLE_FLAG_INHERIT` on this process's standard handles, and say which ones that changed.
///
/// The three of them are the whole set worth clearing: everything else MixEngine opens is created
/// non-inheritable (the API pipe says so explicitly, and the standard library's files are), so the
/// only inheritable handles this process can be holding are the ones it was handed.
///
/// Failures are ignored on purpose. A missing standard handle — the normal state of a process that
/// is itself already detached — is not something to report, and a handle type that will not take the
/// call leaves this process no worse off than not having tried. What matters is that a handle only
/// joins the restore list once the call that cleared it has succeeded.
fn stop_handing_on_the_standard_handles() -> Vec<usize> {
    let mut cleared = Vec::new();

    for id in [STD_INPUT_HANDLE, STD_OUTPUT_HANDLE, STD_ERROR_HANDLE] {
        #[expect(
            unsafe_code,
            reason = "GetStdHandle borrows nothing, and Get/SetHandleInformation read and clear a \
                      flag on the handle they are given; none of the three closes anything, and \
                      the only pointer written through is a u32 this frame owns"
        )]
        unsafe {
            let handle: HANDLE = GetStdHandle(id);

            if handle.is_null() || handle == INVALID_HANDLE_VALUE {
                continue;
            }

            // Asked rather than assumed, so the restore below cannot make a handle inheritable that
            // was not: a process launched with `bInheritHandles = FALSE` holds standard handles
            // nobody was ever meant to pass on.
            let mut flags = 0;

            if GetHandleInformation(handle, &mut flags) == 0 || flags & HANDLE_FLAG_INHERIT == 0 {
                continue;
            }

            if SetHandleInformation(handle, HANDLE_FLAG_INHERIT, 0) != 0 {
                cleared.push(handle as usize);
            }
        }
    }

    cleared
}
