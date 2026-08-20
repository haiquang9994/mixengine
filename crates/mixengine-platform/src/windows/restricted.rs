//! A child created from a restricted copy of this process's token — roadmap task **T34a**.
//!
//! **Why this exists, in one sentence a reader can check:** `postgres` calls `check_root()` at
//! startup, which on Windows asks `pgwin32_is_admin()`, and a process whose token holds
//! `BUILTIN\Administrators` as an *enabled* group is refused with *Execution of PostgreSQL by a
//! user with administrative permissions is not permitted*.
//!
//! An ordinary machine never meets that: an interactive administrator carries a UAC-*filtered*
//! token where the group is present deny-only and grants nothing. The machine that meets it is this
//! repository's own Windows CI leg, which holds a full token deliberately and asserts that it still
//! does (T2b, `.github/workflows/ci.yml`).
//!
//! **So every supervised child is created from a restricted token, not only PostgreSQL's.** The
//! decision and its reasons are
//! `.claude/decisions/0010-supervised-child-never-inherits-administrators.md`; the shortest of them
//! is that on a normal machine this changes nothing, because disabling a group that is already
//! deny-only is a no-op.
//!
//! The two SIDs dropped are the two PostgreSQL's own `src/common/restricted_token.c` drops, for the
//! same reason and in the same order.
//!
//! # Why this is hand-rolled rather than a `Command`
//!
//! [`std::process::Command`] has no way to state a token, and `CreateProcessAsUserW` hands back a
//! raw handle that no [`std::process::Child`] can be built from. What buys the cost is a second
//! thing this gets for free: handles are inherited **explicitly**, through
//! `PROC_THREAD_ATTRIBUTE_HANDLE_LIST`, rather than by setting `bInheritHandles` and hoping — which
//! is the window `hide_stdio_from_children` exists to guard on the paths that still use `Command`.
//!
//! `CreateProcessAsUserW` needs no privilege when the token is a restricted version of the caller's
//! own. That special case is why `initdb` and `pg_ctl` can already do this to themselves, and it is
//! why nothing here goes through `mixengine-elevate`.

use std::collections::BTreeMap;
use std::ffi::{OsStr, OsString};
use std::fs::File;
use std::io;
use std::os::windows::ffi::OsStrExt as _;
use std::os::windows::io::{AsRawHandle as _, FromRawHandle as _, OwnedHandle};
use std::path::Path;

use windows_sys::Win32::Foundation::{
    CloseHandle, HANDLE, HANDLE_FLAG_INHERIT, SetHandleInformation,
};
use windows_sys::Win32::Security::{
    AllocateAndInitializeSid, CreateRestrictedToken, FreeSid, PSID, SECURITY_ATTRIBUTES,
    SECURITY_NT_AUTHORITY, SID_AND_ATTRIBUTES, TOKEN_ALL_ACCESS,
};
use windows_sys::Win32::System::Pipes::CreatePipe;
use windows_sys::Win32::System::SystemServices::{
    DOMAIN_ALIAS_RID_ADMINS, DOMAIN_ALIAS_RID_POWER_USERS, SECURITY_BUILTIN_DOMAIN_RID,
};
use windows_sys::Win32::System::Threading::{
    CREATE_NO_WINDOW, CREATE_UNICODE_ENVIRONMENT, CreateProcessAsUserW,
    DeleteProcThreadAttributeList, EXTENDED_STARTUPINFO_PRESENT, GetCurrentProcess,
    InitializeProcThreadAttributeList, LPPROC_THREAD_ATTRIBUTE_LIST, OpenProcessToken,
    PROC_THREAD_ATTRIBUTE_HANDLE_LIST, PROCESS_INFORMATION, STARTF_USESTDHANDLES, STARTUPINFOEXW,
    UpdateProcThreadAttribute,
};

use super::sid::Token;
use crate::{Error, Result};

// Reading a token back is the assertions' business and nobody else's — see [`groups_of`].
#[cfg(test)]
use super::sid::render;
#[cfg(test)]
use windows_sys::Win32::Security::{GetTokenInformation, TOKEN_GROUPS, TokenGroups};

