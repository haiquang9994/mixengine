//! Putting a release on this machine — roadmap task **T88**, the design's D3, D8 and D11.
//!
//! # Two halves, and only the second one is new
//!
//! **Staging is an install**, and [`crate::install::Installer`] is the code that does installs here.
//! The feed's artifact *is* an [`Artifact`], which is what makes that literally true rather than
//! nearly: the resumable `.part` file, the SHA-256 the signed document carries, the archive entry
//! that would escape its root, the `provides` the payload promised, and the staged `mixengined` run
//! before anything is replaced — all of it is already written, already tested, and already the code
//! path every runtime this product installs goes through. So [`stage`] is a call and not an
//! implementation.
//!
//! **The swap is the part an install never has to do**, because an install writes somewhere nothing
//! is running from. This one replaces the binaries of the process performing it, which is why it is
//! rename-then-write and why it undoes itself.
//!
//! # What is never swapped
//!
//! `mixengine-elevate`. It is installed once to a root-owned location, and replacing it needs its
//! own elevation prompt with a minisign check performed *inside* the elevated context — roadmap task
//! **T88a**. `.claude/features/updates.md` calls this the single most important rule on the page: an
//! auto-updated binary that runs as root, with no OS signature, is a local privilege-escalation
//! vector. Here it is one name in one constant, [`KEPT`], and a test that a payload containing the
//! helper does not get to replace it.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use crate::index::Artifact;
use crate::install::{Installer, NotAnArchive, SmokeTest, Watcher};
use crate::{Error, Result};

/// The binary an update never replaces.
pub const KEPT: &str = "mixengine-elevate";

/// The executable the smoke test runs, by the name the payload publishes it under.
///
/// Public because it is a name `packaging/` has to keep: a payload whose `provides` does not carry
/// this key is one [`stage`] refuses with [`Error::MissingFromArtifact`], which is what
/// `crates/mixengine-core/tests/packaging.rs` checks the release list against.
pub const SMOKE_EXECUTABLE: &str = "mixengined";

/// What is renamed onto a binary before its replacement is written.
///
/// Removed by the next daemon start that succeeds, which gives the property that matters for free:
/// these survive exactly as long as they are the only way back, and a daemon that comes up has
/// proved they are not needed.
pub const OLD_SUFFIX: &str = ".old";

/// What one swap did.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Swapped {
    /// The binaries that were replaced, by name.
    pub replaced: Vec<String>,

    /// The binaries the payload carried that this update deliberately did not replace.
    ///
    /// [`KEPT`] always, when the payload has it; and anything else the payload carries that this
    /// install does not have — which is how a release that gains a binary behaves against an
    /// install predating it.
    pub kept: Vec<String>,
}

/// Download, verify, unpack and smoke-test a payload, leaving it under `into`.
///
/// **Every step is [`Installer::install`]'s**, which is what makes this function short. The one
/// worth naming on its own is the last: running the staged `mixengined` before anything is replaced
/// is the difference between *"the update was refused and nothing changed"* and *"MixEngine no
/// longer starts"*. `.claude/features/updates.md` records that Windows Code Integrity judges each
/// file separately, again after every update, with refusal rather than a warning at the end of it —
/// and a payload for the wrong architecture or past this machine's glibc floor fails here too.
///
/// A staging directory left by an attempt that was killed is removed first: [`Installer::install`]
/// refuses a destination that exists, and what is in one is the wrong half of an archive nobody
/// wants.
///
/// # Errors
///
/// [`Error::ArtifactTransport`] and its neighbours from the download, [`Error::ArtifactChecksum`]
/// when what arrived is not what the signed feed promised, [`Error::UnsafeArchiveEntry`] from
/// unpacking, [`Error::MissingFromArtifact`] when the payload does not hold what it declared,
/// [`Error::SmokeTestFailed`] when the staged daemon will not run here, and [`Error::Io`] for the
/// staging directory itself. Every one of them leaves the installed binaries untouched.
pub async fn stage<W: Watcher>(
    installer: &Installer,
    artifact: &Artifact,
    into: &Path,
    watcher: &W,
) -> Result<PathBuf> {
    if into.exists() {
        tokio::fs::remove_dir_all(into)
            .await
            .map_err(|source| Error::Io {
                action: "remove the staging directory left by a previous update",
                path: into.to_path_buf(),
                source,
            })?;
    }

    if let Some(parent) = into.parent() {
        crate::paths::create_dir(parent)?;
    }

    let smoke = SmokeTest {
        executable: SMOKE_EXECUTABLE.to_owned(),
        args: vec!["--version".to_owned()],
    };

    let installed = installer
        .install(artifact, into, Some(&smoke), NotAnArchive::Refuse, watcher)
        .await?;

    Ok(installed.path)
}

