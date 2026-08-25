//! Where the NSS databases are, on a machine that has any.
//!
//! **Six roots where `.claude/features/tls.md` names two, and the three extra ones are the
//! measurement this task started from** — the T49b design, D3. On Ubuntu 22.04 and later the
//! `firefox` deb is a transitional package to the snap, whose profiles live under `~/snap`, so a
//! faithful implementation of that table finds nothing on the distribution most people run and
//! reports success.
//!
//! **The list is generous on purpose.** A root that matches nothing costs a `read_dir` that returns
//! `ENOENT`; a root that is missing costs a user a red padlock with no diagnostic, in the browser
//! they actually use. The asymmetry is the whole argument.

use std::path::{Path, PathBuf};

/// What a database directory is called on disk, and the only format this build reads — D5.
///
/// The legacy Berkeley-DB pair is `cert8.db` and is deliberately not found: Firefox has written
/// this one since version 58, and a legacy branch would be a second code path nothing here can test
/// and no supported distribution produces.
const MARKER: &str = "cert9.db";

/// One place to look, relative to the user's home directory.
struct Root {
    /// Slash-separated, with at most one `*` segment — a `*` matches one directory name and never a
    /// subtree, because a profile directory is named for a random token and sits exactly one level
    /// under `firefox/`.
    pattern: &'static str,

    /// What put it there, for the screen.
    owner: &'static str,
}

/// Every place a database has been found to live — D3.
const ROOTS: &[Root] = &[
    Root {
        pattern: ".pki/nssdb",
        owner: "Chrome and Chromium",
    },
    Root {
        pattern: ".mozilla/firefox/*",
        owner: "Firefox",
    },
    Root {
        pattern: "snap/firefox/common/.mozilla/firefox/*",
        owner: "Firefox (snap)",
    },
    Root {
        pattern: "snap/chromium/common/chromium",
        owner: "Chromium (snap)",
    },
    Root {
        pattern: ".var/app/org.mozilla.firefox/.mozilla/firefox/*",
        owner: "Firefox (flatpak)",
    },
    Root {
        pattern: ".var/app/com.google.Chrome/.pki/nssdb",
        owner: "Chrome (flatpak)",
    },
];

/// One NSS database, as found on disk.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Database {
    /// The directory itself. `certutil` is given `sql:` and this.
    pub directory: PathBuf,

    /// What put it there: `Firefox`, `Firefox (snap)`, `Chrome and Chromium`.
    pub owner: &'static str,
}

/// Every NSS database under `home`, in the order this module searches: Chrome, Firefox, then their snap and flatpak packagings.
///
/// **A directory that exists is not a database**; one holding `cert9.db` is. A Firefox profile with
/// no `cert9.db` has never been opened, and MixEngine does not create one — D4: a database nobody
/// asked for is a file in the user's home that the browser may replace and that nothing removes.
///
/// Takes the home directory rather than asking for it, which is what makes every root testable on a
/// machine that has none of them — and what lets `tests/browsers.rs` point the whole search at a
/// temp directory instead of at a real profile.
#[must_use]
pub fn databases_under(home: &Path) -> Vec<Database> {
    let mut found = Vec::new();

    for root in ROOTS {
        for directory in expand(home, root.pattern) {
            if directory.join(MARKER).is_file() {
                found.push(Database {
                    directory,
                    owner: root.owner,
                });
            }
        }
    }

    found
}