/// `BUILTIN\Administrators`, the group `pgwin32_is_admin` looks for.
///
/// Named only by the assertions: the token is built from RIDs, and this is how the result is read
/// back. See [`groups_of`].
#[cfg(test)]
const ADMINISTRATORS: &str = "S-1-5-32-544";

/// `BUILTIN\Power Users`, dropped beside it because PostgreSQL drops it.
#[cfg(test)]
const POWER_USERS: &str = "S-1-5-32-547";

/// A child created from a restricted token, and everything the caller owns of it.
#[derive(Debug)]
pub(crate) struct Spawned {
    /// The process itself. Closed when this is dropped, which does **not** end the process.
    pub(crate) process: OwnedHandle,

    /// Its pid, for the job object and for the log line.
    pub(crate) pid: u32,

    /// Its standard output, already ours to read.
    pub(crate) stdout: File,

    /// Its standard error, likewise.
    pub(crate) stderr: File,

    /// Its standard input, for a caller that asked for one — `postgres --single`, and nothing else.
    pub(crate) stdin: Option<File>,
}

/// A restricted copy of this process's token: Administrators and Power Users disabled.
///
/// # Errors
///
/// [`Error::Os`] when the process token cannot be opened or restricted, which means a machine where
/// something has gone very wrong — this is a copy of a token this process already holds.
pub(crate) fn token() -> Result<Token> {
    let original = own_token()?;

    let admins = builtin(DOMAIN_ALIAS_RID_ADMINS)?;
    let power = builtin(DOMAIN_ALIAS_RID_POWER_USERS)?;

    let disable = [
        SID_AND_ATTRIBUTES {
            Sid: admins.0,
            Attributes: 0,
        },
        SID_AND_ATTRIBUTES {
            Sid: power.0,
            Attributes: 0,
        },
    ];

    let mut restricted: HANDLE = std::ptr::null_mut();

    #[expect(
        unsafe_code,
        reason = "the SIDs live in the two guards above and outlive the call; the array is a local \
                  of exactly the length passed; the new token handle is written into a local this \
                  function owns and is wrapped in a guard immediately"
    )]
    let made = unsafe {
        CreateRestrictedToken(
            original.0,
            0,
            2,
            disable.as_ptr(),
            0,
            std::ptr::null(),
            0,
            std::ptr::null(),
            &raw mut restricted,
        )
    };

    if made == 0 {
        return Err(Error::Os {
            action: "restrict a copy of this process's access token",
            source: io::Error::last_os_error(),
        });
    }

    Ok(Token(restricted))
}

/// This process's own access token, unrestricted.
///
/// Split out from [`token`] so that the same spawn can be run with and without the restriction —
/// which is the one experiment that tells a spawn this machine will not perform apart from a token
/// this machine will not grant. See `a_child_from_this_process_own_token_runs`.
///
/// # Errors
///
/// [`Error::Os`] when this process cannot open a token it already holds.
fn own_token() -> Result<Token> {
    let mut opened: HANDLE = std::ptr::null_mut();

    #[expect(
        unsafe_code,
        reason = "GetCurrentProcess returns a pseudo-handle that needs no closing, and the token \
                  handle is written into a local this function owns"
    )]
    let got = unsafe { OpenProcessToken(GetCurrentProcess(), TOKEN_ALL_ACCESS, &raw mut opened) };

    if got == 0 {
        return Err(Error::Os {
            action: "open this process's access token",
            source: io::Error::last_os_error(),
        });
    }

    Ok(Token(opened))
}

/// One of the two `BUILTIN\` SIDs, released when the guard is dropped.
fn builtin(rid: i32) -> Result<Sid> {
    let authority = SECURITY_NT_AUTHORITY;
    let mut sid: PSID = std::ptr::null_mut();

    #[expect(
        unsafe_code,
        reason = "the authority is a local that outlives the call, and the SID it allocates is \
                  written into a local this function owns and is freed by the guard below"
    )]
    let made = unsafe {
        AllocateAndInitializeSid(
            &raw const authority,
            2,
            SECURITY_BUILTIN_DOMAIN_RID.cast_unsigned(),
            rid.cast_unsigned(),
            0,
            0,
            0,
            0,
            0,
            0,
            &raw mut sid,
        )
    };

    if made == 0 {
        return Err(Error::Os {
            action: "name a built-in group",
            source: io::Error::last_os_error(),
        });
    }

    Ok(Sid(sid))
}

