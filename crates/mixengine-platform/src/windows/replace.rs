//! Replacing a system file on Windows without losing the ACL it carried.
//!
//! **Moved out of `windows/hosts.rs` by T42**, which brought a second caller. Nothing here is
//! specific to the hosts file any more except the reason the mechanism was chosen, which the
//! architecture document already records.

use std::path::Path;

use windows_sys::Win32::Storage::FileSystem::{REPLACEFILE_IGNORE_MERGE_ERRORS, ReplaceFileW};

use crate::{Error, Result};

/// Replace `path` with `contents`, keeping the ACL it had.
///
/// `ReplaceFileW` and not a rename: a rename discards the target's ACL, its attributes and its
/// creation time, and `%SystemRoot%\System32\drivers\etc\hosts` has an ACL that matters — the
/// architecture document already names this call for exactly this reason.
///
/// # Errors
///
/// [`Error::Io`] when the temporary cannot be written or cannot be swapped into place.
pub(crate) fn atomically(path: &Path, contents: &str) -> Result<()> {
    use std::io::Write as _;

    let directory = path.parent().unwrap_or_else(|| Path::new("."));
    let temporary = directory.join(temporary_name(path));

    let failed = |path: &Path, source: std::io::Error| Error::Io {
        action: "write",
        path: path.to_path_buf(),
        source,
    };

    let written = (|| -> std::io::Result<()> {
        let mut file = std::fs::File::create(&temporary)?;
        file.write_all(contents.as_bytes())?;
        file.sync_all()
    })();

    if let Err(source) = written {
        let _ = std::fs::remove_file(&temporary);
        return Err(failed(&temporary, source));
    }

    // A file that is not there has no ACL to preserve, and `ReplaceFileW` refuses a target that does
    // not exist. A plain rename is the whole of the swap in that case.
    let swapped = if path.exists() {
        swap(&temporary, path)
    } else {
        std::fs::rename(&temporary, path)
    };

    if let Err(source) = swapped {
        let _ = std::fs::remove_file(&temporary);
        return Err(failed(path, source));
    }

    Ok(())
}

/// `<name>.mixengine-<pid>`, beside the file being replaced.
///
/// Named after the target rather than after `hosts`: more than one file goes through here now, and
/// a temporary called `hosts.mixengine-42` sitting next to another one is something somebody would
/// have to work out.
fn temporary_name(path: &Path) -> String {
    let name = path.file_name().map_or_else(
        || "file".to_owned(),
        |name| name.to_string_lossy().into_owned(),
    );

    format!("{name}.mixengine-{}", std::process::id())
}

/// `ReplaceFileW`, which swaps the two files and carries the replaced one's security descriptor onto
/// the replacement.
#[expect(
    unsafe_code,
    reason = "the only swap on this system that keeps the replaced file's ACL and attributes"
)]
fn swap(temporary: &Path, target: &Path) -> std::io::Result<()> {
    let replaced = wide(target);
    let replacement = wide(temporary);

    // SAFETY: both pointers are to NUL-terminated wide strings owned by this frame, which outlive
    // the call. The backup name is null, which the API documents as "keep no backup"; `lpExclude`
    // and `lpReserved` are documented as reserved and must be null.
    // `REPLACEFILE_IGNORE_MERGE_ERRORS` keeps a failure to merge metadata from failing a swap that
    // otherwise worked.
    let ok = unsafe {
        ReplaceFileW(
            replaced.as_ptr(),
            replacement.as_ptr(),
            std::ptr::null(),
            REPLACEFILE_IGNORE_MERGE_ERRORS,
            std::ptr::null(),
            std::ptr::null(),
        )
    };

    if ok == 0 {
        return Err(std::io::Error::last_os_error());
    }

    Ok(())
}

/// A path as the NUL-terminated wide string every `…W` call wants.
fn wide(path: &Path) -> Vec<u16> {
    use std::os::windows::ffi::OsStrExt as _;

    path.as_os_str().encode_wide().chain(Some(0)).collect()
}
