//! `0700`, plus the ACL that the mode does not cover.
//!
//! macOS ACLs are NFSv4-style and sit *beside* the mode rather than under it. `chmod 0700` leaves an
//! ACE granting another user in place and still working — verified on macOS 15: a directory carrying
//! `group:everyone allow list,search` reports `drwx------+` afterwards and everyone can still list it.
//! That is the same hole `/reset` closes on Windows, and this is where it closes here. Linux is
//! unaffected: a POSIX ACL grants nothing past the group class, which `chmod` sets to zero.
//!
//! The mode half is [`crate::unix::access`], unchanged and shared with Linux; only the ACL is new.
//!
//! Unlike the Windows implementation this calls the C API rather than the command-line tool, because
//! the two jobs are not comparable. Building a Windows DACL means hand-computing ACL sizes behind
//! raw pointers, where a mistake yields a *wrong ACL*; here nothing is built. One call empties the
//! ACL and one asks whether there is one, both with an opaque handle this module never inspects.
//! Text output would have to be parsed instead, and `ls -le` has no format anyone promised to keep.

use std::ffi::{CString, c_char, c_int, c_void};
use std::os::unix::ffi::OsStrExt as _;
use std::path::Path;

use crate::unix::access::Access as Mode;
use crate::{DirectoryAccess, Error, Result};

/// The only ACL kind macOS has. `ACL_TYPE_ACCESS` and `ACL_TYPE_DEFAULT` are declared in
/// `sys/acl.h` for POSIX source compatibility and are rejected at runtime.
const ACL_TYPE_EXTENDED: u32 = 0x0000_0100;

/// How `acl_get_file` says "this directory has no ACL". Spelled out rather than taken from `libc`,
/// which this crate does not depend on: the value is fixed by the Darwin ABI and by POSIX before it.
const ENOENT: i32 = 2;

// From `sys/acl.h`, part of libSystem, so nothing has to be linked explicitly. `acl_t` is opaque by
// specification — it is only ever passed back, never dereferenced, so `*mut c_void` is its full and
// honest shape here.
#[expect(
    unsafe_code,
    reason = "sys/acl.h has no Rust binding: the libc crate does not declare the acl_* family on \
              Apple targets, and the crates that wrap it have been frozen on winapi-era \
              dependencies since 2021"
)]
unsafe extern "C" {
    /// The ACL of `path`, or null with `errno == ENOENT` when it has none.
    fn acl_get_file(path: *const c_char, acl_type: u32) -> *mut c_void;

    /// An empty ACL with room for `count` entries. Null on allocation failure.
    fn acl_init(count: c_int) -> *mut c_void;

    /// Replace the ACL of `path`. `0` on success, `-1` with `errno` set.
    fn acl_set_file(path: *const c_char, acl_type: u32, acl: *mut c_void) -> c_int;

    /// Release what `acl_get_file` or `acl_init` returned.
    fn acl_free(obj: *mut c_void) -> c_int;
}

#[derive(Debug, Default)]
pub(crate) struct Access {
    mode: Mode,
}

impl DirectoryAccess for Access {
    fn restrict_to_owner(&self, path: &Path) -> Result<()> {
        // Mode first: it is the wider of the two restrictions, so the window in which the directory
        // is only half protected is the one where a leftover ACE still applies, not the one where
        // group and other still do.
        self.mode.restrict_to_owner(path)?;
        clear_acl(path)
    }

    fn is_restricted_to_owner(&self, path: &Path) -> Result<bool> {
        // Also the existence check: a missing path has to be reported rather than called
        // unrestricted, and `acl_get_file` cannot tell the caller apart — it answers null with
        // `ENOENT` both for a directory that has no ACL and for one that is not there.
        if !self.mode.is_restricted_to_owner(path)? {
            return Ok(false);
        }

        Ok(!has_acl(path)?)
    }
}

/// Drop every ACE, the way `chmod -N` does.
///
/// `acl_delete_file_np` looks like the function for this and is not: Darwin answers `ENOTSUP` for
/// `ACL_TYPE_EXTENDED`. Setting an empty ACL is what the OS actually supports, and it succeeds on a
/// directory that has no ACL to begin with, which is what makes this idempotent.
fn clear_acl(path: &Path) -> Result<()> {
    let c_path = c_path(path, "restrict")?;

    #[expect(unsafe_code, reason = "see the extern block above")]
    // SAFETY: `c_path` is a valid NUL-terminated string that outlives the call. `empty` is whatever
    // `acl_init` returned and is only handed back to the same API; it is freed exactly once, on
    // every path out of this function, and never used afterwards.
    let failure = unsafe {
        let empty = acl_init(0);
        if empty.is_null() {
            return Err(io(path, "restrict", std::io::Error::last_os_error()));
        }

        let outcome = acl_set_file(c_path.as_ptr(), ACL_TYPE_EXTENDED, empty);

        // Read `errno` before releasing the ACL, not after: `acl_free` is free to set it even when
        // it succeeds, and it would be setting it over the one answer this function exists to
        // report. A message naming the wrong reason is worse than one naming none.
        let failure = (outcome != 0).then(std::io::Error::last_os_error);

        acl_free(empty);
        failure
    };

    failure.map_or(Ok(()), |source| Err(io(path, "restrict", source)))
}

/// Does `path` carry an ACL at all?
///
/// Any ACL counts. The daemon puts none there, so an ACE is by definition something else's — a
/// directory that was shared, restored from a backup that carried its ACL, or copied with `cp -p`.
/// Reading *what* it grants would only let `mix doctor` argue about an entry it is going to remove.
fn has_acl(path: &Path) -> Result<bool> {
    let c_path = c_path(path, "read the ACL of")?;

    #[expect(unsafe_code, reason = "see the extern block above")]
    // SAFETY: as `clear_acl`. The returned handle is freed when it is non-null and never
    // dereferenced; when it is null there is nothing to free, per `acl_get_file`.
    let acl = unsafe { acl_get_file(c_path.as_ptr(), ACL_TYPE_EXTENDED) };

    if acl.is_null() {
        let source = std::io::Error::last_os_error();

        // `ENOENT` is this API's way of saying "no ACL", not "no such file" — the caller has
        // already established that the directory exists. Anything else is a real failure and is
        // reported rather than read as an absence.
        return if source.raw_os_error() == Some(ENOENT) {
            Ok(false)
        } else {
            Err(io(path, "read the ACL of", source))
        };
    }

    #[expect(unsafe_code, reason = "see the extern block above")]
    // SAFETY: `acl` is non-null and came from `acl_get_file`; this is its matching free, and it is
    // not used again.
    unsafe {
        acl_free(acl);
    }

    Ok(true)
}

/// The path as C sees it.
///
/// A Unix path is bytes, and the only byte it may not contain is the one that terminates a C
/// string. A path holding a NUL cannot name a real directory, so this is a caller bug rather than
/// an OS failure — but it arrives through `MIXENGINE_HOME`, so it is answered, not asserted.
fn c_path(path: &Path, action: &'static str) -> Result<CString> {
    CString::new(path.as_os_str().as_bytes()).map_err(|_| {
        io(
            path,
            action,
            std::io::Error::new(
                std::io::ErrorKind::InvalidInput,
                "the path contains a NUL byte",
            ),
        )
    })
}

/// What the C API complained about, attached to the path it was about.
fn io(path: &Path, action: &'static str, source: std::io::Error) -> Error {
    Error::Io {
        action,
        path: path.to_path_buf(),
        source,
    }
}
