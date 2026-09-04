//! Windows: Smart App Control's own policy value — roadmap task **T94**.
//!
//! **The registry rather than `Get-MpComputerStatus`** (T94 design, D1).
//! `.claude/architecture/platform-abstraction.md` rule 5 asks for the API where there is one, and
//! the alternative here is not an API — it is spawning PowerShell and parsing a localised object.
//! And this value is the one with evidence at both ends of its range: it read `1` on a developer
//! machine with Smart App Control enforcing on 2026-08-13, and `0` on the same machine on
//! 2026-09-04 after it had been turned off, with `SAC_PreviousState = 1` beside it.

use windows_sys::Win32::Foundation::{ERROR_FILE_NOT_FOUND, ERROR_SUCCESS};
use windows_sys::Win32::System::Registry::{
    HKEY, HKEY_LOCAL_MACHINE, KEY_QUERY_VALUE, REG_DWORD, REG_VALUE_TYPE, RegCloseKey,
    RegOpenKeyExW, RegQueryValueExW,
};

use crate::{AppControl, AppControlState, Error, Result};

/// Where Code Integrity keeps the policy state.
const POLICY_KEY: &str = r"SYSTEM\CurrentControlSet\Control\CI\Policy";

/// The value inside it. Spelled as Windows spells it; the lookup is case-insensitive regardless.
const POLICY_VALUE: &str = "VerifiedAndReputablePolicyState";

/// This system's answer.
#[derive(Debug, Default)]
pub(crate) struct Policy;

impl AppControl for Policy {
    fn state(&self) -> Result<AppControlState> {
        Ok(AppControlState::from_policy_value(read_policy_value()?))
    }
}

/// The value, or [`None`] when the key or the value is not there.
///
/// **An absent key is not a failure.** Smart App Control arrived in Windows 11 22H2; on everything
/// older there is nothing to read, and D5 says that reads as `Off` rather than as an error.
fn read_policy_value() -> Result<Option<u32>> {
    let Some(key) = open()? else {
        return Ok(None);
    };

    let name = wide(POLICY_VALUE);
    let mut kind: REG_VALUE_TYPE = 0;
    let mut value: u32 = 0;
    let mut bytes: u32 = u32::try_from(size_of::<u32>()).unwrap_or(4);

    #[expect(
        unsafe_code,
        reason = "the registry has no safe binding in this tree; the call writes only the three \
                  out-parameters below, all owned by this frame"
    )]
    let status = unsafe {
        RegQueryValueExW(
            key.0,
            name.as_ptr(),
            std::ptr::null(),
            &raw mut kind,
            (&raw mut value).cast::<u8>(),
            &raw mut bytes,
        )
    };

    if status == ERROR_FILE_NOT_FOUND {
        return Ok(None);
    }
    if status != ERROR_SUCCESS {
        return Err(os(
            "read this machine's Smart App Control policy state",
            status,
        ));
    }

    // A value of another type is not this one. Reading four bytes out of a string and calling it a
    // policy state would be a guess with a number on it.
    if kind != REG_DWORD {
        return Ok(None);
    }

    Ok(Some(value))
}

/// The policy key, opened for reading, or [`None`] when this machine has none.
fn open() -> Result<Option<Key>> {
    let path = wide(POLICY_KEY);
    let mut handle: HKEY = std::ptr::null_mut();

    #[expect(
        unsafe_code,
        reason = "the registry has no safe binding in this tree; the call writes only `handle`"
    )]
    let status = unsafe {
        RegOpenKeyExW(
            HKEY_LOCAL_MACHINE,
            path.as_ptr(),
            0,
            KEY_QUERY_VALUE,
            &raw mut handle,
        )
    };

    if status == ERROR_FILE_NOT_FOUND {
        return Ok(None);
    }
    if status != ERROR_SUCCESS {
        return Err(os("open this machine's Code Integrity policy key", status));
    }

    Ok(Some(Key(handle)))
}

/// An open key that closes itself.
#[derive(Debug)]
struct Key(HKEY);

impl Drop for Key {
    fn drop(&mut self) {
        #[expect(
            unsafe_code,
            reason = "closing a handle this type owns; there is no safe binding for it"
        )]
        unsafe {
            RegCloseKey(self.0);
        }
    }
}

/// What a status this module could not use means to its caller.
fn os(action: &'static str, status: u32) -> Error {
    Error::Os {
        action,
        source: std::io::Error::from_raw_os_error(i32::try_from(status).unwrap_or(i32::MAX)),
    }
}

/// A NUL-terminated wide string, which is what every `W` entry point wants.
fn wide(value: &str) -> Vec<u16> {
    use std::os::windows::ffi::OsStrExt as _;

    std::ffi::OsStr::new(value)
        .encode_wide()
        .chain(std::iter::once(0))
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    /// **An assertion about the code and not about the runner.** Whatever this machine's Smart App
    /// Control is doing, the read answers rather than failing — a CI runner with no such key and a
    /// developer machine with one are both a pass.
    #[test]
    fn this_machine_answers_the_question() {
        assert!(Policy.state().is_ok());
    }
}
