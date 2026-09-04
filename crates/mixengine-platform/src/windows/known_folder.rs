//! A system directory asked of the operating system rather than read out of the environment.
//!
//! **The whole of why this exists is who supplies the environment.** `mixengine-elevate` is started
//! by the daemon through the elevation prompt, and whether the process that prompt creates carries
//! the daemon's environment block is not something this workspace establishes — so a binary running
//! as root that resolves `%ProgramFiles%` is resolving a value it cannot show it chose. The design
//! rule for that binary is not *prove it is safe*, it is
//! `.claude/architecture/security-model.md`'s *"validates everything again rather than trusting its
//! caller"*; `SHGetKnownFolderPath` removes the question instead of answering it.
//!
//! Two callers, both about a directory root owns: the audit log's
//! ([`crate::elevated::audit_directory`]) and the one the privileged helper is installed into
//! ([`crate::install::helper_path`]). See the T85 design, D4.

use std::ffi::OsString;
use std::io;
use std::os::windows::ffi::OsStringExt as _;
use std::path::PathBuf;

use windows_sys::Win32::Foundation::S_OK;
use windows_sys::Win32::System::Com::CoTaskMemFree;
use windows_sys::Win32::UI::Shell::{
    FOLDERID_ProgramData, FOLDERID_ProgramFiles, SHGetKnownFolderPath,
};
use windows_sys::core::GUID;

use crate::{Error, Result};

/// `C:\Program Files`, wherever this installation actually put it.
pub(crate) fn program_files() -> Result<PathBuf> {
    resolve(&FOLDERID_ProgramFiles, "locate the Program Files folder")
}

/// `C:\ProgramData`, likewise.
pub(crate) fn program_data() -> Result<PathBuf> {
    resolve(&FOLDERID_ProgramData, "locate the ProgramData folder")
}

/// One call, one wide string, one free.
///
/// Flags of `0` (`KF_FLAG_DEFAULT`) and a null token: the default is the machine's answer, which is
/// what both callers want — neither is asking about a particular user's profile, and a token is the
/// only way to ask about somebody else's.
fn resolve(folder: &GUID, action: &'static str) -> Result<PathBuf> {
    let mut wide: *mut u16 = std::ptr::null_mut();

    #[expect(
        unsafe_code,
        reason = "the out-pointer is a local of this frame, and the buffer it receives is freed \
                  exactly once on both paths below"
    )]
    let status = unsafe { SHGetKnownFolderPath(folder, 0, std::ptr::null_mut(), &raw mut wide) };

    // The documentation is explicit that the buffer must be freed even when the call fails, so the
    // error path frees before it returns rather than after checking the status alone.
    if status != S_OK || wide.is_null() {
        if !wide.is_null() {
            #[expect(
                unsafe_code,
                reason = "allocated by the call above and freed exactly once here"
            )]
            unsafe {
                CoTaskMemFree(wide.cast());
            }
        }

        return Err(Error::Os {
            action,
            source: io::Error::from_raw_os_error(status),
        });
    }

    #[expect(
        unsafe_code,
        reason = "the buffer is the null-terminated wide string the call above allocated, and the \
                  walk stops at its terminator"
    )]
    let rendered = unsafe {
        let mut length = 0;
        while *wide.add(length) != 0 {
            length += 1;
        }

        OsString::from_wide(std::slice::from_raw_parts(wide, length))
    };

    #[expect(
        unsafe_code,
        reason = "allocated by the call above and freed exactly once here"
    )]
    unsafe {
        CoTaskMemFree(wide.cast());
    }

    Ok(PathBuf::from(rendered))
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Both folders resolve, and neither answer is the empty path.
    ///
    /// The shape of the assertion matters more than the value: a `SHGetKnownFolderPath` that fails
    /// hands back a null pointer, and a bug in the length walk above would show up here as an
    /// empty or truncated string rather than as a crash.
    #[test]
    fn the_shell_names_both_folders() {
        let files = program_files().expect("Program Files");
        let data = program_data().expect("ProgramData");

        assert!(files.is_absolute(), "{}", files.display());
        assert!(data.is_absolute(), "{}", data.display());
        assert!(files.as_os_str().len() > 3);
        assert!(data.as_os_str().len() > 3);
    }

    /// D4, as a test: the answer comes from the operating system, so a caller that rewrites the
    /// environment cannot move where a root process is about to write.
    #[test]
    fn the_answer_does_not_follow_the_environment() {
        let before = program_files().expect("Program Files");

        // This is a test process; nothing else in it reads either variable, and both are put back.
        #[expect(
            unsafe_code,
            reason = "std::env::set_var is unsafe in edition 2024; this test is single-threaded \
                      and restores what it changed"
        )]
        unsafe {
            std::env::set_var("ProgramFiles", r"C:\somewhere-an-attacker-owns");
        }

        let after = program_files().expect("Program Files");

        #[expect(
            unsafe_code,
            reason = "the other half of the change above, on the same thread"
        )]
        unsafe {
            std::env::remove_var("ProgramFiles");
        }

        assert_eq!(before, after);
    }
}