/// A SID from `AllocateAndInitializeSid`, freed on drop.
struct Sid(PSID);

impl Drop for Sid {
    #[expect(
        unsafe_code,
        reason = "the SID came from AllocateAndInitializeSid, is owned by this guard, and is freed \
                  exactly once"
    )]
    fn drop(&mut self) {
        unsafe {
            FreeSid(self.0);
        }
    }
}

/// Every group in `token`, as `(SID, attributes)`.
///
/// **For the test that proves the exclusion structurally**, and for nothing else: reading a token is
/// how `.claude/standards/testing.md` says a Windows exclusion must be asserted.
///
/// # Errors
///
/// [`Error::Os`] when the token cannot be queried.
#[cfg(test)]
fn groups_of(token: HANDLE) -> Result<Vec<(String, u32)>> {
    let mut needed: u32 = 0;

    #[expect(
        unsafe_code,
        reason = "GetTokenInformation with a null buffer and a zero length is the documented way \
                  to ask how large the answer is; it writes only to `needed`"
    )]
    unsafe {
        GetTokenInformation(token, TokenGroups, std::ptr::null_mut(), 0, &raw mut needed)
    };

    // `Vec<u64>` for the reason `sid.rs` uses one: the bytes are read back as a `TOKEN_GROUPS`,
    // which contains pointers and wants pointer alignment.
    let mut buffer = vec![0_u64; (needed as usize).div_ceil(size_of::<u64>()).max(1)];

    #[expect(
        unsafe_code,
        reason = "the buffer is at least `needed` bytes long, is aligned for the pointers inside \
                  TOKEN_GROUPS, and outlives every read below"
    )]
    let read = unsafe {
        GetTokenInformation(
            token,
            TokenGroups,
            buffer.as_mut_ptr().cast(),
            needed,
            &raw mut needed,
        )
    };

    if read == 0 {
        return Err(Error::Os {
            action: "read the groups out of an access token",
            source: io::Error::last_os_error(),
        });
    }

    let mut groups = Vec::new();

    #[expect(
        unsafe_code,
        reason = "the call above filled the buffer with exactly a TOKEN_GROUPS followed by \
                  GroupCount entries, and every SID it names lives inside that same buffer"
    )]
    let entries = unsafe {
        let header = buffer.as_ptr().cast::<TOKEN_GROUPS>();
        let count = (*header).GroupCount as usize;
        let first = (&raw const (*header).Groups).cast::<SID_AND_ATTRIBUTES>();

        (0..count)
            .map(|index| {
                let entry = &*first.add(index);

                (entry.Sid, entry.Attributes)
            })
            .collect::<Vec<_>>()
    };

    for (sid, attributes) in entries {
        groups.push((render(sid)?, attributes));
    }

    Ok(groups)
}

/// Start `program` from a restricted token, with both output streams piped.
///
/// Standard input is the null device unless `input` is `Some`, in which case the caller is handed
/// the write end and is the one that has to close it — an end of file is what tells a program its
/// instruction is complete.
///
/// `env` arrives already composed by
/// [`whole_environment`](crate::process::whole_environment): this builds a block out of it and does
/// not add to it.
///
/// # Errors
///
/// [`Error::Os`] when the token, the pipes or the attribute list cannot be made, and [`Error::Io`]
/// naming the program when it cannot be started at all.
pub(crate) fn spawn(
    program: &Path,
    args: &[OsString],
    directory: &Path,
    env: &BTreeMap<String, String>,
    input: Option<()>,
) -> Result<Spawned> {
    spawn_from(&token()?, program, args, directory, env, input)
}

