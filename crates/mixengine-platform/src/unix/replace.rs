//! Replacing a system file on macOS and Linux without ever leaving a torn one.
//!
//! **Moved out of `unix/hosts.rs` by T42**, which brought three more files that need what
//! `/etc/hosts` needed: `/etc/pf.conf`, `/etc/pf.anchors/mixengine` and a LaunchDaemon plist. One
//! mechanism for both systems, which is what `unix/` is for.

use std::path::Path;

use crate::{Error, Result};

/// What a system file carries when there is no existing one to copy the mode from.
const DEFAULT_MODE: u32 = 0o644;

/// Replace `path` with `contents`, keeping the mode, uid and gid it had.
///
/// A temporary file **in the same directory** and a rename: a rename across filesystems is a copy
/// and is not atomic, and a machine that loses power half way through must find either the old file
/// or the new one and never a truncated one.
///
/// **No backup file.** The rename is atomic, so there is no torn state to recover from, and a
/// `hosts.mixengine.bak` left in `/etc` is litter that outlives the reason for it.
///
/// # Errors
///
/// [`Error::Io`] when the temporary cannot be written, cannot take the target's ownership, or
/// cannot be renamed into place.
pub(crate) fn atomically(path: &Path, contents: &str) -> Result<()> {
    use std::io::Write as _;

    let directory = path.parent().unwrap_or_else(|| Path::new("."));
    let temporary = directory.join(temporary_name(path));

    let failed = |path: &Path, source: std::io::Error| Error::Io {
        action: "write",
        path: path.to_path_buf(),
        source,
    };

    // Every failure below leaves the temporary file behind unless it is removed, and `/etc` is not a
    // directory to leave litter in.
    let written = (|| -> std::io::Result<()> {
        let mut file = std::fs::File::create(&temporary)?;
        file.write_all(contents.as_bytes())?;
        file.sync_all()
    })();

    if let Err(source) = written {
        let _ = std::fs::remove_file(&temporary);
        return Err(failed(&temporary, source));
    }

    // The mode, uid and gid of the file being replaced, never a fresh set: skipping this is how a
    // `0644 root:root` /etc/hosts quietly becomes something wider, and nothing would report it.
    if let Err(source) = carry_ownership(path, &temporary) {
        let _ = std::fs::remove_file(&temporary);
        return Err(failed(&temporary, source));
    }

    if let Err(source) = std::fs::rename(&temporary, path) {
        let _ = std::fs::remove_file(&temporary);
        return Err(failed(path, source));
    }

    // The rename itself is what has to survive the power cut, and on every journalling filesystem
    // here that means the *directory* entry rather than the file.
    if let Ok(handle) = std::fs::File::open(directory) {
        let _ = handle.sync_all();
    }

    Ok(())
}

/// `.<name>.mixengine-<pid>`, beside the file being replaced.
///
/// Named after the target rather than after `hosts`: four files go through here now, and a
/// temporary called `.hosts.mixengine-42` sitting next to `/etc/pf.conf` is something somebody would
/// have to work out.
fn temporary_name(path: &Path) -> String {
    let name = path.file_name().map_or_else(
        || "file".to_owned(),
        |name| name.to_string_lossy().into_owned(),
    );

    format!(".{name}.mixengine-{}", std::process::id())
}

/// Give `temporary` the mode, uid and gid `path` has — or the mode a system file normally carries,
/// when there is no `path` to read them from.
fn carry_ownership(path: &Path, temporary: &Path) -> std::io::Result<()> {
    use std::os::unix::fs::{MetadataExt as _, PermissionsExt as _};

    let Ok(metadata) = std::fs::metadata(path) else {
        return std::fs::set_permissions(temporary, std::fs::Permissions::from_mode(DEFAULT_MODE));
    };

    std::fs::set_permissions(
        temporary,
        std::fs::Permissions::from_mode(metadata.permissions().mode()),
    )?;

    chown(temporary, metadata.uid(), metadata.gid())
}

/// `chown`, which the standard library has no stable equivalent of.
#[expect(
    unsafe_code,
    reason = "std has no chown; the alternative is leaving a replaced system file owned by whoever \
              ran the helper"
)]
fn chown(path: &Path, uid: u32, gid: u32) -> std::io::Result<()> {
    use std::ffi::CString;
    use std::os::unix::ffi::OsStrExt as _;

    let raw = CString::new(path.as_os_str().as_bytes()).map_err(|_| {
        std::io::Error::new(std::io::ErrorKind::InvalidInput, "a path holding a NUL")
    })?;

    // SAFETY: `raw` is a NUL-terminated C string that outlives the call, and `chown` reads it and
    // nothing else. The two ids come from the metadata of a file this process just stat'd.
    let result = unsafe { libc::chown(raw.as_ptr(), uid, gid) };

    if result == 0 {
        Ok(())
    } else {
        Err(std::io::Error::last_os_error())
    }
}
