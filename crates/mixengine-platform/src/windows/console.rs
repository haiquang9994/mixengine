//! Letting go of a console Windows created for this process and nobody is reading.
//!
//! # The measurement
//!
//! Windows 11 Pro 26200, 2026-09-04. A console program started by Task Scheduler under
//! `InteractiveToken` found **1** process attached to its console — itself — and that console's
//! window reported `IsWindowVisible() == true`, in the user's own session. The same program started
//! from a shell (`cmd` → `powershell`) found **4**. A console this process is the only member of is
//! one Windows made because a console-subsystem program has to have one; a console it shares is a
//! terminal somebody is looking at.
//!
//! `GetConsoleWindow()` alone is **not** the discriminator, measured in the same run: from the shell
//! it returned `0`, because a ConPTY console has no window of its own. The process count is what
//! separates the two cases, and it is the only thing tested here.
//!
//! # Why the standard handles are redirected first
//!
//! `FreeConsole` invalidates them. A daemon whose every write to stderr fails afterwards is a worse
//! bug than the window this removes, so the null device is opened and `SetStdHandle` points all
//! three at it *before* the console is let go.
//!
//! # What is left
//!
//! A flash. The window exists from process creation until this call, which is tens of milliseconds.
//! That is the price of `mixengined` staying a console program a terminal can run — the alternatives
//! are a windows-subsystem binary that prints nothing when a person runs it, and a fourth binary in
//! every artifact. See the T85b design, D4.

use std::ffi::OsStr;
use std::os::windows::ffi::OsStrExt as _;

use windows_sys::Win32::Foundation::{GENERIC_READ, GENERIC_WRITE, INVALID_HANDLE_VALUE};
use windows_sys::Win32::Storage::FileSystem::{
    CreateFileW, FILE_ATTRIBUTE_NORMAL, FILE_SHARE_READ, FILE_SHARE_WRITE, OPEN_EXISTING,
};
use windows_sys::Win32::System::Console::{
    FreeConsole, GetConsoleProcessList, GetConsoleWindow, STD_ERROR_HANDLE, STD_INPUT_HANDLE,
    STD_OUTPUT_HANDLE, SetStdHandle,
};

/// [`crate::process::release_unattended_console`] on this system.
pub(crate) fn release_unattended() -> bool {
    #[expect(
        unsafe_code,
        reason = "GetConsoleWindow takes no arguments and returns a borrowed handle"
    )]
    let window = unsafe { GetConsoleWindow() };

    // No console at all: a `--detach`ed daemon, started with `DETACHED_PROCESS`, and every child
    // `spawn_detached` makes. Nothing to let go of.
    if window.is_null() {
        return false;
    }

    if !alone() {
        return false;
    }

    redirect_to_null();

    #[expect(
        unsafe_code,
        reason = "FreeConsole takes no arguments; the standard handles no longer name the console"
    )]
    let freed = unsafe { FreeConsole() };

    freed != 0
}

/// Whether this process is the only one attached to its console.
///
/// The buffer holds two ids because that is all the answer needs: the question is "exactly one or
/// more than one", and `GetConsoleProcessList` returns the *total* count whether or not it fitted.
fn alone() -> bool {
    let mut attached = [0_u32; 2];

    #[expect(
        unsafe_code,
        reason = "`attached` is a local array and its length is what is passed"
    )]
    let count = unsafe {
        GetConsoleProcessList(
            attached.as_mut_ptr(),
            u32::try_from(attached.len()).expect("2 fits in a u32"),
        )
    };

    count == 1
}

/// Point standard input, output and error at the null device.
///
/// Best effort in every direction: a handle that cannot be opened, or one Windows will not accept,
/// leaves that stream naming a console that is about to go away — which is what would have happened
/// anyway, and is not worth failing a daemon's start over.
fn redirect_to_null() {
    let name: Vec<u16> = OsStr::new(r"\\.\NUL")
        .encode_wide()
        .chain(std::iter::once(0))
        .collect();

    #[expect(
        unsafe_code,
        reason = "`name` is a NUL-terminated local buffer and outlives the call"
    )]
    let null = unsafe {
        CreateFileW(
            name.as_ptr(),
            GENERIC_READ | GENERIC_WRITE,
            FILE_SHARE_READ | FILE_SHARE_WRITE,
            std::ptr::null(),
            OPEN_EXISTING,
            FILE_ATTRIBUTE_NORMAL,
            std::ptr::null_mut(),
        )
    };

    if null == INVALID_HANDLE_VALUE || null.is_null() {
        return;
    }

    for stream in [STD_INPUT_HANDLE, STD_OUTPUT_HANDLE, STD_ERROR_HANDLE] {
        #[expect(
            unsafe_code,
            reason = "`null` is an open handle this function owns; SetStdHandle duplicates nothing"
        )]
        unsafe {
            SetStdHandle(stream, null);
        }
    }

    // **And it is deliberately not closed.** `SetStdHandle` stores the value and duplicates
    // nothing, so closing this would leave all three standard handles naming a file that is gone —
    // which is the bug this function exists to avoid, arrived at from the other side. The process
    // holds one open handle to the null device for as long as it runs, which is what every daemon
    // that redirects its own streams does.
}
