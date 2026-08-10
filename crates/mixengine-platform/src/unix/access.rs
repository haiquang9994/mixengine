//! `0700`, and nothing subtler.
//!
//! The mode is the whole story on Linux: a POSIX ACL grants nothing past the group class, and
//! `chmod` sets that mask to zero here, so a named-user entry left by somebody else is masked out
//! whether or not it is still listed.
//!
//! **Not so on macOS**, whose ACLs are NFSv4-style and sit beside the mode rather than under it: an
//! ACE granting another user survives `chmod` and keeps working. This module is therefore only half
//! of the macOS answer — `macos/access.rs` wraps it and empties the ACL as well. (Named rather than
//! linked: each OS directory is mapped onto `sys` by `#[path]`, so `crate::macos` is not a path
//! that exists, and `crate::sys::access` would resolve only on the OS that has one.)

use std::fs;
use std::os::unix::fs::PermissionsExt as _;
use std::path::Path;

use crate::{DirectoryAccess, Error, Result};

/// Owner: read, write, execute. Group and other: nothing.
const OWNER_ONLY: u32 = 0o700;

/// The permission bits of `st_mode`, without the file-type bits it also carries. Wide enough to
/// include setuid, setgid and sticky, which is deliberate — see [`Access::is_restricted_to_owner`].
const MODE_BITS: u32 = 0o7777;

#[derive(Debug, Default)]
pub(crate) struct Access;

impl DirectoryAccess for Access {
    fn restrict_to_owner(&self, path: &Path) -> Result<()> {
        // Set rather than mask: a directory that arrived from a backup or a `mv` off another
        // filesystem can be *more* permissive than the umask would have made it, and clearing only
        // the bits this process happens to dislike would leave the rest.
        fs::set_permissions(path, fs::Permissions::from_mode(OWNER_ONLY)).map_err(|source| {
            Error::Io {
                action: "restrict",
                path: path.to_path_buf(),
                source,
            }
        })
    }

    fn is_restricted_to_owner(&self, path: &Path) -> Result<bool> {
        let mode = fs::metadata(path)
            .map_err(|source| Error::Io {
                action: "read the permissions of",
                path: path.to_path_buf(),
                source,
            })?
            .permissions()
            .mode();

        // Exactly `0700`, not merely "no group or other bits": setgid on a directory changes who
        // owns what is created inside it and sticky changes who may delete it, neither of which
        // MixEngine ever asks for here. Something else set them, and `mix doctor` should say so.
        Ok(mode & MODE_BITS == OWNER_ONLY)
    }
}