/// [`spawn`], from a token the caller already has.
///
/// The parameter exists for one experiment and is not a way to start a child unrestricted: the only
/// other caller is the test that runs this exact path from the *unrestricted* token, which is what
/// separates "this machine will not perform this spawn" from "this machine will not grant this
/// token what the spawn needs".
fn spawn_from(
    token: &Token,
    program: &Path,
    args: &[OsString],
    directory: &Path,
    env: &BTreeMap<String, String>,
    input: Option<()>,
) -> Result<Spawned> {
    let (out_read, out_write) = pipe()?;
    let (err_read, err_write) = pipe()?;
    let (in_read, in_write) = match input {
        None => (null_device()?, None),
        Some(()) => {
            let (read, write) = pipe()?;

            (read, Some(write))
        }
    };

    // Only these three may cross into the child, and they are named rather than left to
    // `bInheritHandles` — which is process-wide for the length of a spawn and is exactly the window
    // `hide_stdio_from_children` exists to guard on the paths that still use `Command`.
    let inheritable: [HANDLE; 3] = [
        in_read.as_raw_handle().cast(),
        out_write.as_raw_handle().cast(),
        err_write.as_raw_handle().cast(),
    ];

    for handle in inheritable {
        #[expect(
            unsafe_code,
            reason = "each handle is owned by a local in this frame and outlives the call, which \
                      sets a flag and closes nothing"
        )]
        let marked =
            unsafe { SetHandleInformation(handle, HANDLE_FLAG_INHERIT, HANDLE_FLAG_INHERIT) };

        if marked == 0 {
            return Err(Error::Os {
                action: "let a child inherit the pipe it writes to",
                source: io::Error::last_os_error(),
            });
        }
    }

    let mut attributes = AttributeList::naming(&inheritable)?;

    let mut started = STARTUPINFOEXW::default();
    started.StartupInfo.cb = u32::try_from(size_of::<STARTUPINFOEXW>()).unwrap_or(u32::MAX);
    started.StartupInfo.dwFlags = STARTF_USESTDHANDLES;
    started.StartupInfo.hStdInput = inheritable[0];
    started.StartupInfo.hStdOutput = inheritable[1];
    started.StartupInfo.hStdError = inheritable[2];
    started.lpAttributeList = attributes.list;

    let mut line = command_line(program, args);
    let mut block = environment_block(env);
    let directory = wide(directory.as_os_str());
    let mut running = PROCESS_INFORMATION::default();

    #[expect(
        unsafe_code,
        reason = "every buffer passed is a local that outlives the call; the command line is \
                  mutable because CreateProcess may write into it; the handles named in the \
                  attribute list are all owned by this frame"
    )]
    let made = unsafe {
        CreateProcessAsUserW(
            token.0,
            std::ptr::null(),
            line.as_mut_ptr(),
            std::ptr::null(),
            std::ptr::null(),
            1,
            CREATE_UNICODE_ENVIRONMENT | CREATE_NO_WINDOW | EXTENDED_STARTUPINFO_PRESENT,
            block.as_mut_ptr().cast(),
            directory.as_ptr(),
            (&raw const started).cast(),
            &raw mut running,
        )
    };

    if made == 0 {
        return Err(Error::Io {
            action: "start",
            path: program.to_path_buf(),
            source: io::Error::last_os_error(),
        });
    }

    attributes.release();

    // The child holds its own copies now. Ours are dropped by leaving this scope; the thread handle
    // is closed here because nothing ever waits on a thread.
    #[expect(
        unsafe_code,
        reason = "the thread handle was created by the call above, is used nowhere else, and is \
                  closed exactly once"
    )]
    unsafe {
        CloseHandle(running.hThread);
    }

    drop(in_read);
    drop(out_write);
    drop(err_write);

    #[expect(
        unsafe_code,
        reason = "the process handle was created by the call above and is handed to an OwnedHandle \
                  that closes it exactly once"
    )]
    let process = unsafe { OwnedHandle::from_raw_handle(running.hProcess.cast()) };

    Ok(Spawned {
        process,
        pid: running.dwProcessId,
        stdout: File::from(out_read),
        stderr: File::from(err_read),
        stdin: in_write.map(File::from),
    })
}

