//! What the two Unixes do identically about an installed helper: one mode.
//!
//! The path is the half they do not share, and it stays in each of their own directories — the same
//! shape `unix/elevated.rs` has.

use std::fs;
use std::os::unix::fs::PermissionsExt as _;
use std::path::Path;

use crate::{Error, Result};

/// Owner writes; everybody reads and executes.
///
/// Executable by everybody because the account that raises the elevation prompt is an ordinary one,
/// and a helper it may not execute is a prompt that cannot start anything. Writable only by the
/// owner because the owner is root, which is the whole point of the directory this sits in.
const EXECUTABLE: u32 = 0o755;

pub(crate) fn make_executable(path: &Path) -> Result<()> {
    // Set rather than masked, and after creation rather than through the umask: a umask this
    // process did not choose would otherwise decide whether the helper can be run at all.
    fs::set_permissions(path, fs::Permissions::from_mode(EXECUTABLE)).map_err(|source| Error::Io {
        action: "set the permissions of",
        path: path.to_path_buf(),
        source,
    })
}
