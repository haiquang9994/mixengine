//! macOS: the shared Unix primitives, and `/Library/Logs/MixEngine`.

#[cfg(feature = "elevated")]
use std::path::PathBuf;

#[cfg(feature = "elevated")]
use crate::Result;

#[cfg(feature = "elevated")]
pub(crate) use crate::unix::elevated::{create_root_owned_directory, is_elevated};
pub(crate) use crate::unix::elevated::{others_can_write, owner_of};

/// Where a system-wide log belongs on this system, and where `Console.app` looks for one.
///
/// `/var/log` exists here too, but it is Apple's: a third-party log in it is a log the OS's own
/// maintenance may take a view on, and nothing in the user interface goes looking there.
#[cfg(feature = "elevated")]
const AUDIT_DIRECTORY: &str = "/Library/Logs/MixEngine";

#[cfg(feature = "elevated")]
pub(crate) fn audit_directory() -> Result<PathBuf> {
    Ok(PathBuf::from(AUDIT_DIRECTORY))
}
