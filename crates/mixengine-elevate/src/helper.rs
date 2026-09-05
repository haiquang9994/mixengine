//! Putting this binary where only an administrator can rewrite it.
//!
//! **The source is this process's own image and the destination is a compiled-in constant** — the
//! T85 design, D2. There is nothing here for a caller to aim: the operation carries no field, so a
//! compromised daemon gains no *copy this file as root* primitive from this existing. That is the
//! same reasoning `.claude/architecture/security-model.md` refuses `Exec { cmd }` with, applied to
//! an operation whose whole job is a file copy.
//!
//! **The directory is checked before it is used**, on the rule the whole binary follows.
//! [`create_root_owned_directory`] re-asserts the owner and the permissions on every call, so a
//! directory that already exists converges on the right *permissions* — but one that already exists
//! and belongs to somebody else is a target that was arranged, not a convenience, and that is what
//! ownership refuses. Its own documentation says so; this is the caller it is talking about.
//!
//! **[`install`] decides nothing about whether an upgrade deserves to be installed**, and it
//! cannot: the binary it copies is its own image, so a check it made would be a check made by the
//! thing being checked. On a machine with nothing installed that is the only candidate there is,
//! and `.claude/architecture/security-model.md` states the residual plainly rather than hiding it.
//!
//! **[`replace`] is the one that decides** — roadmap task **T88a**. It runs only as the
//! *installed* copy, in a directory an ordinary account cannot write, and it checks a detached
//! minisign signature over the candidate against a key compiled into itself. See
//! `crate::candidate`.

use std::path::{Path, PathBuf};

use mixengine_platform::elevated::{create_root_owned_directory, owner_of};
use mixengine_platform::install::{helper_path, own_as_root};
use mixengine_proto::privileged::{AT_NEXT_RESTART, OpOutcome};

/// Copy this running helper to the path this operating system keeps a privileged helper at.
pub(crate) fn install() -> OpOutcome {
    let destination = match helper_path() {
        Ok(path) => path,
        Err(error) => {
            return OpOutcome::Failed {
                message: format!("this machine will not name a directory for a helper: {error}"),
            };
        }
    };

    let source = match std::env::current_exe() {
        Ok(path) => path,
        Err(error) => {
            return OpOutcome::Failed {
                message: format!("this process cannot name its own image: {error}"),
            };
        }
    };

    // The installed helper installing itself. Nothing to copy, and on Windows nothing that *could*
    // be copied — a running image cannot be replaced in place. Compared after canonicalisation,
    // because a path reached through a symlink or an 8.3 alias is the same file spelled otherwise.
    if same_file(&source, &destination) {
        return OpOutcome::AlreadyDone;
    }

    let Some(directory) = destination.parent() else {
        return OpOutcome::Failed {
            message: format!("{} has no parent directory", destination.display()),
        };
    };

    if directory.exists() {
        match owner_of(directory) {
            Ok(owner) if owner.is_administrative() => {}
            Ok(owner) => {
                return OpOutcome::Refused {
                    reason: format!(
                        "{} already exists and belongs to {owner}, which is not an administrative \
                         account; installing a privileged helper into it would put the one file \
                         MixEngine runs as root somewhere that account can rewrite",
                        directory.display()
                    ),
                };
            }
            Err(error) => {
                return OpOutcome::Failed {
                    message: format!("cannot read who owns {}: {error}", directory.display()),
                };
            }
        }
    }

    if let Err(error) = create_root_owned_directory(directory) {
        return OpOutcome::Failed {
            message: format!("cannot create {}: {error}", directory.display()),
        };
    }

    match settled(&source, &destination) {
        Ok(true) => return OpOutcome::AlreadyDone,
        Ok(false) => {}
        Err(error) => {
            return OpOutcome::Failed {
                message: format!("cannot examine {}: {error}", destination.display()),
            };
        }
    }

    match place(&source, &destination) {
        Ok(()) => OpOutcome::Applied {
            detail: format!("installed this helper at {}", destination.display()),
        },
        Err(message) => OpOutcome::Failed { message },
    }
}

