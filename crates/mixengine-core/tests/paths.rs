//! The directory layout and how `MIXENGINE_HOME` is chosen.
//!
//! Every test owns a `TempDir` and passes it in explicitly. Nothing here reads or writes the
//! environment: `std::env::set_var` is `unsafe` in edition 2024 and process-global either way, so
//! two tests running in parallel would silently rewrite each other's home directory.

use std::path::{Path, PathBuf};

use mixengine_core::config::PathOverrides;
use mixengine_core::paths::{Paths, resolve_root};
use mixengine_platform::mock;
use tempfile::TempDir;

fn paths_at(root: &Path) -> Paths {
    Paths::new(root.to_path_buf(), &PathOverrides::default())
}

#[test]
fn layout_matches_the_documented_tree() {
    let root = PathBuf::from("/srv/mixengine");
    let paths = paths_at(&root);

    assert_eq!(paths.root(), root);
    assert_eq!(paths.bin(), root.join("bin"));
    assert_eq!(paths.runtimes(), root.join("runtimes"));
    assert_eq!(paths.packages(), root.join("packages"));
    assert_eq!(paths.data(), root.join("data"));
    assert_eq!(paths.etc(), root.join("etc"));
    assert_eq!(paths.certs(), root.join("certs"));
    assert_eq!(paths.logs(), root.join("logs"));
    assert_eq!(paths.extensions(), root.join("extensions"));
    assert_eq!(paths.blueprints(), root.join("blueprints"));
    assert_eq!(paths.run(), root.join("run"));
    assert_eq!(paths.database_file(), root.join("mixengine.db"));
    assert_eq!(paths.config_file(), root.join("config.toml"));
}

#[test]
fn bootstrap_creates_every_directory_and_can_be_repeated() {
    let home = TempDir::new().unwrap();
    let root = home.path().join("nested/root");
    let paths = paths_at(&root);

    paths.bootstrap().unwrap();
    for directory in paths.directories() {
        assert!(
            directory.is_dir(),
            "{} was not created",
            directory.display()
        );
    }

    // Idempotent: this is the same call `mix doctor` makes against a healthy install.
    paths.bootstrap().unwrap();
    assert!(paths.run().is_dir());
}

#[test]
fn bootstrap_leaves_existing_content_alone() {
    let home = TempDir::new().unwrap();
    let paths = paths_at(home.path());
    paths.bootstrap().unwrap();

    let database = paths.database_file();
    std::fs::write(database, b"not really a database").unwrap();
    std::fs::write(paths.certs().join("root.crt"), b"a certificate").unwrap();

    paths.bootstrap().unwrap();

    assert_eq!(std::fs::read(database).unwrap(), b"not really a database");
    assert!(paths.certs().join("root.crt").is_file());
}

#[test]
fn bootstrap_reports_the_path_it_could_not_create() {
    let home = TempDir::new().unwrap();
    let root = home.path().join("root");
    // A file where the root should be: `create_dir_all` cannot win this one.
    std::fs::write(&root, b"in the way").unwrap();

    let error = paths_at(&root).bootstrap().unwrap_err();
    let message = error.to_string();

    assert!(message.contains("create directory"), "{message}");
    assert!(message.contains("root"), "{message}");
}

#[test]
fn an_absolute_override_moves_a_directory_out_of_the_root() {
    let root = PathBuf::from("/srv/mixengine");
    let elsewhere = if cfg!(windows) {
        PathBuf::from(r"D:\bulk\runtimes")
    } else {
        PathBuf::from("/mnt/bulk/runtimes")
    };

    let paths = Paths::new(
        root.clone(),
        &PathOverrides {
            runtimes: Some(elsewhere.clone()),
            ..PathOverrides::default()
        },
    );

    assert_eq!(paths.runtimes(), elsewhere);
    // Only the overridden directory moves.
    assert_eq!(paths.packages(), root.join("packages"));
    assert_eq!(paths.root(), root);
}

#[test]
fn a_relative_override_is_relative_to_the_root_not_the_working_directory() {
    let root = PathBuf::from("/srv/mixengine");
    let paths = Paths::new(
        root.clone(),
        &PathOverrides {
            data: Some(PathBuf::from("volumes/data")),
            ..PathOverrides::default()
        },
    );

    assert_eq!(paths.data(), root.join("volumes/data"));
}

#[test]
fn the_platform_default_is_used_when_nothing_overrides_it() {
    let home = TempDir::new().unwrap();
    let host = mock::Host::with_home(home.path());

    assert_eq!(resolve_root(None, &host).unwrap(), home.path());
}

#[test]
fn an_override_beats_the_platform_default() {
    let home = TempDir::new().unwrap();
    let chosen = home.path().join("somewhere-else");
    let host = mock::Host::with_home(home.path());

    assert_eq!(resolve_root(Some(&chosen), &host).unwrap(), chosen);
}

#[test]
fn a_relative_override_becomes_absolute() {
    let host = mock::Host::with_home("/unused");

    let root = resolve_root(Some(Path::new("relative-home")), &host).unwrap();

    assert!(root.is_absolute(), "{} is not absolute", root.display());
    assert!(root.ends_with("relative-home"), "{}", root.display());
}

#[test]
fn an_empty_override_is_refused_rather_than_treated_as_absent() {
    // `mixengined` never produces this: `clap` rejects an empty `--home`/`MIXENGINE_HOME` first.
    // The guard is here for every other caller of this public function, and what it must never do
    // is fall back to the platform default — a sandbox run would land in the real install.
    let home = TempDir::new().unwrap();
    let host = mock::Host::with_home(home.path());

    let error = resolve_root(Some(Path::new("")), &host).unwrap_err();

    assert!(
        matches!(error, mixengine_core::Error::EmptyHome),
        "{error:?}"
    );
}

#[test]
fn a_host_with_no_answer_is_reported_rather_than_guessed() {
    let host = mock::Host::without_home();

    let error = resolve_root(None, &host).unwrap_err();

    assert!(
        matches!(error, mixengine_core::Error::Platform(_)),
        "{error:?}"
    );
    // The message has to name the way out, because the user's next move is to set it.
    assert!(error.to_string().contains("MIXENGINE_HOME"), "{error}");
}
