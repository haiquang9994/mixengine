//! A path spelled the way the filesystem itself spells it.

use std::path::{Path, PathBuf};

/// `path`, with every component the filesystem knows a longer name for replaced by that name.
///
/// **This exists because one managed program refuses the alternative.** Windows keeps an 8.3 alias
/// for most names — `C:\Users\RUNNER~1` beside `C:\Users\runneradmin` — and both open the same
/// directory, so nothing in this workspace had a reason to prefer one. nginx does: every file it
/// opens for reading goes through `ngx_win32_check_filename`, which expands the name it was given
/// and **reports `ENOENT` when the expansion differs from what it was handed**. A home under an 8.3
/// alias is therefore a home whose every rendered configuration is "the system cannot find the
/// file specified", for a file that is sitting right there. `%TEMP%` on a GitHub Windows runner is
/// exactly such a path, which is how this was found.
///
/// Applied to the root and nowhere else: `etc/`, `data/`, `packages/` and the rest are all joined
/// onto it, so one spelling at the top is the whole of it.
///
/// **A component the filesystem cannot name is left exactly as it came.** A root is resolved before
/// it is created, so the leaf usually does not exist yet — and a name that is not there has no
/// alias to expand, which makes the answer the same either way. That matters more than tidiness:
/// `mix` and `mixengined` both derive the endpoint from this, and a spelling that changed the first
/// time the directory appeared would be two processes disagreeing about which home they are in.
#[must_use]
pub fn in_full(path: &Path) -> PathBuf {
    #[cfg(windows)]
    {
        crate::sys::fullname::in_full(path)
    }

    #[cfg(not(windows))]
    {
        // Every other system has one name per file.
        path.to_path_buf()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A directory that is really there, under a name long enough to have an alias.
    fn somewhere() -> tempfile::TempDir {
        tempfile::Builder::new()
            .prefix("mixengine-spelling-check")
            .tempdir()
            .expect("a temporary directory")
    }

    #[test]
    fn a_path_that_is_not_there_is_handed_back_as_it_came() {
        let home = somewhere();
        let missing = home.path().join("not-created-yet").join("nor-this");

        assert_eq!(in_full(&missing), missing);
    }

    #[test]
    fn spelling_a_path_that_is_already_spelled_in_full_changes_nothing() {
        let home = somewhere();
        let spelled = in_full(home.path());

        assert_eq!(in_full(&spelled), spelled);
    }

    /// The whole point on this system: the alias and the name resolve to one spelling.
    #[cfg(windows)]
    #[test]
    fn an_8_dot_3_alias_comes_back_as_the_name_it_stands_for() {
        let home = somewhere();
        let inside = home.path().join("a-directory-nobody-would-call-short");
        std::fs::create_dir(&inside).expect("a directory");

        let Some(alias) = alias(&inside) else {
            // 8.3 generation is per-volume and can be turned off. Nothing to compare on a machine
            // where the filesystem keeps one name per file, which is the state this asks about.
            return;
        };

        assert_ne!(
            alias, inside,
            "the alias is the name, so this proves nothing"
        );
        assert_eq!(in_full(&alias), in_full(&inside));
        assert!(
            !in_full(&alias).to_string_lossy().contains('~'),
            "{alias:?}"
        );
    }

    /// `path` as its 8.3 alias, or `None` where the volume keeps none.
    #[cfg(windows)]
    fn alias(path: &Path) -> Option<PathBuf> {
        use std::os::windows::ffi::{OsStrExt as _, OsStringExt as _};

        let wide: Vec<u16> = path.as_os_str().encode_wide().chain(Some(0)).collect();
        let mut short = vec![0u16; 1024];

        #[expect(
            unsafe_code,
            reason = "both buffers are locals of this frame, and the length passed is the one \
                      allocated"
        )]
        let written = unsafe {
            windows_sys::Win32::Storage::FileSystem::GetShortPathNameW(
                wide.as_ptr(),
                short.as_mut_ptr(),
                u32::try_from(short.len()).unwrap_or(u32::MAX),
            )
        };

        let written = usize::try_from(written).ok()?;

        if written == 0 || written >= short.len() {
            return None;
        }

        let spelled = PathBuf::from(std::ffi::OsString::from_wide(&short[..written]));

        (spelled != path).then_some(spelled)
    }
}