/// An anonymous pipe, as `(read, write)`.
///
/// Neither end is inheritable here. The one that crosses is marked afterwards, by name, which is
/// what keeps a spawn from handing a child the end this process is reading.
fn pipe() -> Result<(OwnedHandle, OwnedHandle)> {
    let mut read: HANDLE = std::ptr::null_mut();
    let mut write: HANDLE = std::ptr::null_mut();

    let attributes = SECURITY_ATTRIBUTES {
        nLength: u32::try_from(size_of::<SECURITY_ATTRIBUTES>()).unwrap_or(u32::MAX),
        lpSecurityDescriptor: std::ptr::null_mut(),
        bInheritHandle: 0,
    };

    #[expect(
        unsafe_code,
        reason = "the attributes are a local that outlives the call, and both handles are written \
                  into locals this function owns and hands straight to a guard"
    )]
    let made = unsafe { CreatePipe(&raw mut read, &raw mut write, &raw const attributes, 0) };

    if made == 0 {
        return Err(Error::Os {
            action: "make a pipe for a supervised child",
            source: io::Error::last_os_error(),
        });
    }

    #[expect(
        unsafe_code,
        reason = "both handles were created by the call above and are each handed to one \
                  OwnedHandle, which closes them exactly once"
    )]
    let ends = unsafe {
        (
            OwnedHandle::from_raw_handle(read.cast()),
            OwnedHandle::from_raw_handle(write.cast()),
        )
    };

    Ok(ends)
}

/// `NUL`, for the child that was given nothing to read.
///
/// A real handle rather than a null one: a child whose `hStdInput` is null inherits *this process's*
/// standard input, which for a daemon is a console it must never touch and for a test is the
/// harness's own pipe.
fn null_device() -> Result<OwnedHandle> {
    File::open("NUL")
        .map(OwnedHandle::from)
        .map_err(|source| Error::Io {
            action: "open",
            path: std::path::PathBuf::from("NUL"),
            source,
        })
}

/// The command line, quoted the way `CommandLineToArgvW` parses it back.
///
/// `CreateProcess` takes one string where `Command` takes a list, so the quoting the standard
/// library does is this function's to do — see [`append`], which is where the rule is.
fn command_line(program: &Path, args: &[OsString]) -> Vec<u16> {
    let mut line = Vec::new();

    append(program.as_os_str(), &mut line);

    for arg in args {
        line.push(u16::from(b' '));
        append(arg, &mut line);
    }

    line.push(0);
    line
}

/// One argument of [`command_line`], quoted only where it has to be.
///
/// **Only where it has to be, and that is not a stylistic choice.** `cmd.exe /c` removes the outer
/// quotes around what follows only when the whole line carries exactly two of them; a spawn that
/// quoted the program path as well would leave four, and `cmd /c "echo hello"` would then look for
/// a program called `"echo hello`. The standard library quotes on the same condition for the same
/// reason, and a probe that spelled its line differently from the `Command` beside it would be a
/// difference nobody could see until a service failed to start.
///
/// The quoting rule inside is `CommandLineToArgvW`'s: a backslash is an escape only immediately
/// before a quote, so the run of backslashes before one is doubled, and so is the run before the
/// closing quote this adds.
fn append(argument: &OsStr, line: &mut Vec<u16>) {
    const QUOTE: u16 = b'"' as u16;
    const BACKSLASH: u16 = b'\\' as u16;
    const SPACE: u16 = b' ' as u16;
    const TAB: u16 = b'\t' as u16;

    let units: Vec<u16> = argument.encode_wide().collect();
    let needs_quoting = units.is_empty()
        || units
            .iter()
            .any(|unit| matches!(*unit, SPACE | TAB | QUOTE));

    if !needs_quoting {
        line.extend(units);

        return;
    }

    line.push(QUOTE);

    let mut backslashes = 0;

    for unit in units {
        match unit {
            BACKSLASH => backslashes += 1,
            QUOTE => {
                // The run before a quote is doubled, and the quote itself escaped.
                for _ in 0..=backslashes {
                    line.push(BACKSLASH);
                }

                backslashes = 0;
            }
            _ => backslashes = 0,
        }

        line.push(unit);
    }

    // And the run before the closing quote, for the same reason.
    for _ in 0..backslashes {
        line.push(BACKSLASH);
    }

    line.push(QUOTE);
}

