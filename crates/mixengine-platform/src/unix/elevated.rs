//! What the two Unixes do identically: an effective uid, a `st_uid`, a mode, and `0755`.
//!
//! The path the log lives at is the half they do not share, and it stays in each of their own
//! directories. This is the same shape `unix/access.rs` has — shared behaviour here, the one
//! difference in the OS that has it.

use std::fs;
use std::os::unix::fs::MetadataExt as _;
#[cfg(feature = "elevated")]
use std::os::unix::fs::PermissionsExt as _;
use std::path::Path;

use crate::elevated::Owner;
use crate::{Error, Result};

/// Owner: read, write, execute. Everyone else: read and traverse. The log is evidence, and evidence
/// nobody may read is not evidence — and the helper installed beside it has to be startable by the
/// prompt an ordinary account raises.
#[cfg(feature = "elevated")]
const ROOT_OWNED: u32 = 0o755;

/// The group and other write bits — either of them means somebody else can rewrite the file.
const OTHERS_WRITE: u32 = 0o022;

#[cfg(feature = "elevated")]
pub(crate) fn is_elevated() -> bool {
    // `geteuid` and not `getuid`: what the process may do now is what matters, and a setuid binary
    // differs in exactly that. No error path — the call cannot fail.
    #[expect(
        unsafe_code,
        reason = "geteuid takes no arguments, touches no memory and cannot fail"
    )]
    let euid = unsafe { libc::geteuid() };

    euid == 0
}

pub(crate) fn owner_of(path: &Path) -> Result<Owner> {
    // `symlink_metadata`: the caller is about to decide whether to trust this path, and following a
    // link would answer about the target an attacker chose rather than about the link they planted.
    let uid = fs::symlink_metadata(path)
        .map_err(|source| Error::Io {
            action: "read the owner of",
            path: path.to_path_buf(),
            source,
        })?
        .uid();

    Ok(Owner::new(uid.to_string(), uid == 0, uid == 0))
}

pub(crate) fn others_can_write(path: &Path) -> Result<bool> {
    let mode = fs::symlink_metadata(path)
        .map_err(|source| Error::Io {
            action: "read the permissions of",
            path: path.to_path_buf(),
            source,
        })?
        .permissions()
        .mode();

    Ok(mode & OTHERS_WRITE != 0)
}

#[cfg(feature = "elevated")]
pub(crate) fn create_root_owned_directory(path: &Path) -> Result<()> {
    fs::create_dir_all(path).map_err(|source| Error::Io {
        action: "create",
        path: path.to_path_buf(),
        source,
    })?;

    // Set rather than masked, and after creation rather than through the umask: a umask this process
    // did not choose would otherwise decide whether the log can be read back at all.
    fs::set_permissions(path, fs::Permissions::from_mode(ROOT_OWNED)).map_err(|source| Error::Io {
        action: "set the permissions of",
        path: path.to_path_buf(),
        source,
    })
}
