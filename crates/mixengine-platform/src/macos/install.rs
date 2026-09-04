//! macOS: `/Library/PrivilegedHelperTools`, and emphatically not `/usr/local`.

use std::path::PathBuf;

use crate::Result;

/// Where macOS puts a privileged helper: the directory `SMJobBless` installs into, `root:wheel`,
/// and claimed by no package manager.
///
/// **`/usr/local` was the draft and is wrong on this system** — the T85 design, D3. Homebrew on an
/// Intel Mac takes ownership of `/usr/local` and everything under it for the installing user, which
/// makes it the one directory here where a "root-owned" helper would be nothing of the kind. On
/// Apple Silicon Homebrew uses `/opt/homebrew` and leaves `/usr/local` absent, so the same constant
/// would also mean two different things on two Macs.
///
/// The directory is flat by convention, so the file carries the reverse-DNS name rather than a bare
/// one that could collide with somebody else's helper.
const HELPER: &str = "/Library/PrivilegedHelperTools/dev.mixengine.elevate";

pub(crate) fn helper_path() -> Result<PathBuf> {
    Ok(PathBuf::from(HELPER))
}

#[cfg(feature = "elevated")]
pub(crate) use crate::unix::install::own_as_root;

/// `/Library/PrivilegedHelperTools` is shared with every other product that installs a helper there,
/// so the file goes and the directory stays — which is why this passes `false` where Linux passes
/// `true`. The same fact that made the directory the right place to install into makes it the wrong
/// one to remove.
#[cfg(feature = "elevated")]
pub(crate) fn remove_helper() -> Result<crate::install::HelperRemoval> {
    crate::unix::install::remove(&helper_path()?, false)
}

/// The executable bit an archive may not have carried — roadmap task **T88**.
pub(crate) use crate::unix::install::make_executable;
