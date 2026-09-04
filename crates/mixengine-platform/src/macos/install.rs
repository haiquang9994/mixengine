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