/// Replace the installed binaries with the staged ones, or put everything back.
///
/// `provides` is the payload's own map of executable name to path inside the archive — the payload's
/// contents and not a list compiled into this binary, which is what lets an installed 0.2.0 take a
/// 0.3.0 payload that carries a binary 0.2.0 never had.
///
/// Three rules, in this order, per name:
///
/// 1. [`KEPT`] is skipped and reported as kept.
/// 2. A name this install does not have is skipped and reported as kept. Nothing is *added* by an
///    update: a binary appearing for the first time is an install's business, not an update's.
/// 3. Otherwise `rename(target, target.old)` and then copy the staged file to `target`.
///
/// **Rename rather than overwrite**, which is what makes this work at all on Windows: the running
/// `mix.exe` is one of the files being replaced, an open image cannot be deleted or written, and it
/// *can* be renamed — after which the freed name accepts the new file. On Unix an overwrite would
/// also be safe, and doing it the same way on both keeps one code path and one set of tests.
///
/// **Copy and not rename from the staging directory**: the cache is inside `MIXENGINE_HOME` and the
/// install directory need not be on the same volume.
///
/// # Errors
///
/// [`Error::Io`] for the first rename or copy that fails — and every rename made before it is undone
/// first, so a partial swap is never left behind. What this cannot undo is the *stop* that preceded
/// it, which is why the caller starts the services again on this path.
pub fn swap(
    staged: &Path,
    provides: &BTreeMap<String, String>,
    directory: &Path,
) -> Result<Swapped> {
    let mut swapped = Swapped::default();
    // What has been renamed, so a failure part way through can put it back. In order, and undone in
    // reverse, which costs nothing and is what a reader expects of an unwind.
    let mut renamed: Vec<(PathBuf, PathBuf)> = Vec::new();

    for (name, relative) in provides {
        if name == KEPT {
            swapped.kept.push(name.clone());
            continue;
        }

        let target = directory.join(binary_name(name));

        if !target.exists() {
            swapped.kept.push(name.clone());
            continue;
        }

        let old = with_old_suffix(&target);
        let source = staged.join(relative);

        // A `.old` from an update whose daemon never came up. Removed rather than refused: it is
        // the one thing in the way, and the copy about to be made is the way back from here.
        let _ = std::fs::remove_file(&old);

        if let Err(error) = replace(&source, &target, &old) {
            unwind(&renamed);
            return Err(error);
        }

        renamed.push((target, old));
        swapped.replaced.push(name.clone());
    }

    Ok(swapped)
}

/// Remove the `.old` files a completed update left beside the binaries it replaced.
///
/// Called by the **next daemon start that succeeds**, which is what makes it safe: a daemon that is
/// answering has proved the binaries beside these are the ones this machine runs. Failures are
/// reported as a count and never as an error — on Windows a `mix.exe.old` is still held open by the
/// `mix` that ran the update, and it goes at the start after that one.
#[must_use]
pub fn discard_old(directory: &Path, names: &[String]) -> usize {
    names
        .iter()
        .filter(|name| {
            let old = with_old_suffix(&directory.join(binary_name(name)));

            old.exists() && std::fs::remove_file(&old).is_ok()
        })
        .count()
}

