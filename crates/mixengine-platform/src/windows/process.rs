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
use std::process::{Child, Command};

use windows_sys::Win32::Foundation::{
    CloseHandle, GetHandleInformation, HANDLE, HANDLE_FLAG_INHERIT, INVALID_HANDLE_VALUE,
    SetHandleInformation,
};
use windows_sys::Win32::System::Console::{
    GetStdHandle, STD_ERROR_HANDLE, STD_INPUT_HANDLE, STD_OUTPUT_HANDLE,
};
use windows_sys::Win32::System::JobObjects::{
    AssignProcessToJobObject, CreateJobObjectW, JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE,
    JOBOBJECT_EXTENDED_LIMIT_INFORMATION, JobObjectExtendedLimitInformation,
    SetInformationJobObject, TerminateJobObject,
};
use windows_sys::Win32::System::Threading::{
    CREATE_NEW_PROCESS_GROUP, CREATE_NO_WINDOW, DETACHED_PROCESS,
};

use crate::{Error, Result};

/// The standard handles whose inheritance was turned off for one spawn, waiting to be turned back
/// on.
///
/// Held by [`detach`]'s caller across the spawn and dropped straight after it. Restoring in `Drop`
/// rather than in a function the caller has to remember is the point: a spawn that fails must put
/// the flags back too, and there is no path out of `spawn_detached` that skips a drop.
#[derive(Debug)]
pub(crate) struct Detaching {
    /// Only the handles this actually changed. A handle that was already non-inheritable is left
    /// out, so nothing here can hand out an inheritance the process did not have to begin with.
    restore: Vec<HANDLE>,
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
pub(crate) fn hide_stdio() -> Detaching {
    Detaching {
        restore: stop_handing_on_the_standard_handles(),
    }
}

impl Drop for Detaching {
    fn drop(&mut self) {
        for &handle in &self.restore {
            #[expect(
                unsafe_code,
                reason = "SetHandleInformation only sets a flag on the handle it is given, which is \
                          one this process fetched from GetStdHandle and has not closed"
            )]
            unsafe {
                SetHandleInformation(handle, HANDLE_FLAG_INHERIT, HANDLE_FLAG_INHERIT);
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
/// console cannot send one. Stopping a service politely on Windows is roadmap task T15's question
/// and it should be answered there rather than pre-empted by a flag set here.
///
/// A free function taking no group, because on this system the group is joined after the spawn and
/// on the other one it is joined by the child itself — neither has anything to say at this point
/// beyond what goes on the `Command`.
pub(crate) fn arrange(command: &mut Command) {
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

impl Group {
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
fn stop_handing_on_the_standard_handles() -> Vec<HANDLE> {
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
                cleared.push(handle);
            }
        }
    }

    cleared
}
