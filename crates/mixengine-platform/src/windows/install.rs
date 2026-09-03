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
