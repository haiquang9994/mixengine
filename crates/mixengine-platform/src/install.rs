//! Where an installed MixEngine keeps the one file it runs as root.
//!
//! **One answer per operating system, read by both sides of the privilege boundary.** The daemon
//! reads it to decide which file to hand the elevation prompt; `mixengine-elevate` reads the same
//! function to decide where to put itself. Two answers to that question would be a helper installed
//! somewhere nothing ever looks — so there is one, and it is here.
//!
//! Nothing in this module creates anything or checks anything: it says *where*, and
//! `mixengine-elevate` is the only thing that ever writes there. Who owns what is already
//! [`crate::elevated::owner_of`]'s question, and the answer to it is what
//! `mixengine_core::elevation::helper` refuses on. See the T85 design, D1 and D3.

use std::path::PathBuf;

use crate::Result;

/// The privileged helper's installed path on this machine, whether or not it is there yet.
///
/// `%ProgramFiles%\MixEngine\mixengine-elevate.exe` on Windows,
/// `/Library/PrivilegedHelperTools/dev.mixengine.elevate` on macOS,
/// `/usr/local/libexec/mixengine/mixengine-elevate` on Linux — each argued in its own module.
///
/// **Not the directory beside the program**, which is what `mixengine_core::elevation::helper`
/// falls back to and what every build out of `cargo` uses. This is the copy an installed MixEngine
/// runs, and the whole reason it exists is that nothing running as the user may rewrite it.
///
/// # Errors
///
/// [`Error::Os`](crate::Error::Os) on Windows when the shell will not name Program Files. The two
/// Unixes cannot fail: their answers are compiled-in constants, and the [`Result`] is there so all
/// three have one signature.
pub fn helper_path() -> Result<PathBuf> {
    crate::sys::install::helper_path()
}

/// Make a freshly copied file root's, and one the elevation prompt can start.
///
/// The other half of [`helper_path`], and the reason this module has a write at all: putting a
/// binary where root keeps one is two OS-specific facts, not one. Where it goes is above; what a
/// freshly created file there ends up being is here.
///
/// **The owner is set and not assumed, and that is a measurement rather than a precaution.**
/// `std::fs::copy` on macOS is `fclonefileat`/`fcopyfile` with `COPYFILE_ALL`, which carries the
/// *source's* uid across — so a helper copied by root out of a user-owned build directory arrives
/// owned by that user, inside a directory root owns, and the whole point of the directory is gone.
/// CI's macOS leg is what said so: the file installed as uid 501. Linux copies permission bits and
/// not ownership, so it never showed there, and Windows has no such call.
///
/// `elevated` only: nothing running as the user has any business making a file in that directory.
///
/// # Errors
///
/// [`Error::Io`](crate::Error::Io) when the owner or the permission cannot be set.
#[cfg(feature = "elevated")]
pub fn own_as_root(path: &std::path::Path) -> Result<()> {
    crate::sys::install::own_as_root(path)
}

/// What a helper removal actually managed, per path.
///
/// **Two lists and not a boolean**, because on one of the three systems the answer is neither yes
/// nor no: Windows cannot unlink a file whose image is mapped, and the helper is the running program
/// when it is asked to remove itself — so what happens there is that the operating system accepts
/// the removal and performs it at the next restart. A caller that folded both into "removed" would
/// report a file as gone while it was still on disk. See the T87 design, D8.
#[derive(Debug, Default, Clone, PartialEq, Eq)]
pub struct HelperRemoval {
    /// Paths that are gone now.
    pub removed: Vec<PathBuf>,

    /// Paths the operating system has accepted and will remove at the next restart.
    pub at_next_restart: Vec<PathBuf>,
}

impl HelperRemoval {
    /// Was there nothing to do?
    ///
    /// A helper that was never installed answers `true`, which is what makes running an uninstall
    /// twice not a failure the second time.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.removed.is_empty() && self.at_next_restart.is_empty()
    }
}

/// Take the privileged helper off this machine — roadmap task **T87**.
///
/// The other end of [`own_as_root`], and the reversal
/// [ADR 0015](../../../../.claude/decisions/0015-the-helper-installs-itself.md) owed uninstall: the
/// helper installs itself, so the helper is what removes itself.
///
/// Removes the file, and then the directory holding it **only where that directory is MixEngine's
/// own and only when it is empty**. A directory somebody else has put a file in is not this
/// function's to delete, and `rmdir` refusing is the check rather than a walk deciding what belongs
/// to whom. `/Library/PrivilegedHelperTools` on macOS is therefore never touched at all: it is
/// shared with every other product that installs a helper there.
///
/// **A helper that is not installed is not an error.** `mix uninstall` run twice must not fail the
/// second time, and *there was nothing there* is the answer rather than a fault.
///
/// `elevated` only: nothing running as the user can write that directory, so nothing running as the
/// user has any business trying.
///
/// # Errors
///
/// [`Error::Io`](crate::Error::Io) when a path that is there cannot be removed or, on Windows,
/// cannot be scheduled, and [`Error::Os`](crate::Error::Os) on Windows when the shell will not name
/// Program Files.
#[cfg(feature = "elevated")]
pub fn remove_helper() -> Result<HelperRemoval> {
    crate::sys::install::remove_helper()
}

#[cfg(test)]
mod tests {
    use super::*;

    /// An empty removal is neither a failure nor a change, and the two vectors are how a caller
    /// tells "it was not there" from "it is scheduled".
    #[test]
    fn a_removal_that_did_nothing_carries_nothing() {
        let nothing = HelperRemoval::default();

        assert!(nothing.removed.is_empty());
        assert!(nothing.at_next_restart.is_empty());
        assert!(nothing.is_empty());
    }

    /// The one property all three systems share, asserted from the outside: a helper that is not
    /// there is a removal that did nothing, and never an error. That is what makes `mix uninstall`
    /// idempotent — running it twice must not fail the second time.
    ///
    /// **Where one *is* installed the premise does not hold**, and this asserts the premise instead
    /// of the behaviour: running it there would be uninstalling the developer's own helper.
    #[cfg(feature = "elevated")]
    #[test]
    fn removing_a_helper_that_is_not_installed_is_not_a_failure() {
        let path = helper_path().expect("this OS names a directory for a privileged helper");

        if path.exists() {
            return;
        }

        let removal = remove_helper().expect("nothing to remove is not an error");

        assert!(removal.is_empty(), "{removal:?}");
    }

    /// The one property all three systems share: an absolute path with a parent.
    ///
    /// A relative answer would be resolved against whatever directory the daemon happened to be
    /// started from, which is not a property of this machine at all — and the caller that acts on
    /// it is running as root.
    #[test]
    fn the_helper_has_an_absolute_home_on_this_system() {
        let path = helper_path().expect("this OS names a directory for a privileged helper");

        assert!(path.is_absolute(), "{} is not absolute", path.display());
        assert!(path.parent().is_some());
        assert!(
            path.file_name()
                .and_then(std::ffi::OsStr::to_str)
                .is_some_and(|name| name.contains("elevate")),
            "{} is not named after the helper",
            path.display()
        );
    }
}