/// Take this helper off the machine — roadmap task **T87**.
///
/// The mirror of [`install`], and the reversal
/// [ADR 0015](../../../../.claude/decisions/0015-the-helper-installs-itself.md) owed uninstall: the
/// helper installs itself, so the helper is what removes itself. Where the file is, and which
/// directory is MixEngine's own, are `mixengine_platform::install`'s answers exactly as they are on
/// the way in; what is decided here is only what to call the result.
///
/// **Three outcomes, and the middle one is Windows'.** A helper that was never installed is
/// [`OpOutcome::AlreadyDone`] — an uninstall run twice must not fail the second time. A helper
/// unlinked at once is `Applied`. A helper handed to the operating system's own removal queue is
/// *also* `Applied`, with [`AT_NEXT_RESTART`] in the detail: the daemon reads that word and reports
/// the file as scheduled rather than as gone, because it is still on disk.
pub(crate) fn remove() -> OpOutcome {
    // The way back a replacement kept — roadmap task T88a. Removed here so that "nothing is left
    // behind" keeps meaning what T87 says it means: this file sits beside the helper, in the same
    // root-owned directory, and no unprivileged path can reach it. By now the process that renamed
    // itself has exited, so its image is unmapped and even Windows will unlink it.
    if let Ok(destination) = helper_path() {
        let _ = std::fs::remove_file(with_old_suffix(&destination));
    }

    let removal = match mixengine_platform::install::remove_helper() {
        Ok(removal) => removal,
        Err(error) => {
            return OpOutcome::Failed {
                message: mixengine_proto::flatten(&error),
            };
        }
    };

    if removal.is_empty() {
        return OpOutcome::AlreadyDone;
    }

    // Deliberately not two sentences joined: a system answers with one list or the other and never
    // both, so a `format!` covering both cases would be describing a state no machine produces.
    if removal.removed.is_empty() {
        return OpOutcome::Applied {
            detail: format!(
                "a running program cannot be deleted on this system, so this is scheduled for \
                 removal {AT_NEXT_RESTART}: {}",
                list(&removal.at_next_restart)
            ),
        };
    }

    OpOutcome::Applied {
        detail: format!("removed {}", list(&removal.removed)),
    }
}

/// Paths in one sentence, for a log line and for the report a person reads.
fn list(paths: &[std::path::PathBuf]) -> String {
    paths
        .iter()
        .map(|path| path.display().to_string())
        .collect::<Vec<String>>()
        .join(", ")
}

/// Copy beside the destination, then rename over it.
///
/// **Never written in place**, so a reader gets either the old helper or the new one and never half
/// of either: an interrupted copy straight onto the destination would leave the one file MixEngine
/// runs as root truncated. `fs::rename` replaces an existing file on all three systems — it is
/// `MoveFileExW` with `MOVEFILE_REPLACE_EXISTING` on Windows.
///
/// The staged file is removed on every failure after it exists, because a `.new` left in a
/// root-owned directory is litter only another elevation could clear.
fn place(source: &Path, destination: &Path) -> Result<(), String> {
    let staged = destination.with_extension("new");

    std::fs::copy(source, &staged)
        .map_err(|error| format!("cannot write {}: {error}", staged.display()))?;

    // **Before the rename and not after**, so nothing is ever reachable at the destination that is
    // not already root's — and not skipped because the copy was made by root, which on macOS is not
    // enough: `fs::copy` there carries the *source's* owner across. See `install::own_as_root`.
    if let Err(error) = own_as_root(&staged) {
        let _ = std::fs::remove_file(&staged);
        return Err(format!("cannot make {} root's: {error}", staged.display()));
    }

    if let Err(error) = std::fs::rename(&staged, destination) {
        let _ = std::fs::remove_file(&staged);
        return Err(format!(
            "cannot put {} in place: {error}",
            destination.display()
        ));
    }

    Ok(())
}

/// Are these two names the same file?
///
/// `canonicalize` on both, and a pair where either cannot be canonicalised is *not* the same file:
/// the destination usually does not exist yet, which is exactly that case.
fn same_file(one: &Path, other: &Path) -> bool {
    match (one.canonicalize(), other.canonicalize()) {
        (Ok(one), Ok(other)) => one == other,
        _ => false,
    }
}

