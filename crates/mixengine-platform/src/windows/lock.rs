//! A share mode nobody else can match, released by Windows when the process ends.
//!
//! Windows has `LockFileEx`, and this deliberately does not use it. A byte-range lock is *mandatory*
//! here rather than advisory, so the range holding the pid could not then be read by the daemon that
//! wants to know who is holding the file — the lock would have to be parked at some invented offset
//! away from the data, which is a trick to remember rather than a rule to follow.
//!
//! What is used instead is the thing Windows does natively and no other system does: the share mode
//! declared when the file is opened. This handle asks for read and write access and permits others
//! only to *read*, so a second daemon opening the same file for writing is refused with
//! `ERROR_SHARING_VIOLATION` while anybody may still read the pid out of it. It costs no `unsafe`,
//! no `windows-sys` call, and — like `flock` on the other side — it is released by the kernel when
//! the process ends, killed or not.
//!
//! **It also withholds `FILE_SHARE_DELETE`, and that is a difference from Unix worth knowing about.**
//! While a daemon is running, `run/mixengined.lock` cannot be deleted or renamed, which means `run/`
//! and the home directory above it cannot be either — Windows refuses to remove a directory holding
//! a file somebody has open this way. On Unix nothing stops an `rm -rf` of a live home, and the
//! daemon carries on writing into files that no longer have names. Neither behaviour is designed;
//! this one is the better of the two and is left as it is rather than granted away, because a home
//! that cannot be deleted out from under its daemon is a home that cannot be half-deleted.

use std::fs::{File, OpenOptions};
use std::io;
use std::os::windows::fs::OpenOptionsExt as _;
use std::path::Path;

use windows_sys::Win32::Foundation::ERROR_SHARING_VIOLATION;
use windows_sys::Win32::Storage::FileSystem::FILE_SHARE_READ;

use crate::lock::Acquired;
use crate::{Error, Result};

/// The handle whose share mode is the lock. Closing it releases the file for the next daemon.
#[derive(Debug)]
pub(crate) struct Lock {
    _file: File,
}

pub(crate) fn acquire(path: &Path) -> Result<Acquired> {
    // `FILE_SHARE_READ` alone: readers welcome, a second writer refused. Without `read` in the
    // access mask this handle could not be re-read by anything, and without `truncate(false)` the
    // open would map to `CREATE_ALWAYS` — which fails on a locked file anyway, but would empty the
    // holder's pid the moment it did not.
    let mut file = match OpenOptions::new()
        .read(true)
        .write(true)
        .create(true)
        .truncate(false)
        .share_mode(FILE_SHARE_READ)
        .open(path)
    {
        Ok(file) => file,

        // The one answer that is not a failure. Nothing else produces it here: the path is inside
        // `run/`, which belongs to this account, so the file cannot be somebody else's to refuse.
        Err(error) if error.raw_os_error() == Some(ERROR_SHARING_VIOLATION as i32) => {
            return Ok(crate::lock::taken(crate::lock::recorded_pid(path)));
        }

        Err(source) => return Err(failed("open the lock file at", path, source)),
    };

    crate::lock::record_pid(&mut file)
        .map_err(|source| failed("write the lock file at", path, source))?;

    Ok(crate::lock::held(Lock { _file: file }))
}

/// An operation on the lock file that Windows refused.
fn failed(action: &'static str, path: &Path, source: io::Error) -> Error {
    Error::Io {
        action,
        path: path.to_path_buf(),
        source,
    }
}