/// Resolve one pattern against `home`, expanding its single `*` segment if it has one.
///
/// **Hand-written rather than a glob crate**, on this workspace's rule that a dependency is a
/// decision somebody argues for: what is needed here is one wildcard segment, and the whole of it
/// is the loop below.
///
/// A pattern with *two* `*` segments would multiply out rather than nest — none of [`ROOTS`] has
/// two, and a seventh that needed them would have to change this.
fn expand(home: &Path, pattern: &str) -> Vec<PathBuf> {
    let mut paths = vec![home.to_path_buf()];

    for segment in pattern.split('/') {
        if segment == "*" {
            paths = paths
                .iter()
                .filter_map(|path| std::fs::read_dir(path).ok())
                .flatten()
                .filter_map(Result::ok)
                .map(|entry| entry.path())
                .filter(|path| path.is_dir())
                .collect();
        } else {
            for path in &mut paths {
                path.push(segment);
            }
        }
    }

    paths
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Make a database at `relative` under `home`, the way a browser's first run would.
    fn database(home: &std::path::Path, relative: &str) {
        let directory = home.join(relative);
        std::fs::create_dir_all(&directory).expect("the fixture directory is made");
        std::fs::write(directory.join("cert9.db"), b"not really a database")
            .expect("the fixture file is written");
    }

    /// The root `.claude/features/tls.md` does not have, and the one most Ubuntu desktops use.
    #[test]
    fn a_firefox_snap_profile_is_found() {
        let home = tempfile::tempdir().expect("a temp home");
        database(
            home.path(),
            "snap/firefox/common/.mozilla/firefox/a1b2c3d4.default",
        );

        let found = databases_under(home.path());

        assert_eq!(found.len(), 1, "found {found:?}");
        assert_eq!(found[0].owner, "Firefox (snap)");
        assert_eq!(
            found[0].directory,
            home.path()
                .join("snap/firefox/common/.mozilla/firefox/a1b2c3d4.default")
        );
    }

    /// Every root in the table resolves, and each is labelled with what put it there.
    #[test]
    fn every_root_is_found_and_named() {
        let home = tempfile::tempdir().expect("a temp home");

        for relative in [
            ".pki/nssdb",
            ".mozilla/firefox/one.default",
            "snap/firefox/common/.mozilla/firefox/two.default",
            "snap/chromium/common/chromium",
            ".var/app/org.mozilla.firefox/.mozilla/firefox/three.default",
            ".var/app/com.google.Chrome/.pki/nssdb",
        ] {
            database(home.path(), relative);
        }

        let found = databases_under(home.path());

        assert_eq!(found.len(), 6, "found {found:?}");
        assert!(
            found.iter().all(|one| !one.owner.is_empty()),
            "a database with no owner: {found:?}"
        );
    }

    /// A profile the browser has never opened has no `cert9.db`, and writing into one would make a
    /// database the browser may later replace — D4.
    #[test]
    fn a_profile_without_cert9_is_not_a_database() {
        let home = tempfile::tempdir().expect("a temp home");
        std::fs::create_dir_all(home.path().join(".mozilla/firefox/empty.default"))
            .expect("the fixture directory is made");

        assert!(databases_under(home.path()).is_empty());
    }

    /// The legacy format is not found — D5. Firefox has written `cert9.db` since version 58.
    #[test]
    fn a_legacy_database_is_not_found() {
        let home = tempfile::tempdir().expect("a temp home");
        let directory = home.path().join(".mozilla/firefox/old.default");
        std::fs::create_dir_all(&directory).expect("the fixture directory is made");
        std::fs::write(directory.join("cert8.db"), b"legacy").expect("the fixture file is written");

        assert!(databases_under(home.path()).is_empty());
    }

    /// A home with none of them is an empty list, never an error — the state of every machine that
    /// has no browser, which is most servers.
    #[test]
    fn a_home_with_no_browser_finds_nothing() {
        let home = tempfile::tempdir().expect("a temp home");

        assert!(databases_under(home.path()).is_empty());
    }

    /// A `*` matches one segment, so a database nested a level deeper is not swept in.
    #[test]
    fn a_star_matches_one_segment_and_not_a_subtree() {
        let home = tempfile::tempdir().expect("a temp home");
        database(home.path(), ".mozilla/firefox/one.default/nested");

        assert!(databases_under(home.path()).is_empty());
    }

    /// A file where a directory was expected is skipped rather than panicking — a home can hold
    /// anything, and discovery runs on every daemon start.
    #[test]
    fn a_file_in_place_of_a_profile_directory_is_skipped() {
        let home = tempfile::tempdir().expect("a temp home");
        std::fs::create_dir_all(home.path().join(".mozilla")).expect("the fixture directory");
        std::fs::write(
            home.path().join(".mozilla/firefox"),
            b"a file, not a directory",
        )
        .expect("the fixture file is written");

        assert!(databases_under(home.path()).is_empty());
    }
}
