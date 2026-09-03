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

/// Make a file the elevation prompt can start.
///
/// The other half of [`helper_path`], and the reason this module has a write at all: putting a
/// binary where root keeps one is two OS-specific facts, not one. Where it goes is above; whether a
/// freshly created file is executable is here — a mode on Unix, and on Windows a question the
/// filesystem does not ask, because the ACL inherited from
/// [`create_root_owned_directory`](crate::elevated::create_root_owned_directory) already says
/// Administrators and SYSTEM may write and everybody may read and execute.
///
/// `elevated` only: nothing running as the user has any business making a file in that directory.
///
/// # Errors
///
/// [`Error::Io`](crate::Error::Io) when the permission cannot be set.
#[cfg(feature = "elevated")]
pub fn make_executable(path: &std::path::Path) -> Result<()> {
    crate::sys::install::make_executable(path)
}

#[cfg(test)]
mod tests {
    use super::*;

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