/// The environment block: `name=value\0` for each, then a final `\0`.
///
/// Sorted, because [`BTreeMap`] already is and Windows documents the block as being in sort order.
/// A `CREATE_UNICODE_ENVIRONMENT` block that is empty is still two NULs rather than nothing, which
/// is the difference between "no variables" and "inherit the parent's".
fn environment_block(env: &BTreeMap<String, String>) -> Vec<u16> {
    let mut block = Vec::new();

    for (name, value) in env {
        block.extend(OsStr::new(name).encode_wide());
        block.push(u16::from(b'='));
        block.extend(OsStr::new(value).encode_wide());
        block.push(0);
    }

    block.push(0);
    block
}

/// A NUL-terminated wide string, for the arguments that are read rather than written.
fn wide(text: &OsStr) -> Vec<u16> {
    text.encode_wide().chain(std::iter::once(0)).collect()
}

/// The attribute list naming exactly the handles that may be inherited.
///
/// A guard, because `InitializeProcThreadAttributeList` allocates into a buffer that has to be
/// deleted whichever way the spawn ends — and after a *successful* `CreateProcess` the list has been
/// consumed, which is what [`release`](Self::release) records.
struct AttributeList {
    list: LPPROC_THREAD_ATTRIBUTE_LIST,
    buffer: Vec<u8>,
    live: bool,
}

impl AttributeList {
    /// One attribute: the handle list.
    fn naming(handles: &[HANDLE]) -> Result<Self> {
        let mut size: usize = 0;

        #[expect(
            unsafe_code,
            reason = "a null list with a zero size is the documented way to ask how large the \
                      buffer has to be; it writes only to `size`"
        )]
        unsafe {
            InitializeProcThreadAttributeList(std::ptr::null_mut(), 1, 0, &raw mut size)
        };

        let mut buffer = vec![0_u8; size];
        let list: LPPROC_THREAD_ATTRIBUTE_LIST = buffer.as_mut_ptr().cast();

        #[expect(
            unsafe_code,
            reason = "the buffer is exactly the length the call above asked for and outlives this \
                      value, which owns it"
        )]
        let made = unsafe { InitializeProcThreadAttributeList(list, 1, 0, &raw mut size) };

        if made == 0 {
            return Err(Error::Os {
                action: "make the list of handles a child may inherit",
                source: io::Error::last_os_error(),
            });
        }

        let attributes = Self {
            list,
            buffer,
            live: true,
        };

        #[expect(
            unsafe_code,
            reason = "the handles are owned by the caller's frame, which outlives both this value \
                      and the spawn that reads the list"
        )]
        let updated = unsafe {
            UpdateProcThreadAttribute(
                attributes.list,
                0,
                PROC_THREAD_ATTRIBUTE_HANDLE_LIST as usize,
                handles.as_ptr().cast(),
                size_of_val(handles),
                std::ptr::null_mut(),
                std::ptr::null(),
            )
        };

        if updated == 0 {
            return Err(Error::Os {
                action: "name the handles a child may inherit",
                source: io::Error::last_os_error(),
            });
        }

        Ok(attributes)
    }

    /// The spawn succeeded and the list has been consumed; delete it now rather than on drop.
    fn release(&mut self) {
        if !self.live {
            return;
        }

        self.live = false;

        #[expect(
            unsafe_code,
            reason = "the list was initialised by this value, is deleted exactly once, and its \
                      buffer is still alive"
        )]
        unsafe {
            DeleteProcThreadAttributeList(self.list);
        }
    }
}

impl Drop for AttributeList {
    fn drop(&mut self) {
        self.release();

        // Named so a reader does not think the buffer is unused: it is what `list` points into.
        let _ = &self.buffer;
    }
}

#[cfg(test)]
mod tests {
    use windows_sys::Win32::System::SystemServices::SE_GROUP_ENABLED;

    use super::*;

