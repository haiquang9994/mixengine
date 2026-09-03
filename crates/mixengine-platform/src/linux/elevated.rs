//! Linux: the shared Unix primitives, and `/var/log/mixengine`.

#[cfg(feature = "elevated")]
use std::path::PathBuf;

#[cfg(feature = "elevated")]
use crate::Result;

#[cfg(feature = "elevated")]
pub(crate) use crate::unix::elevated::{create_root_owned_directory, is_elevated};
pub(crate) use crate::unix::elevated::{others_can_write, owner_of};

/// Where a daemon's log belongs on this system, by the Filesystem Hierarchy Standard.
#[cfg(feature = "elevated")]
const AUDIT_DIRECTORY: &str = "/var/log/mixengine";

#[cfg(feature = "elevated")]
pub(crate) fn audit_directory() -> Result<PathBuf> {
    Ok(PathBuf::from(AUDIT_DIRECTORY))
}