/// Is the destination already what this operation would put there?
///
/// Three questions and not one, and the middle one is why this is not simply a byte comparison:
///
/// 1. **is it there** — a destination that is not is `Ok(false)`, there is something to do;
/// 2. **is it root's** — one that is not is `Ok(false)` too, so this operation is **its own
///    repair**. A helper somebody left owned by an ordinary account, with the right bytes in it, is
///    exactly the arrangement the root-owned directory exists to prevent; short-circuiting on the
///    bytes alone would leave it there for ever, and `elevation::helper` would go on refusing to run
///    it. That is the whole-state idiom every other privileged operation in this binary follows;
/// 3. **are the bytes the same** — length first, since a different build almost always differs in
///    it, and the contents only when the lengths agree.
fn settled(source: &Path, destination: &Path) -> Result<bool, String> {
    if !destination.exists() {
        return Ok(false);
    }

    let owner =
        owner_of(destination).map_err(|error| format!("cannot read who owns it: {error}"))?;

    if !owner.is_administrative() {
        return Ok(false);
    }

    identical(source, destination).map_err(|error| error.to_string())
}

/// Do these two files hold the same bytes?
///
/// Length first, and the contents only when the lengths agree. A destination that is not there is
/// `Ok(false)`: there is something to do.
///
/// Separate from [`settled`] so that it is testable without a token — the ownership half of that
/// question has a different answer on an elevated runner than on a developer's machine, and this
/// half has the same one everywhere.
fn identical(source: &Path, destination: &Path) -> std::io::Result<bool> {
    let Ok(there) = std::fs::metadata(destination) else {
        return Ok(false);
    };

    if there.len() != std::fs::metadata(source)?.len() {
        return Ok(false);
    }

    Ok(std::fs::read(source)? == std::fs::read(destination)?)
}

/// Replace this helper with the candidate MixEngine staged, if it deserved the prompt — roadmap
/// task **T88a**.
///
/// **Only the installed copy may do this**, and the refusal is the first thing here rather than a
/// consequence of something later. The value of the whole task is that the *trusted* copy — root's,
/// in a directory an ordinary account cannot write — is the one checking the signature; a helper
/// running out of the user's own directory checking one proves nothing, because whoever could
/// replace the helper could replace the check. On a machine with nothing installed the operation to
/// ask for is [`install`].
///
/// **Rename, then write, then rename back on failure.** A file whose image is mapped cannot be
/// unlinked or written on Windows and this process *is* that file, so the destination is renamed
/// out of the way first — which is exactly what `updates::apply::swap` does for `mix.exe`. Unix does
/// not need it and does it anyway: one code path, one set of tests, and the `.old` is the only way
/// back on the platform that has none.
///
/// The `.old` left by the *previous* replacement is removed on the way in. By then the process that
/// renamed itself has exited and its image is unmapped, so this is the first moment it can go.
pub(crate) fn replace(home: &Path) -> OpOutcome {
    let destination = match helper_path() {
        Ok(path) => path,
        Err(error) => {
            return OpOutcome::Failed {
                message: format!("this machine will not name a directory for a helper: {error}"),
            };
        }
    };

    let source = match std::env::current_exe() {
        Ok(path) => path,
        Err(error) => {
            return OpOutcome::Failed {
                message: format!("this process cannot name its own image: {error}"),
            };
        }
    };

    if !same_file(&source, &destination) {
        return OpOutcome::Refused {
            reason: format!(
                "this is not the helper installed at {}, and a copy anything running as the user \
                 could replace is not one whose signature check means anything; a machine with no \
                 installed helper wants helper-install instead",
                destination.display()
            ),
        };
    }

    let _ = std::fs::remove_file(with_old_suffix(&destination));

    let (bytes, stamp) = match crate::candidate::read_verified(
        &mixengine_proto::privileged::helper_candidate(home),
        &mixengine_proto::privileged::helper_candidate_signature(home),
        crate::candidate::PUBLIC_KEY,
        env!("CARGO_PKG_VERSION"),
    ) {
        Ok(verified) => verified,
        Err(refusal) => return refusal.into_outcome(),
    };

    // **The bytes and not the file.** `read_verified` handed back what it checked, and re-opening
    // the candidate here would be a check the caller could step past by swapping the file in
    // between — see `crate::candidate`.
    match put(&bytes, &destination) {
        Ok(()) => OpOutcome::Applied {
            detail: format!(
                "replaced this helper with MixEngine {} at {}",
                stamp.version,
                destination.display()
            ),
        },
        Err(message) => OpOutcome::Failed { message },
    }
}

