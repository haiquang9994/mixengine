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

/// Nothing to do — roadmap task **T88**.
///
/// There is no execute bit on this system: a file is a program because of its contents and its
/// extension, and the swap that just wrote it kept the name it had. Named rather than silently
/// omitted, so a reader comparing the two halves of this module finds a sentence rather than a gap.
pub(crate) fn make_executable(path: &std::path::Path) -> Result<()> {
    let _ = path;

    Ok(())
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

/// Hand the helper and its directory to Windows' own removal queue — roadmap task **T87**.
///
/// **A measured constraint and not a preference.** A file whose image is mapped cannot be unlinked
/// on this system, and `mixengine-elevate.exe` is the running program when it applies
/// `helper-remove`. Renaming it is allowed; deleting it is not. `MoveFileExW` with a null
/// destination and `MOVEFILE_DELAY_UNTIL_REBOOT` writes the path into
/// `HKLM\SYSTEM\CurrentControlSet\Control\Session Manager\PendingFileRenameOperations`, which the
/// session manager applies **in the order it was written** at the next boot — so the directory can
/// follow the file it holds, provided it is queued after it.
///
/// `%ProgramFiles%\MixEngine` is MixEngine's own directory and holds exactly one file, so it is
/// queued too. `%ProgramFiles%` itself is never named.
///
/// **Rejected: the NTFS self-delete.** Renaming the primary data stream and then setting the delete
/// disposition does work, and it is the technique malware uses to remove its own dropper. Putting it
/// inside the one binary in this product that runs as root — whose stated design constraint is being
/// auditable in a sitting — buys one file's worth of tidiness for a paragraph no reviewer should
/// have to accept. See the T87 design, D8.
#[cfg(feature = "elevated")]
pub(crate) fn remove_helper() -> Result<crate::install::HelperRemoval> {
    let helper = helper_path()?;
    let mut removal = crate::install::HelperRemoval::default();

    // Nothing to schedule is the answer, not a fault: an uninstall run twice must not fail the
    // second time — and scheduling a path that does not exist would leave a queue entry naming
    // something that was never ours.
    if !helper.exists() {
        return Ok(removal);
    }

    schedule(&helper)?;
    removal.at_next_restart.push(helper.clone());

    // After the file and never before it: the queue is applied in the order it was written, and a
    // directory scheduled first would still hold the file when its turn came.
    if let Some(directory) = helper.parent() {
        schedule(directory)?;
        removal.at_next_restart.push(directory.to_path_buf());
    }

    Ok(removal)
}

/// One `MoveFileExW(path, NULL, MOVEFILE_DELAY_UNTIL_REBOOT)`.
///
/// Split out because it is called twice and because the `unsafe` belongs in a frame with nothing
/// else in it — `known_folder.rs`' arrangement, one module over.
#[cfg(feature = "elevated")]
fn schedule(path: &std::path::Path) -> Result<()> {
    use std::os::windows::ffi::OsStrExt as _;

    use windows_sys::Win32::Storage::FileSystem::{MOVEFILE_DELAY_UNTIL_REBOOT, MoveFileExW};

    let wide: Vec<u16> = path
        .as_os_str()
        .encode_wide()
        .chain(std::iter::once(0))
        .collect();

    #[expect(
        unsafe_code,
        reason = "one call with a NUL-terminated buffer owned by this frame and a null destination, \
                  which is how the documentation spells `delete this at the next restart`"
    )]
    let scheduled =
        unsafe { MoveFileExW(wide.as_ptr(), std::ptr::null(), MOVEFILE_DELAY_UNTIL_REBOOT) };

    if scheduled == 0 {
        return Err(crate::Error::Io {
            action: "schedule the removal of",
            path: path.to_path_buf(),
            source: std::io::Error::last_os_error(),
        });
    }

    Ok(())
}
