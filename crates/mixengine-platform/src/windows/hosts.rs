//! Where Windows keeps the hosts file, and how a file is replaced there.

use std::path::{Path, PathBuf};

#[cfg(feature = "elevated")]
use windows_sys::Win32::Storage::FileSystem::{REPLACEFILE_IGNORE_MERGE_ERRORS, ReplaceFileW};

#[cfg(feature = "elevated")]
use crate::{Error, Result};

/// `%SystemRoot%\System32\drivers\etc\hosts`, with the directory read from the environment.
///
/// **Not hard-coded to `C:\Windows`**: a machine imaged onto another drive letter is unusual and is
/// not something a binary running as an administrator should be guessing about. `SystemRoot` is set
/// by the kernel on every Windows process, so the fallback below is unreachable in practice and is
/// written rather than unwrapped because nothing in this crate panics.
pub(crate) fn path() -> PathBuf {
    let root = std::env::var_os("SystemRoot").unwrap_or_else(|| r"C:\Windows".into());

    PathBuf::from(root)
        .join("System32")
        .join("drivers")
        .join("etc")
        .join("hosts")
}

/// Replace `path` with `contents`, keeping the ACL it had.
///
/// `ReplaceFileW` and not a rename: a rename discards the target's ACL, its attributes and its
/// creation time, and `%SystemRoot%\System32\drivers\etc\hosts` has an ACL that matters — the
/// architecture document already names this call for exactly this reason.
#[cfg(feature = "elevated")]
pub(crate) fn replace(path: &Path, contents: &str) -> Result<()> {
    use std::io::Write as _;

    let directory = path.parent().unwrap_or_else(|| Path::new("."));
    let temporary = directory.join(format!("hosts.mixengine-{}", std::process::id()));

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

    // A hosts file that is not there has no ACL to preserve, and `ReplaceFileW` refuses a target
    // that does not exist. A plain rename is the whole of the swap in that case.
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

/// `ReplaceFileW`, which swaps the two files and carries the replaced one's security descriptor onto
/// the replacement.
#[cfg(feature = "elevated")]
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
#[cfg(feature = "elevated")]
fn wide(path: &Path) -> Vec<u16> {
    use std::os::windows::ffi::OsStrExt as _;

    path.as_os_str().encode_wide().chain(Some(0)).collect()
}
