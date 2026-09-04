//! Whether this copy of MixEngine may replace itself — roadmap task **T88**, the design's D7.
//!
//! **A write probe and not a path table.** A list of "system" prefixes would be per-OS knowledge in
//! this crate, which `CLAUDE.md` forbids, and would be wrong for anybody who installed somewhere
//! unusual. Asking the actual machine covers the four ways of installing that must be refused
//! without a stack trace — `/usr/bin` from a `.deb` or an `.rpm`, `/usr/local/bin` from a `.pkg`,
//! and a read-only AppImage mount — and costs one file created and removed.
//!
//! **Nothing here ever elevates, and nothing here ever will.** An updater that could ask for root
//! would be the local privilege-escalation path this whole feature is written to avoid —
//! `.claude/features/updates.md`, and [ADR 0005](../../../../.claude/decisions/0005-on-demand-elevation.md).

use std::ffi::OsStr;
use std::path::{Path, PathBuf};

/// What the probe writes, and removes.
///
/// Dotted and product-named rather than random, because the failure mode worth designing for is
/// somebody finding it beside their binaries and wondering what wrote it.
pub const PROBE_FILE: &str = ".mixengine-update-probe";

/// Where this build is installed, and what that means for updating it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Placement {
    /// MixEngine may replace its own binaries here.
    SelfUpdatable {
        /// The directory holding them.
        directory: PathBuf,
    },

    /// Something else installed this copy, and something else updates it.
    Managed {
        /// The directory holding them, or the AppImage's own path.
        directory: PathBuf,

        /// Why, phrased for a person and rendered by a client unchanged.
        ///
        /// **Never a package-manager command.** Which of `apt`, `dnf` or `brew` put a binary in a
        /// directory is per-OS knowledge this crate may not hold, and a guess printed as an
        /// instruction is worse than the fact. What can be said honestly is that the directory is
        /// not this account's to write, and therefore that MixEngine did not put itself there.
        because: String,
    },
}

/// Read the placement of the daemon at `daemon_exe`.
///
/// `appimage` is `APPIMAGE` from the environment, passed in rather than read here: configuration
/// enters at `main` and is passed down (`.claude/standards/rust.md`), and it is also what lets this
/// be tested without `set_var`, which is `unsafe` in edition 2024 and process-global regardless.
///
/// **The AppImage question is asked before the probe**, which matters for the case that is not the
/// obvious one: the mount is read-only, so a probe would refuse it anyway — but an AppImage a user
/// extracted by hand into a writable directory would *pass* a write probe while still being one
/// file somebody placed rather than a directory of binaries an updater may replace one at a time.
#[must_use]
pub fn of(daemon_exe: &Path, appimage: Option<&OsStr>) -> Placement {
    let Some(directory) = daemon_exe
        .parent()
        .filter(|parent| !parent.as_os_str().is_empty())
    else {
        return Placement::Managed {
            directory: daemon_exe.to_path_buf(),
            because: format!(
                "{} is not in a directory this daemon can name, so nothing here can replace it",
                daemon_exe.display()
            ),
        };
    };

    if let Some(appimage) = appimage {
        return Placement::Managed {
            directory: PathBuf::from(appimage),
            because:
                "this is a running AppImage: it is one file somebody placed, and replacing it \
                      is theirs to do"
                    .to_owned(),
        };
    }

    match probe(directory) {
        Ok(()) => Placement::SelfUpdatable {
            directory: directory.to_path_buf(),
        },
        Err(source) => Placement::Managed {
            directory: directory.to_path_buf(),
            because: format!(
                "this account cannot write to {}, so MixEngine was installed by something else and \
                 that is what updates it ({source})",
                directory.display()
            ),
        },
    }
}

/// Create a file in `directory` and remove it again.
///
/// # Errors
///
/// Whatever the file system said, which is the sentence [`Placement::Managed`] carries.
pub fn probe(directory: &Path) -> std::io::Result<()> {
    let path = directory.join(PROBE_FILE);

    // A copy left by a daemon killed between the write and the removal must not be able to make a
    // directory look occupied, and must not accumulate. Ignored when there is nothing there, which
    // is every ordinary run.
    let _ = std::fs::remove_file(&path);

    std::fs::write(&path, b"")?;
    std::fs::remove_file(&path)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_directory_this_account_can_write_is_self_updatable() {
        let directory = tempfile::tempdir().expect("a temporary directory");
        let exe = directory.path().join("mixengined");

        assert!(matches!(of(&exe, None), Placement::SelfUpdatable { .. }));
    }

    #[test]
    fn an_appimage_is_managed_however_writable_its_directory_is() {
        let directory = tempfile::tempdir().expect("a temporary directory");
        let exe = directory.path().join("mixengined");

        let placement = of(&exe, Some(OsStr::new("/home/x/MixEngine.AppImage")));

        let Placement::Managed { because, .. } = placement else {
            panic!(
                "an AppImage is managed however writable the directory it was extracted into is"
            );
        };
        assert!(because.contains("AppImage"), "{because}");
    }

    /// A daemon whose own directory cannot be named. Refused in words rather than unwrapped.
    #[test]
    fn a_binary_with_no_directory_at_all_is_managed() {
        let missing = Path::new("mixengined");

        assert!(matches!(of(missing, None), Placement::Managed { .. }));
    }

    #[test]
    fn the_probe_leaves_nothing_behind() {
        let directory = tempfile::tempdir().expect("a temporary directory");

        probe(directory.path()).expect("this account can write here");

        assert!(
            !directory.path().join(PROBE_FILE).exists(),
            "the probe file is removed by the function that made it"
        );
    }

    /// A probe file a killed daemon left is deleted rather than believed: it must not be able to
    /// make a directory look occupied, and it must not accumulate.
    #[test]
    fn a_probe_file_left_by_a_previous_run_is_removed() {
        let directory = tempfile::tempdir().expect("a temporary directory");
        std::fs::write(directory.path().join(PROBE_FILE), b"stale").expect("a stale probe");

        probe(directory.path()).expect("a stale probe does not refuse a writable directory");

        assert!(!directory.path().join(PROBE_FILE).exists());
    }
}
