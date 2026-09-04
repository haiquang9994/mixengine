//! What the two Unixes do identically about an installed helper: one owner and one mode.
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

pub(crate) fn own_as_root(path: &Path) -> Result<()> {
    // **First, and not skipped on Linux because it is a no-op there.** `std::fs::copy` on macOS is
    // `fclonefileat`/`fcopyfile` with `COPYFILE_ALL`, which carries the source file's uid across —
    // so a root process copying a helper out of a user-owned build directory produces a file that
    // user owns, in a directory root owns. Measured: CI's macOS leg installed it as uid 501.
    std::os::unix::fs::chown(path, Some(0), Some(0)).map_err(|source| Error::Io {
        action: "give root the ownership of",
        path: path.to_path_buf(),
        source,
    })?;

    // Set rather than masked, and after creation rather than through the umask: a umask this
    // process did not choose would otherwise decide whether the helper can be run at all — and on
    // macOS the copy above brought the source's mode with it, which is whatever `cargo` left.
    fs::set_permissions(path, fs::Permissions::from_mode(EXECUTABLE)).map_err(|source| Error::Io {
        action: "set the permissions of",
        path: path.to_path_buf(),
        source,
    })
}
