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

use std::os::windows::process::CommandExt as _;
use std::process::Command;

use windows_sys::Win32::Foundation::{
    GetHandleInformation, HANDLE, HANDLE_FLAG_INHERIT, INVALID_HANDLE_VALUE, SetHandleInformation,
};
use windows_sys::Win32::System::Console::{
    GetStdHandle, STD_ERROR_HANDLE, STD_INPUT_HANDLE, STD_OUTPUT_HANDLE,
};
use windows_sys::Win32::System::Threading::{CREATE_NEW_PROCESS_GROUP, DETACHED_PROCESS};

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
