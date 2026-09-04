//! The primitives that only mean anything under an administrative token.
//!
//! **Two callers and one split**, since T85. The *writing* half — the elevation bit, the audit
//! directory, the root-owned `mkdir` — is `mixengine-elevate`'s alone and stays behind the
//! `elevated` feature, so that binary can take this crate without `tokio`, `keyring` or
//! `directories` (the T40 design, D8). The *reading* half — [`owner_of`] and [`others_can_write`] —
//! is also the daemon's, because the question it answers is *may I run this file as root?* and that
//! is asked on the unprivileged side of the boundary, before a prompt is spent. Same shape as
//! `hosts`, `port_access`, `resolver` and `trust`, one capability along.
//!
//! **The identity of a caller is the owner of the file it wrote**, and that is the whole reason
//! [`owner_of`] exists. The daemon runs as the user, and if the daemon is compromised it *is* the
//! attacker, so nothing the request document asserts about who is asking can be believed. The
//! filesystem's answer can: `PKEXEC_UID`, a walk up to a parent process and an environment variable
//! are three mechanisms that differ per OS and two of which an attacker sets.

use std::fmt;
use std::path::Path;
#[cfg(feature = "elevated")]
use std::path::PathBuf;

use crate::Result;

/// The account a file belongs to.
///
/// Rendered as a decimal uid on Unix and as a SID on Windows — the SID because it survives a rename
/// and is not localised, which a display name is neither of. Compared, never parsed.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Owner {
    id: String,
    superuser: bool,
    administrative: bool,
}

impl Owner {
    /// Build one. Each OS decides what its two predicates mean; see the per-OS modules.
    pub(crate) fn new(id: String, superuser: bool, administrative: bool) -> Self {
        Self {
            id,
            superuser,
            administrative,
        }
    }

    /// Is this the account no MixEngine daemon ever runs as?
    ///
    /// `uid 0` on Unix, `SYSTEM` (`S-1-5-18`) on Windows — **and not `BUILTIN\Administrators`**,
    /// because a file created by an administrator on Windows is owned by that group and most Windows
    /// users are administrators. Reading that as "root wrote this" would refuse the ordinary case.
    #[must_use]
    pub fn is_superuser(&self) -> bool {
        self.superuser
    }

    /// Does this account already hold administrative power?
    ///
    /// The wider question, and the one the audit log's directory asks: a directory root appends into
    /// must not be one an ordinary account created, or the log is the attacker's to arrange. `uid 0`
    /// on Unix; `SYSTEM` **or** `BUILTIN\Administrators` on Windows, where `%ProgramData%` lets any
    /// account create a subdirectory.
    #[must_use]
    pub fn is_administrative(&self) -> bool {
        self.administrative
    }
}

impl fmt::Display for Owner {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.id)
    }
}

/// Is this process actually running with an administrative token?
///
/// Asked once and reported, rather than turned into a refusal to run at all: the operation whose job
/// includes reporting this could otherwise never report `false`. See
/// `PrivilegedOp::requires_elevation`.
#[cfg(feature = "elevated")]
#[must_use]
pub fn is_elevated() -> bool {
    crate::sys::elevated::is_elevated()
}

/// Who owns `path`, without following a symlink at the end of it.
///
/// # Errors
///
/// [`Error::Io`](crate::Error::Io) when the path cannot be read, and
/// [`Error::Os`](crate::Error::Os) when the OS will not name the owner.
pub fn owner_of(path: &Path) -> Result<Owner> {
    crate::sys::elevated::owner_of(path)
}

/// Can anybody other than the owner write `path`?
///
/// The mode's group and other write bits on Unix. **On Windows this is always `false`**: answering it
/// there means walking a DACL and resolving every trustee, and the check that carries the weight on
/// that system is ownership, which [`owner_of`] answers exactly.
///
/// # Errors
///
/// [`Error::Io`](crate::Error::Io) when the path cannot be read.
pub fn others_can_write(path: &Path) -> Result<bool> {
    crate::sys::elevated::others_can_write(path)
}

/// The directory the audit log lives in, whether or not it is there yet.
///
/// **Outside `MIXENGINE_HOME`, deliberately.** A root-owned file inside a directory the user owns can
/// be renamed or unlinked by that user whatever its own mode says, so "append-only" there is a
/// promise the filesystem does not keep. `%ProgramData%\MixEngine` on Windows,
/// `/Library/Logs/MixEngine` on macOS, `/var/log/mixengine` on Linux.
///
/// # Errors
///
/// [`Error::Os`](crate::Error::Os) on Windows when `%ProgramData%` is not set. Guessing a path in a
/// binary that runs as root is not a trade worth making.
#[cfg(feature = "elevated")]
pub fn audit_directory() -> Result<PathBuf> {
    crate::sys::elevated::audit_directory()
}

/// Create `path` as a directory root owns and everyone may read.
///
/// The opposite question to [`DirectoryAccess::restrict_to_owner`](crate::DirectoryAccess): there the
/// point is keeping other accounts out, here root owns it and the user must be able to read it —
/// `mix doctor` reads the log back.
///
/// Parents are created too, and **every call re-asserts the owner and the permissions** rather than
/// only the call that created the directory. That is what makes it safe to run on a directory that
/// is already there: creating one is `mkdir` plus two `icacls` calls, so a directory can exist with
/// the permissions of whatever it inherited from, and a caller that skipped this on the "it is
/// already there" branch would leave that state permanent.
///
/// The caller still checks ownership first, because a directory that is already there and is not
/// root's is a target rather than a convenience — permissions converge, ownership refuses.
///
/// # Errors
///
/// [`Error::Io`](crate::Error::Io) when it cannot be created, and
/// [`Error::Command`](crate::Error::Command) on Windows when `icacls` refuses.
#[cfg(feature = "elevated")]
pub fn create_root_owned_directory(path: &Path) -> Result<()> {
    crate::sys::elevated::create_root_owned_directory(path)
}