    /// **The assertion T34a exists to make**, and it is made by reading rather than by trying.
    ///
    /// `.claude/standards/testing.md` says why: this repository's Windows CI leg holds a *full*
    /// token where `BUILTIN\Administrators` is an enabled group, so a test that proved the exclusion
    /// by attempting an access would pass on a developer's filtered token and prove nothing at all
    /// on the runner — the one machine where it matters. What is asserted instead is the token's own
    /// contents, which mean the same thing on both.
    ///
    /// Present **and** disabled, not absent: `CreateRestrictedToken` leaves a disabled SID in the
    /// group list marked `SE_GROUP_USE_FOR_DENY_ONLY`, which is exactly what a UAC-filtered token
    /// looks like and exactly what `pgwin32_is_admin` answers no to.
    #[test]
    fn a_restricted_token_holds_administrators_and_grants_nothing_through_it() {
        let token = token().expect("this process can restrict a copy of its own token");
        let groups = groups_of(token.0).expect("a token this process made can be read");

        let administrators = groups
            .iter()
            .find(|(sid, _)| sid == ADMINISTRATORS)
            .unwrap_or_else(|| {
                panic!("{ADMINISTRATORS} is not in the restricted token at all: {groups:?}")
            });

        assert_eq!(
            administrators.1 & SE_GROUP_ENABLED.cast_unsigned(),
            0,
            "Administrators is still enabled in a token that was supposed to disable it: {groups:?}"
        );
    }

    /// And the same for Power Users, which PostgreSQL's own `restricted_token.c` drops beside it.
    ///
    /// Absent is the ordinary case: Power Users has been an empty group since Vista, so most
    /// machines carry no such membership to disable. Its presence is what is asserted about, not its
    /// existence.
    #[test]
    fn a_restricted_token_disables_power_users_too() {
        let token = token().expect("this process can restrict a copy of its own token");
        let groups = groups_of(token.0).expect("a token this process made can be read");

        if let Some((_, attributes)) = groups.iter().find(|(sid, _)| sid == POWER_USERS) {
            assert_eq!(
                *attributes & SE_GROUP_ENABLED.cast_unsigned(),
                0,
                "{groups:?}"
            );
        }
    }

