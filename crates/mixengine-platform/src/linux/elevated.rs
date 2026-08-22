//! Linux: the shared Unix primitives, and `/var/log/mixengine`.

use std::path::PathBuf;

use crate::Result;

pub(crate) use crate::unix::elevated::{
    create_root_owned_directory, is_elevated, others_can_write, owner_of,
};

/// Where a daemon's log belongs on this system, by the Filesystem Hierarchy Standard.
const AUDIT_DIRECTORY: &str = "/var/log/mixengine";

pub(crate) fn audit_directory() -> Result<PathBuf> {
    Ok(PathBuf::from(AUDIT_DIRECTORY))
}
