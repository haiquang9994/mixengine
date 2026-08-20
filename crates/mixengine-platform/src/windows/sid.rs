//! Reading an account's SID out of an access token.
//!
//! Two callers, both in `ipc.rs`: the pipe's DACL has to name this account, and the peer check has
//! to name whoever connected. Both want the SID in its string form (`S-1-5-21-…`), which is what
//! SDDL takes and what a log line can print, and neither ever wants the display name — that is
//! localised, it can be changed, and two accounts on two machines can share one.
//!
//! `windows/access.rs` answers the same question a different way, by parsing `whoami /user`. That
//! is not an oversight to be tidied away here: it is a verified path with tests behind it, and T47
//! owns the decision to revisit the whole Windows ACL implementation, this included. What must not
//! happen is a third way.

use std::io;

use windows_sys::Win32::Foundation::{CloseHandle, HANDLE, LocalFree};
use windows_sys::Win32::Security::Authorization::ConvertSidToStringSidW;
use windows_sys::Win32::Security::{GetTokenInformation, PSID, TOKEN_QUERY, TOKEN_USER, TokenUser};
use windows_sys::Win32::System::Threading::{GetCurrentProcess, OpenProcessToken};

use crate::{Error, Result};

/// The SID of the account this process runs as.
///
/// # Errors
///
/// [`Error::Os`] if the process token cannot be opened or read, which in practice means a machine
/// where something has gone very wrong.
pub(crate) fn current_user() -> Result<String> {
    let token = open_process_token()?;

    // The guard is held until the end of the call, not dropped at the semicolon of a `let`: the
    // handle has to outlive the read that goes through it.
    of_token(token.0)
}

/// The SID the given access token belongs to.
///
/// The handle stays the caller's to close; this only reads through it.
///
/// # Errors
///
/// [`Error::Os`] when the token cannot be queried — an anonymous impersonation token, which names
/// no user at all, fails here rather than being reported as somebody.
pub(crate) fn of_token(token: HANDLE) -> Result<String> {
    // `TOKEN_USER` is a header plus a SID whose length depends on the account, so the size has to
    // be asked for. The first call is expected to fail; only the size it writes back is of
    // interest, which is why its return value is ignored rather than checked.
    let mut needed: u32 = 0;
    #[expect(
        unsafe_code,
        reason = "GetTokenInformation with a null buffer and a zero length is the documented way \
                  to ask how large the answer is; it writes only to `needed`"
    )]
    unsafe {
        GetTokenInformation(token, TokenUser, std::ptr::null_mut(), 0, &raw mut needed)
    };

    // `Vec<u64>` rather than `Vec<u8>`: the bytes are read back as a `TOKEN_USER`, which contains a
    // pointer and therefore wants pointer alignment. A byte vector guarantees an alignment of one,
    // and "malloc happens to return something aligned" is not a guarantee to build on.
    let mut buffer = vec![0_u64; (needed as usize).div_ceil(size_of::<u64>()).max(1)];

    #[expect(
        unsafe_code,
        reason = "the buffer is at least `needed` bytes long, is aligned for the pointer inside \
                  TOKEN_USER, and outlives every read below"
    )]
    let read = unsafe {
        GetTokenInformation(
            token,
            TokenUser,
            buffer.as_mut_ptr().cast(),
            needed,
            &raw mut needed,
        )
    };

    if read == 0 {
        return Err(Error::Os {
            action: "read the user out of an access token",
            source: io::Error::last_os_error(),
        });
    }

    #[expect(
        unsafe_code,
        reason = "the call above filled the buffer with exactly a TOKEN_USER, and the SID it \
                  points at lives inside that same buffer"
    )]
    let sid = unsafe { (*buffer.as_ptr().cast::<TOKEN_USER>()).User.Sid };

    render(sid)
}

/// A SID, rendered as `S-1-5-…`.
pub(crate) fn render(sid: PSID) -> Result<String> {
    let mut text = std::ptr::null_mut();

    #[expect(
        unsafe_code,
        reason = "`sid` came from the OS and is valid for this call; the string it allocates is \
                  released below on both paths"
    )]
    let converted = unsafe { ConvertSidToStringSidW(sid, &raw mut text) };

    if converted == 0 {
        return Err(Error::Os {
            action: "render a SID",
            source: io::Error::last_os_error(),
        });
    }

    #[expect(
        unsafe_code,
        reason = "ConvertSidToStringSidW hands back a NUL-terminated wide string it allocated, \
                  which is what both of these read and then release"
    )]
    let sid = unsafe {
        let mut units = Vec::new();
        let mut cursor = text;

        while *cursor != 0 {
            units.push(*cursor);
            cursor = cursor.add(1);
        }

        // Before any early return: the allocation is `LocalAlloc`'d and is ours to free from here
        // on, and a SID is ASCII, so nothing between the two lines can fail.
        LocalFree(text.cast());

        String::from_utf16_lossy(&units)
    };

    Ok(sid)
}

/// This process's access token, closed when the returned guard is dropped.
fn open_process_token() -> Result<Token> {
    let mut token: HANDLE = std::ptr::null_mut();

    #[expect(
        unsafe_code,
        reason = "GetCurrentProcess returns a pseudo-handle that needs no closing, and the token \
                  handle is written into a local this function owns"
    )]
    let opened = unsafe { OpenProcessToken(GetCurrentProcess(), TOKEN_QUERY, &raw mut token) };

    if opened == 0 {
        return Err(Error::Os {
            action: "open this process's access token",
            source: io::Error::last_os_error(),
        });
    }

    Ok(Token(token))
}

/// An open token handle, closed on drop.
///
/// A guard rather than a `CloseHandle` at each exit: every function here has at least two failure
/// paths, and a leaked token handle is the kind of leak that shows up as a daemon that has been
/// running for a week.
pub(crate) struct Token(pub(crate) HANDLE);

impl Drop for Token {
    #[expect(
        unsafe_code,
        reason = "the handle came from OpenProcessToken/OpenThreadToken, is owned by this guard, \
                  and is closed exactly once"
    )]
    fn drop(&mut self) {
        unsafe {
            CloseHandle(self.0);
        }
    }
}

impl std::fmt::Debug for Token {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("Token(<handle>)")
    }
}