    /// How a child ended, for a failure that has to say more than "it printed nothing".
    ///
    /// `0xC0000142` is `STATUS_DLL_INIT_FAILED`: the process was created and died before its first
    /// instruction, which is what a token denied the window station looks like from out here.
    fn ended(spawned: &Spawned) -> u32 {
        use windows_sys::Win32::Foundation::WAIT_OBJECT_0;
        use windows_sys::Win32::System::Threading::{
            GetExitCodeProcess, INFINITE, WaitForSingleObject,
        };

        #[expect(
            unsafe_code,
            reason = "the handle is owned by the caller's `Spawned`, which outlives this call, and \
                      neither call closes it"
        )]
        unsafe {
            let handle: HANDLE = spawned.process.as_raw_handle().cast();
            assert_eq!(WaitForSingleObject(handle, INFINITE), WAIT_OBJECT_0);

            let mut code: u32 = 0;
            assert_ne!(GetExitCodeProcess(handle, &raw mut code), 0);

            code
        }
    }

    /// The window station and desktop this process is on, which is what a child inherits.
    ///
    /// **The question this answers**: an interactive session is `WinSta0`, and a service session is
    /// `Service-0x0-3e7$` — whose DACL grants `SYSTEM` and `Administrators` rather than a logon SID.
    /// A token with Administrators deny-only loses access to the second and keeps the first, and a
    /// child that cannot reach its station dies exactly where these tests find it.
    fn station() -> String {
        use windows_sys::Win32::System::StationsAndDesktops::{
            GetProcessWindowStation, GetUserObjectInformationW, UOI_NAME,
        };

        let mut name = [0_u16; 256];
        let mut needed: u32 = 0;

        #[expect(
            unsafe_code,
            reason = "the station handle belongs to this process and is not closed here, and the \
                      buffer is a local of exactly the length passed"
        )]
        let read = unsafe {
            GetUserObjectInformationW(
                GetProcessWindowStation().cast(),
                UOI_NAME,
                name.as_mut_ptr().cast(),
                u32::try_from(size_of_val(&name)).unwrap_or(u32::MAX),
                &raw mut needed,
            )
        };

        if read == 0 {
            return format!("<unreadable: {}>", io::Error::last_os_error());
        }

        String::from_utf16_lossy(&name)
            .trim_end_matches('\0')
            .to_owned()
    }

    /// **The experiment that separates the two explanations**, and it is why `spawn_from` takes a
    /// token at all.
    ///
    /// If this passes where [`a_restricted_child_runs_and_is_read_back`] fails, then every part of
    /// this module's process creation is right on that machine and the only thing it will not
    /// accept is the *token* — which means the restricted token is being denied something the
    /// child needs, and the window station is the documented candidate. If it fails too, the
    /// explanation is in the spawn rather than in the token, and the token is a red herring.
    #[test]
    fn a_child_from_this_process_own_token_runs() {
        use std::io::Read as _;

        let shell =
            std::path::PathBuf::from(std::env::var_os("COMSPEC").expect("Windows has a shell"));

        let mut spawned = spawn_from(
            &own_token().expect("this process can open its own token"),
            &shell,
            &["/c".into(), "echo unrestricted".into()],
            &std::env::temp_dir(),
            &crate::process::whole_environment(&BTreeMap::new()),
            None,
        )
        .expect("a child from this process's own token can be created");

        let mut said = String::new();
        spawned
            .stdout
            .read_to_string(&mut said)
            .expect("its stdout is readable");

        assert!(
            said.contains("unrestricted"),
            "a child from this process's *own* token printed nothing: {said:?} \
             exit=0x{:08X} station={}",
            ended(&spawned),
            station()
        );
    }

    /// A child really is created from that token, and says what it was asked to say.
    #[test]
    fn a_restricted_child_runs_and_is_read_back() {
        use std::io::Read as _;

        let shell =
            std::path::PathBuf::from(std::env::var_os("COMSPEC").expect("Windows has a shell"));

        let mut spawned = spawn(
            &shell,
            &["/c".into(), "echo restricted".into()],
            &std::env::temp_dir(),
            // The floor a real spawn composes: a shell handed a wholly empty environment cannot
            // find its own system directory and says so instead of doing what it was asked.
            &crate::process::whole_environment(&BTreeMap::new()),
            None,
        )
        .expect("a restricted child can be created from this process's own token");

        let mut said = String::new();
        spawned
            .stdout
            .read_to_string(&mut said)
            .expect("its stdout is readable");

        let mut complained = String::new();
        spawned
            .stderr
            .read_to_string(&mut complained)
            .expect("its stderr is readable");

        assert!(
            said.contains("restricted"),
            "{said:?} {complained:?} exit=0x{:08X} station={}",
            ended(&spawned),
            station()
        );
    }

    /// And it can be handed something to read, which is the ritual's second step.
    ///
    /// `postgres --single` takes its `ALTER ROLE` there and opens no port and no socket while it
    /// does, which is the whole reason a superuser password never has to touch disk.
    #[test]
    fn a_restricted_child_reads_what_it_was_given() {
        use std::io::{Read as _, Write as _};

        let shell =
            std::path::PathBuf::from(std::env::var_os("COMSPEC").expect("Windows has a shell"));

        let mut spawned = spawn(
            &shell,
            &["/c".into(), "more".into()],
            &std::env::temp_dir(),
            &crate::process::whole_environment(&BTreeMap::new()),
            Some(()),
        )
        .expect("a restricted child can be given something to read");

        let mut writing = spawned.stdin.take().expect("it was given a pipe");
        writing
            .write_all(b"mixengine\r\n")
            .expect("its stdin is writable");
        drop(writing);

        let mut said = String::new();
        spawned
            .stdout
            .read_to_string(&mut said)
            .expect("its stdout is readable");

        assert!(
            said.contains("mixengine"),
            "{said:?} exit=0x{:08X} station={}",
            ended(&spawned),
            station()
        );
    }
}