/// One binary's swap: rename the installed file out of the way, then write the new one.
fn replace(source: &Path, target: &Path, old: &Path) -> Result<()> {
    std::fs::rename(target, old).map_err(|source| Error::Io {
        action: "rename the installed binary out of the way",
        path: target.to_path_buf(),
        source,
    })?;

    let copied = std::fs::copy(source, target).map_err(|error| Error::Io {
        action: "copy the staged binary into place",
        path: target.to_path_buf(),
        source: error,
    });

    if let Err(error) = copied {
        // The rename this function made, undone by this function: the caller's unwind covers the
        // ones made before it, and leaving a half-done name for it to guess at would be worse.
        let _ = std::fs::rename(old, target);
        return Err(error);
    }

    mixengine_platform::install::make_executable(target).map_err(|error| Error::Io {
        action: "make the replacement executable",
        path: target.to_path_buf(),
        source: std::io::Error::other(error.to_string()),
    })
}

/// Put back everything a failed swap had already moved.
fn unwind(renamed: &[(PathBuf, PathBuf)]) {
    for (target, old) in renamed.iter().rev() {
        // The new file is in the way of its own predecessor, and it is the thing being abandoned.
        let _ = std::fs::remove_file(target);

        if let Err(error) = std::fs::rename(old, target) {
            // Nothing left to try, and a warning is the only honest thing: the caller is about to
            // report the failure that started this, and the path is what somebody needs.
            tracing::warn!(
                path = %target.display(),
                kept = %old.display(),
                %error,
                "an update that was rolled back could not put a binary back under its own name"
            );
        }
    }
}

/// What a binary is called on this system.
fn binary_name(name: &str) -> String {
    format!("{name}{}", std::env::consts::EXE_SUFFIX)
}

