//! Filling `<root>/bin` — roadmap task **T26**.
//!
//! What is proved here is the *arithmetic* of a refresh: which files appear, which are left alone,
//! which are replaced and which are swept away. That a shim copied under a name then dispatches on
//! it is `crates/mixengine-shim/tests/shim.rs`', which runs the real binary; this suite copies a
//! few bytes instead, because nothing it asks depends on the copy being a program.

use std::path::{Path, PathBuf};

use mixengine_core::shims;

/// A `bin/` and a file standing in for the shim binary, in a directory this test owns.
struct Fixture {
    root: tempfile::TempDir,
}

impl Fixture {
    fn new() -> Self {
        let root = tempfile::tempdir().expect("a temporary directory");
        let fixture = Self { root };

        fixture.publish(b"the shim, build one");
        fixture
    }

    /// (Re)write the source binary with `contents`, which is how an upgrade is spelled here.
    ///
    /// A different length is what [`shims::refresh`] compares on first, and a length that happens
    /// to match is the case the modification time carries — `written_at` below is for that one.
    fn publish(&self, contents: &[u8]) {
        std::fs::write(self.shim(), contents).expect("a source binary");
    }

    fn bin(&self) -> PathBuf {
        self.root.path().join("bin")
    }

    fn shim(&self) -> PathBuf {
        self.root
            .path()
            .join(format!("mixengine-shim{}", std::env::consts::EXE_SUFFIX))
    }

    fn refresh(&self) -> shims::Refreshed {
        shims::refresh(&self.bin(), &self.shim()).expect("a writable temporary directory")
    }

    fn copy_of(&self, name: &str) -> PathBuf {
        self.bin().join(name)
    }

    fn php(&self) -> PathBuf {
        self.copy_of(&format!("php{}", std::env::consts::EXE_SUFFIX))
    }
}

/// The whole of what a first start does to a home that has never had one.
#[test]
fn a_bin_that_does_not_exist_yet_is_created_and_filled() {
    let fixture = Fixture::new();
    let refreshed = fixture.refresh();

    assert_eq!(refreshed.commands.len(), shims::COMMANDS.len());
    assert_eq!(refreshed.written, refreshed.commands, "all of them, once");
    assert!(refreshed.removed.is_empty() && refreshed.refused.is_empty());

    for command in shims::COMMANDS {
        let copy = fixture.copy_of(&shims::file_name(command));
        assert_eq!(
            std::fs::read(&copy).expect("a copy"),
            b"the shim, build one",
            "{} is not the shim",
            copy.display()
        );
    }
}

/// A daemon is restarted many times a day, and every start calls this. Copying nineteen multi-megabyte
/// files each time would be the most expensive thing a start does.
#[test]
fn a_second_pass_over_an_intact_bin_copies_nothing() {
    let fixture = Fixture::new();
    fixture.refresh();

    let refreshed = fixture.refresh();

    assert!(refreshed.written.is_empty(), "{:?}", refreshed.written);
    assert_eq!(refreshed.commands.len(), shims::COMMANDS.len());
}

/// The upgrade case: a new build of the shim replaces every copy of the old one.
#[test]
fn a_shim_of_a_different_length_replaces_every_copy() {
    let fixture = Fixture::new();
    fixture.refresh();

    fixture.publish(b"the shim, build two, which is longer than build one");
    let refreshed = fixture.refresh();

    assert_eq!(refreshed.written, refreshed.commands, "all of them again");
    assert_eq!(
        std::fs::read(fixture.php()).expect("a copy"),
        b"the shim, build two, which is longer than build one"
    );
}

/// The repair a person performs by deleting a file: a start puts back what is missing, and only
/// what is missing.
#[test]
fn a_deleted_command_comes_back_and_the_rest_are_left_alone() {
    let fixture = Fixture::new();
    fixture.refresh();

    std::fs::remove_file(fixture.php()).expect("removable");

    let refreshed = fixture.refresh();

    assert_eq!(
        refreshed.written,
        vec![format!("php{}", std::env::consts::EXE_SUFFIX)],
        "one file was missing"
    );
    assert!(fixture.php().is_file());
}

/// `bin/` is entirely MixEngine's, which is what lets a refresh remove what it does not recognise —
/// a command that was renamed between releases would otherwise stay on the user's PATH forever,
/// running a shim that answers to nobody.
#[test]
fn a_name_no_command_answers_to_is_removed() {
    let fixture = Fixture::new();
    fixture.refresh();

    let stranger = fixture.copy_of("php7");
    std::fs::write(&stranger, b"from a MixEngine that is not this one").expect("a file");

    let refreshed = fixture.refresh();

    assert_eq!(refreshed.removed, vec!["php7".to_owned()]);
    assert!(refreshed.refused.is_empty());
    assert!(!stranger.exists());
}

/// What removing the home does first, and the one place a copy that will not go is reported rather
/// than fatal.
#[test]
fn clearing_takes_bin_back_to_nothing() {
    let fixture = Fixture::new();
    fixture.refresh();

    let cleared = shims::clear(&fixture.bin()).expect("a readable directory");

    assert_eq!(cleared.removed.len(), shims::COMMANDS.len());
    assert!(cleared.refused.is_empty());
    assert_eq!(
        std::fs::read_dir(fixture.bin())
            .expect("still a directory")
            .count(),
        0
    );

    // A home that never had a `bin/` is cleared without one being created to report on.
    let empty = tempfile::tempdir().expect("a temporary directory");
    let nothing = empty.path().join("bin");
    assert!(
        shims::clear(&nothing)
            .expect("nothing to do")
            .removed
            .is_empty()
    );
    assert!(!nothing.exists());
}

/// Where the shim is looked for, and what a broken installation is told.
#[test]
fn the_shim_is_found_beside_the_program_that_asks_for_it() {
    let fixture = Fixture::new();
    let mixengined = fixture
        .root
        .path()
        .join(format!("mixengined{}", std::env::consts::EXE_SUFFIX));

    assert_eq!(
        shims::source(&mixengined).expect("beside it"),
        fixture.shim()
    );

    let elsewhere = fixture.root.path().join("sbin").join("mixengined");
    let error = shims::source(&elsewhere).expect_err("nothing is beside it");

    assert!(
        matches!(error, mixengine_core::Error::ShimMissing { .. }),
        "{error}"
    );
}

/// Not a claim about any one filesystem: the table is what `bin/` is named from, and a row whose
/// name could not be a file would be a command nobody can type on the system that refuses it.
#[test]
fn every_command_becomes_a_file_name_this_system_can_run() {
    for command in shims::COMMANDS {
        let name = shims::file_name(command);

        assert!(name.starts_with(command.name));
        assert!(
            Path::new(&name)
                .file_name()
                .is_some_and(|found| found == name.as_str()),
            "{name} is not a single path component"
        );
        assert_eq!(
            name.ends_with(".exe"),
            cfg!(windows),
            "the suffix is the loader's rule: {name}"
        );
    }
}
