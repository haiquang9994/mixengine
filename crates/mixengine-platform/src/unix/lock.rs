//! `flock` on an open file, released by the kernel when the process ends.
//!
//! Identical on Linux and macOS: `flock` is BSD's, both systems have it, and neither has anything to
//! add to it — so like the socket next door this lives in `unix/` and neither OS directory mentions
//! it.
//!
//! **`flock`, not `fcntl`.** POSIX record locks are owned by the *process* and are dropped by any
//! `close` of any descriptor onto the same file, so a library that opened the lock file to read the
//! pid out of it and then closed it would silently release a lock it never took. `flock` belongs to
//! the open file description instead, which means the lock lives and dies with the handle this
//! module holds and nothing else in the process can affect it.

use std::fs::{File, OpenOptions};
use std::io;
use std::os::fd::AsRawFd as _;
use std::path::Path;

use crate::lock::Acquired;
use crate::{Error, Result};

/// The handle whose existence is the lock. Closing it — deliberately, or because the process died —
/// is what releases it, and there is nothing else to undo.
#[derive(Debug)]
pub(crate) struct Lock {
    _file: File,
}

pub(crate) fn acquire(path: &Path) -> Result<Acquired> {
    // Not `truncate(true)`: opening a file another daemon has flocked succeeds — only the lock is
    // refused — so truncating here would erase a running daemon's pid before we discovered it was
    // running. The file is emptied after the lock is ours instead.
    let mut file = OpenOptions::new()
        .read(true)
        .write(true)
        .create(true)
        .truncate(false)
        .open(path)
        .map_err(|source| failed("open the lock file at", path, source))?;

    if !acquired(&file)? {
        // Read from the path rather than from the handle we are holding: on the taken path this
        // process has no business seeking around in a file another daemon is writing, and the
        // answer is a courtesy either way.
        return Ok(crate::lock::taken(crate::lock::recorded_pid(path)));
    }

    crate::lock::record_pid(&mut file)
        .map_err(|source| failed("write the lock file at", path, source))?;

    Ok(crate::lock::held(Lock { _file: file }))
}

/// Take the exclusive lock, and say whether it is now ours.
///
/// `true` means *this* process holds it, which is the direction the name has to read in: the caller
/// branches on it immediately, and a predicate that answered "somebody has it" would be true in both
/// outcomes.
///
/// Non-blocking on purpose: this is asked during startup, and a daemon that waited here would hang
/// for as long as the other one runs instead of saying which one it is.
#[expect(
    unsafe_code,
    reason = "flock takes a descriptor and two flags, touches no memory of ours, and is the only \
              way to ask; the descriptor is borrowed from a File that outlives the call"
)]
fn acquired(file: &File) -> Result<bool> {
    let locked = unsafe { libc::flock(file.as_raw_fd(), libc::LOCK_EX | libc::LOCK_NB) };

    if locked == 0 {
        return Ok(true);
    }

    let error = io::Error::last_os_error();

    // `EWOULDBLOCK` is the answer, not a failure: somebody else holds the lock. It is spelled
    // `EAGAIN` on both of these systems, which is the same number.
    if error.raw_os_error() == Some(libc::EWOULDBLOCK) {
        return Ok(false);
    }

    Err(Error::Os {
        action: "lock the daemon's lock file",
        source: error,
    })
}

/// An operation on the lock file that the OS refused.
fn failed(action: &'static str, path: &Path, source: io::Error) -> Error {
    Error::Io {
        action,
        path: path.to_path_buf(),
        source,
    }
}
