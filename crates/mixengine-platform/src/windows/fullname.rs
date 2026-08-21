//! The name behind an 8.3 alias. See [`crate::paths::in_full`], which is the whole of the argument.

use std::ffi::OsString;
use std::os::windows::ffi::{OsStrExt as _, OsStringExt as _};
use std::path::{Path, PathBuf};

use windows_sys::Win32::Storage::FileSystem::GetLongPathNameW;

/// [`crate::paths::in_full`] on this system.
///
/// `GetLongPathNameW` answers for a path that exists and refuses one that does not — and a root is
/// resolved before it is created, so refusing is the ordinary case rather than the exceptional one.
/// What is expanded is therefore the longest prefix the filesystem can name, with everything below
/// it put back exactly as it came. A component that is not there has no alias to stand for, so the
/// two spellings of a path meet as soon as the directory appears and never diverge afterwards.
pub(crate) fn in_full(path: &Path) -> PathBuf {
    let mut trailing = Vec::new();
    let mut head = path.to_path_buf();

    loop {
        if let Some(spelled) = expanded(&head) {
            return trailing
                .iter()
                .rev()
                .fold(spelled, |whole: PathBuf, part| whole.join(part));
        }

        let Some(name) = head.file_name().map(std::ffi::OsStr::to_owned) else {
            // A root, a prefix, or the empty path: nothing left to walk up to.
            return path.to_path_buf();
        };

        trailing.push(name);

        if !head.pop() {
            return path.to_path_buf();
        }
    }
}

/// `path` as the filesystem names it, or `None` when it cannot name it at all.
fn expanded(path: &Path) -> Option<PathBuf> {
    let wide: Vec<u16> = path.as_os_str().encode_wide().chain(Some(0)).collect();

    // Asked for the length first, because the expansion can be longer than what was handed in —
    // an alias is the *short* name — and a buffer sized by guess would be a path silently cut in
    // half. The answer counts the terminator; the second call's does not.
    let needed = call(&wide, &mut []);

    if needed == 0 {
        return None;
    }

    let mut buffer = vec![0u16; usize::try_from(needed).ok()?];
    let written = usize::try_from(call(&wide, &mut buffer)).ok()?;

    // Zero is failure, and anything that did not fit means the path changed under the two calls —
    // both are "this system will not name it", which is what the caller does something about.
    if written == 0 || written >= buffer.len() {
        return None;
    }

    Some(PathBuf::from(OsString::from_wide(&buffer[..written])))
}

/// One `GetLongPathNameW`, over a buffer that may be empty.
fn call(wide: &[u16], buffer: &mut [u16]) -> u32 {
    #[expect(
        unsafe_code,
        reason = "both slices are the caller's locals and outlive the call, and the length passed \
                  is the one the buffer has"
    )]
    unsafe {
        GetLongPathNameW(
            wide.as_ptr(),
            buffer.as_mut_ptr(),
            u32::try_from(buffer.len()).unwrap_or(u32::MAX),
        )
    }
}