/// `mix.exe` → `mix.exe.old`.
///
/// Appended rather than substituted, so `mix.exe.old` is not something Windows will start by
/// accident and so the name says which file it came from.
fn with_old_suffix(path: &Path) -> PathBuf {
    let mut name = path.to_path_buf().into_os_string();
    name.push(OLD_SUFFIX);
    PathBuf::from(name)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A staged payload and an install directory, each holding the named binaries.
    fn layout(
        staged_names: &[&str],
        installed_names: &[&str],
    ) -> (tempfile::TempDir, PathBuf, PathBuf) {
        let root = tempfile::tempdir().expect("a temporary directory");
        let staged = root.path().join("staged");
        let installed = root.path().join("installed");
        std::fs::create_dir_all(staged.join("mixengine")).expect("a staging directory");
        std::fs::create_dir_all(&installed).expect("an install directory");

        for name in staged_names {
            std::fs::write(staged.join("mixengine").join(binary_name(name)), b"new")
                .expect("a staged file");
        }
        for name in installed_names {
            std::fs::write(installed.join(binary_name(name)), b"old").expect("an installed file");
        }

        (root, staged, installed)
    }

    /// The map the feed carries, as `packaging/feed.sh` computes it from the archive.
    fn provides(names: &[&str]) -> BTreeMap<String, String> {
        names
            .iter()
            .map(|name| {
                (
                    (*name).to_owned(),
                    format!("mixengine/{}", binary_name(name)),
                )
            })
            .collect()
    }

    #[test]
    fn the_swap_replaces_what_is_installed_and_keeps_the_old_file_beside_it() {
        let (_root, staged, installed) = layout(&["mix", "mixengined"], &["mix", "mixengined"]);

        let swapped = swap(&staged, &provides(&["mix", "mixengined"]), &installed).expect("a swap");

        assert_eq!(
            swapped.replaced,
            vec!["mix".to_owned(), "mixengined".to_owned()]
        );
        assert_eq!(
            std::fs::read(installed.join(binary_name("mix"))).expect("the new file"),
            b"new"
        );
        assert_eq!(
            std::fs::read(with_old_suffix(&installed.join(binary_name("mix"))))
                .expect("the old file"),
            b"old"
        );
    }

    /// `.claude/features/updates.md`'s single most important rule, as a test: an auto-updated binary
    /// that runs as root, with no OS signature, is a local privilege-escalation vector.
    #[test]
    fn the_elevated_helper_is_never_replaced_and_is_reported_as_kept() {
        let (_root, staged, installed) = layout(&["mix", KEPT], &["mix", KEPT]);

        let swapped = swap(&staged, &provides(&["mix", KEPT]), &installed).expect("a swap");

        assert_eq!(swapped.kept, vec![KEPT.to_owned()]);
        assert_eq!(
            std::fs::read(installed.join(binary_name(KEPT))).expect("the helper"),
            b"old",
            "the helper this install already had is the helper it still has"
        );
        assert!(!with_old_suffix(&installed.join(binary_name(KEPT))).exists());
    }

    /// A payload that gained a binary against an install that does not have it yet — which is how
    /// `mixengine-shim` behaves the day T85c is done.
    #[test]
    fn a_binary_this_install_does_not_have_is_left_alone() {
        let (_root, staged, installed) = layout(&["mix", "mixengine-shim"], &["mix"]);

        let swapped =
            swap(&staged, &provides(&["mix", "mixengine-shim"]), &installed).expect("a swap");

        assert_eq!(swapped.replaced, vec!["mix".to_owned()]);
        assert_eq!(swapped.kept, vec!["mixengine-shim".to_owned()]);
        assert!(!installed.join(binary_name("mixengine-shim")).exists());
    }

    /// The rollback. A staged directory missing its second file fails half way, and everything the
    /// first half moved comes back.
    #[test]
    fn a_swap_that_fails_half_way_puts_everything_back() {
        let (_root, staged, installed) = layout(&["mix"], &["mix", "mixengined"]);

        swap(&staged, &provides(&["mix", "mixengined"]), &installed)
            .expect_err("the payload has no mixengined to copy");

        assert_eq!(
            std::fs::read(installed.join(binary_name("mix"))).expect("the old file, back"),
            b"old"
        );
        assert!(
            !with_old_suffix(&installed.join(binary_name("mix"))).exists(),
            "the rename was undone rather than left for somebody to find"
        );
        assert_eq!(
            std::fs::read(installed.join(binary_name("mixengined"))).expect("untouched"),
            b"old"
        );
    }

    /// A `.old` from an update whose daemon never came up must not stop the next attempt.
    #[test]
    fn a_leftover_old_file_does_not_refuse_the_next_swap() {
        let (_root, staged, installed) = layout(&["mix"], &["mix"]);
        std::fs::write(
            with_old_suffix(&installed.join(binary_name("mix"))),
            b"older",
        )
        .expect("a leftover");

        swap(&staged, &provides(&["mix"]), &installed).expect("a swap");

        assert_eq!(
            std::fs::read(with_old_suffix(&installed.join(binary_name("mix"))))
                .expect("the old file"),
            b"old",
            "the file replaced by this swap, not the one left by the last"
        );
    }

    #[test]
    fn the_old_files_of_a_finished_update_are_discarded() {
        let (_root, staged, installed) = layout(&["mix"], &["mix"]);
        let swapped = swap(&staged, &provides(&["mix"]), &installed).expect("a swap");

        assert_eq!(discard_old(&installed, &swapped.replaced), 1);
        assert!(!with_old_suffix(&installed.join(binary_name("mix"))).exists());
        assert_eq!(
            discard_old(&installed, &swapped.replaced),
            0,
            "a second start has nothing left to discard, and says so rather than failing"
        );
    }
}
