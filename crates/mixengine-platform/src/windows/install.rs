//! Windows: `%ProgramFiles%\MixEngine`, asked of the shell rather than of the environment.

use std::path::PathBuf;

use crate::Result;

/// MixEngine's own directory under Program Files.
///
/// The application itself installs per-user, into `%LOCALAPPDATA%\Programs\MixEngine`, so that an
/// update needs no UAC. This is the one directory it has anywhere else, and it holds exactly one
/// file.
const DIRECTORY: &str = "MixEngine";

/// The helper. `.exe`, because this is the one of the three systems where a program has a suffix.
const HELPER: &str = "mixengine-elevate.exe";

pub(crate) fn helper_path() -> Result<PathBuf> {
    Ok(super::known_folder::program_files()?
        .join(DIRECTORY)
        .join(HELPER))
}

#[cfg(feature = "elevated")]
pub(crate) fn own_as_root(path: &std::path::Path) -> Result<()> {
    // **Nothing to do on this system, and both halves of that are measured.**
    //
    // There is no execute bit: a file is a program because of its contents and its extension, and
    // who may run it is the DACL the directory hands down — `(OI)(CI)RX` for `Users`, written by
    // `create_root_owned_directory` on the directory this file was just created in.
    //
    // And a file created by a process holding an administrative token belongs to
    // `BUILTIN\Administrators` by that token's default owner, not to the account behind it: read
    // back off a real install on 2026-09-04, where `(Get-Acl …).Owner` answered
    // `BUILTIN\Administrators`. `CopyFile` does not carry an owner across the way macOS's
    // `fcopyfile` does, which is the case the Unix half of this exists for.
    //
    // Named rather than silently omitted, the way `others_can_write` is one module over.
    let _ = path;

    Ok(())
}