/// Move the installed helper aside, write the verified bytes under its name, and undo both on
/// failure.
fn put(bytes: &[u8], destination: &Path) -> Result<(), String> {
    let old = with_old_suffix(destination);
    let staged = destination.with_extension("new");

    std::fs::rename(destination, &old).map_err(|error| {
        format!(
            "cannot move {} out of the way: {error}",
            destination.display()
        )
    })?;

    let written = std::fs::write(&staged, bytes)
        .map_err(|error| format!("cannot write {}: {error}", staged.display()))
        // **Before the rename and not after**, and not skipped because the writer is root: on macOS
        // a file carries the creating process's owner, which `install::own_as_root` exists to
        // correct. `place` does the same in the same order and for the same reason.
        .and_then(|()| {
            own_as_root(&staged)
                .map_err(|error| format!("cannot make {} root's: {error}", staged.display()))
        })
        .and_then(|()| {
            std::fs::rename(&staged, destination)
                .map_err(|error| format!("cannot put {} in place: {error}", destination.display()))
        });

    if let Err(error) = written {
        let _ = std::fs::remove_file(&staged);

        if let Err(back) = std::fs::rename(&old, destination) {
            // Nothing left to try, and both halves are what somebody needs: this machine now has
            // its helper under a name no elevation prompt will look for.
            return Err(format!(
                "{error}; and the helper could not be put back under its own name either: {back}"
            ));
        }

        return Err(error);
    }

    Ok(())
}

/// `mixengine-elevate.exe` becomes `mixengine-elevate.exe.old`.
///
/// Appended rather than substituted, on `updates::apply::with_old_suffix`'s rule: the name says
/// which file it came from, and it is not something Windows will start by accident.
fn with_old_suffix(path: &Path) -> PathBuf {
    let mut name = path.to_path_buf().into_os_string();
    name.push(".old");
    PathBuf::from(name)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The comparison is by bytes and not by length, and a file that is not there is a difference.
    ///
    /// Written against a temporary directory rather than against the real destination, which no
    /// unit test may touch: what is under test here is the predicate, and the operation that uses
    /// it is in `tests/system.rs` under a real token.
    #[test]
    fn two_files_are_identical_only_when_their_bytes_are() {
        let directory = tempfile::tempdir().expect("a temporary directory");
        let one = directory.path().join("one");
        let same = directory.path().join("same");
        let longer = directory.path().join("longer");
        let differing = directory.path().join("differing");

        std::fs::write(&one, b"aaaa").expect("a file");
        std::fs::write(&same, b"aaaa").expect("a file");
        std::fs::write(&longer, b"aaaaa").expect("a file");
        std::fs::write(&differing, b"aaab").expect("a file");

        assert!(identical(&one, &same).expect("both read"));
        assert!(!identical(&one, &longer).expect("both read"));
        assert!(!identical(&one, &differing).expect("both read"));
        assert!(
            !identical(&one, &directory.path().join("absent"))
                .expect("an absent destination is an answer, not a failure")
        );
    }

    /// The suffix is appended and never substituted, so the name says which file it came from and
    /// so Windows will not start it by accident — `updates::apply::with_old_suffix`'s rule, one
    /// directory along.
    #[test]
    fn the_way_back_is_named_after_the_file_it_came_from() {
        assert_eq!(
            with_old_suffix(Path::new("mixengine-elevate")),
            PathBuf::from("mixengine-elevate.old")
        );
        assert_eq!(
            with_old_suffix(Path::new("mixengine-elevate.exe")),
            PathBuf::from("mixengine-elevate.exe.old")
        );
    }

    /// One name and a second name for the same bytes are not the same *file*, and the check has to
    /// tell them apart: it is what stops the installed helper from trying to copy over itself.
    #[test]
    fn a_second_copy_of_the_same_bytes_is_not_the_same_file() {
        let directory = tempfile::tempdir().expect("a temporary directory");
        let one = directory.path().join("one");
        let two = directory.path().join("two");

        std::fs::write(&one, b"aaaa").expect("a file");
        std::fs::write(&two, b"aaaa").expect("a file");

        assert!(same_file(&one, &one));
        assert!(!same_file(&one, &two));
        assert!(!same_file(&one, &directory.path().join("absent")));
    }
}
