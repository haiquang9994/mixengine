//! Writing a file that only this account may read.
//!
//! **A free function rather than a `Host` capability**, for [`generate_secret`](crate::generate_secret)'s
//! reason: it belongs to no subsystem, it holds no state, and a trait method would come with a mock
//! that could answer "restricted" while restricting nothing — which for the one file in this
//! product that must not be readable is the wrong thing to make easy.
//!
//! **It is not `DirectoryAccess` with a different argument.** That capability grants `(OI)(CI)F` on
//! Windows — Object Inherit and Container Inherit, which describe what a *directory* hands down to
//! what is created inside it — and `icacls` refuses those flags on a file. The accounts are the
//! same three; the grant is not.
//!
//! What each system does is in `sys::private_file`, and the two are not variations on one idea.
//! Unix carries the permission in the `open(2)` call that creates the file. Windows cannot name a
//! file in an ACL before the file exists, so it creates an empty one, restricts that, and only then
//! writes. Both close the same window — the moment in which a private key sits in a file somebody
//! else could open — and they close it at different points because that is where each OS puts it.

use std::path::Path;

use crate::Result;

/// Write `bytes` to `path`, readable by this account and by nobody who is not already entitled to
/// read every file on the machine.
///
/// Overwrites. An existing file is made private **before** the new content reaches it, so replacing
/// a key in a home written by an older version does not publish it on the way past.
///
/// "Nobody" carries [`DirectoryAccess::restrict_to_owner`](crate::DirectoryAccess::restrict_to_owner)'s
/// meaning: this user, plus `root` on Unix and `SYSTEM` and the local administrators on Windows,
/// who can read anything regardless and whom promising to exclude would be a promise no OS keeps.
///
/// # Errors
///
/// [`Error::Io`](crate::Error::Io) naming `path` when it cannot be created, restricted or written.
pub fn write_private(path: &Path, bytes: &[u8]) -> Result<()> {
    crate::sys::private_file::write(path, bytes)
}

/// Is `path` a file only this account may read?
///
/// **Windows only, and it exists for the tests.** On Unix the answer is one `metadata` call and a
/// mask, which a caller reads directly; on Windows it is a parse of `icacls` output that already
/// lives in this crate, and writing a second parser inside a test would give the test its own
/// opinion of what this crate applies.
///
/// # Errors
///
/// [`Error::Io`](crate::Error::Io) when `path` does not exist or its permissions cannot be read.
#[cfg(windows)]
pub fn is_private_file(path: &Path) -> Result<bool> {
    crate::sys::private_file::is_private(path)
}
