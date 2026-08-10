//! Keeping other local users out of the directories MixEngine owns.

use std::path::Path;

use crate::Result;

/// Owner-only access to a directory MixEngine created.
///
/// A freshly created directory is readable by everyone else on the machine on both families of
/// OS, for different reasons: on Unix the process umask leaves it `0755`, and on Windows it
/// inherits the volume's ACL — `C:\` grants `BUILTIN\Users` read and execute, inheritable into
/// every subdirectory. Neither is acceptable for `certs/`, which holds the CA private key, or for
/// `data/`, which holds the user's databases.
///
/// macOS has *both* problems at once, which is why it is the one platform whose implementation is
/// not the plain Unix one: the mode is `0755` as everywhere else, and an NFSv4 ACE marked
/// `directory_inherit` on any parent of the home lands on every directory created below it. That
/// ACE sits beside the mode rather than under it, so `chmod` neither removes nor masks it.
///
/// The default home is safe on Windows by accident (`%LOCALAPPDATA%` inherits an owner-only ACL)
/// and unsafe on Unix always. A relocated home or a `[paths]` override is unsafe on both.
pub trait DirectoryAccess: std::fmt::Debug + Send + Sync {
    /// Restrict `path` to its owner, replacing whatever it inherited.
    ///
    /// "Owner" means the current user, plus whoever can already read every file on the machine:
    /// `root` on Unix by definition, `SYSTEM` and the local administrators on Windows, who can
    /// take ownership regardless. Promising to exclude them would be a promise no OS keeps.
    ///
    /// Idempotent, and applied to directories that already exist as well as new ones — a home
    /// created by an older version does not stay world-readable just because it is not new.
    ///
    /// # Errors
    ///
    /// [`Error::Io`](crate::Error::Io) when the permissions cannot be read or written, and
    /// [`Error::UnsupportedPlatform`](crate::Error::UnsupportedPlatform) where the OS offers no
    /// way to express this.
    fn restrict_to_owner(&self, path: &Path) -> Result<()>;

    /// Is the restriction [`restrict_to_owner`](Self::restrict_to_owner) applies still in force?
    ///
    /// For `mix doctor` (T47), which reports rather than repairs: a home that was moved onto
    /// another volume, restored from a backup, or copied by hand arrives with the destination's
    /// permissions and nobody is told.
    ///
    /// What counts as "in force" is per-OS and deliberately narrow — see each implementation. It
    /// answers "is what we applied still there", not "is this directory secure by every measure".
    ///
    /// # Errors
    ///
    /// As [`restrict_to_owner`](Self::restrict_to_owner), plus the case where `path` does not
    /// exist: an absent directory has no permissions to report on, and saying `false` would send
    /// `mix doctor` off to fix the wrong problem.
    fn is_restricted_to_owner(&self, path: &Path) -> Result<bool>;
}
