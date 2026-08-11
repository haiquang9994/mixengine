//! The lock that makes one daemon per home.
//!
//! `run/mixengined.lock` is held for as long as the daemon runs and released by the operating
//! system when it stops — including when it is killed, which is the whole reason the lock is a file
//! handle and not a pid file somebody has to clean up. A stale lock file is therefore not a state
//! this code has to recognise: the file surviving means nothing, only the handle does.
//!
//! **This is not the same question the endpoint answers.** [`ipc::Listener::bind`](crate::ipc)
//! already refuses to start a second daemon that arrives *after* the first, and does it by dialling
//! the endpoint. What is left for the lock is two daemons starting at the same instant, where both
//! can find the endpoint dead and, on Unix, the second one's `bind` replaces the first one's socket
//! file while the first is still listening on it. The lock is taken before anything else so that
//! outcome is unreachable rather than merely tidied up afterwards, and it is why the daemon takes it
//! **before** it opens SQLite: `sqlx-sqlite` implements the migration lock as a no-op, so two
//! daemons that get that far can both read the schema as behind and both migrate it.
//!
//! **A daemon that finds the lock taken is not a failure**, which is why [`Acquired`] is not a
//! `Result`: the caller asked for a running daemon and there is one. `.claude/architecture/daemon-and-ipc.md`
//! has it exit successfully after printing the endpoint, and that is only a sensible thing to do if
//! the answer arrives as an outcome rather than as an error somebody has to classify.
//!
//! Not a [`Host`](crate::Host) capability, for the same reason [`ipc`](crate::ipc) is not: what is
//! being tested is whether *this* operating system keeps a second process out, and a mock that
//! answered from memory would prove nothing about that.

use std::fmt;
use std::fs::File;
use std::io::{Seek as _, SeekFrom, Write as _};
use std::path::Path;
use std::{fs, io};

use crate::Result;
use crate::sys::lock as sys;

/// What one [`Lock::acquire`] found.
#[derive(Debug)]
pub enum Acquired {
    /// Nobody held it. This process does now, until the [`Lock`] is dropped or the process ends.
    Held(Lock),

    /// Somebody else holds it, described as well as the lock file allows.
    Taken(Holder),
}

/// A held single-instance lock.
///
/// Releasing it is dropping it, and the file is deliberately **not** removed on the way out.
/// Unlinking a lock file is how two processes end up holding two different files under one name: the
/// next daemon can create and lock a fresh file at the same path while this one still holds the old
/// one. The content is rewritten by whoever acquires it next, so nothing accumulates.
#[derive(Debug)]
pub struct Lock {
    /// The open handle, whose existence *is* the lock on both systems. Never read again.
    _inner: sys::Lock,
}

/// Whoever is holding the lock, for a message to a person.
///
/// Descriptive and not actionable, like [`Peer`](crate::ipc::Peer): the pid comes from the lock
/// file, which the holder writes after it has the lock, so a daemon that started microseconds ago
/// may not have written one yet. Nothing branches on it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Holder {
    pid: Option<u32>,
}

impl Lock {
    /// Take the lock at `path`, or find out who has it.
    ///
    /// The file is created if it is not there and is **never truncated on open**, which matters
    /// more than it looks: truncating would erase the running daemon's pid on Unix, where opening a
    /// file somebody else has flocked succeeds and only the lock itself is refused.
    ///
    /// # Errors
    ///
    /// [`Error::Io`](crate::Error::Io) when the lock file cannot be created or written — a `run/`
    /// that is not writable, a full disk — and [`Error::Os`](crate::Error::Os) when the OS refuses
    /// the lock for a reason other than somebody else holding it.
    pub fn acquire(path: &Path) -> Result<Acquired> {
        sys::acquire(path)
    }
}

impl Holder {
    /// The process id it recorded, when it had got as far as recording one.
    #[must_use]
    pub fn pid(&self) -> Option<u32> {
        self.pid
    }
}

impl fmt::Display for Holder {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self.pid {
            Some(pid) => write!(f, "pid {pid}"),
            None => f.write_str("an unidentified process"),
        }
    }
}

/// Wrap a lock an implementation has just taken.
pub(crate) fn held(inner: sys::Lock) -> Acquired {
    Acquired::Held(Lock { _inner: inner })
}

/// Report the lock as somebody else's, with whatever the file said about them.
pub(crate) fn taken(pid: Option<u32>) -> Acquired {
    Acquired::Taken(Holder { pid })
}

/// Who the lock file says is holding it, if anyone legible.
///
/// Every failure — no file, no permission, a file holding something that is not a number — is one
/// answer: nothing is known about the holder. The pid is a courtesy in a log line, so there is
/// nothing here worth failing a startup over.
pub(crate) fn recorded_pid(path: &Path) -> Option<u32> {
    fs::read_to_string(path).ok()?.trim().parse().ok()
}

/// Write this process's id into the lock file it has just taken.
///
/// Truncated first, because the previous holder's pid is longer or shorter than ours at random and a
/// partial overwrite would leave a number belonging to neither of us. Flushed rather than left to
/// the buffer, because the next daemon may read this file within milliseconds.
pub(crate) fn record_pid(file: &mut File) -> io::Result<()> {
    file.set_len(0)?;
    file.seek(SeekFrom::Start(0))?;
    file.write_all(format!("{}\n", std::process::id()).as_bytes())?;
    file.flush()
}
