//! `open_home` — the four startup steps in the one order that works.
//!
//! Every root is a `TempDir` the test owns: `open_home` creates directories, so a test that passed
//! a relative path would create them next to the source tree instead.

use mixengine_core::config::{self, LogLevel};
use mixengine_core::open_home;
use mixengine_platform::mock;
use tempfile::TempDir;

/// TOML wants forward slashes or escaped backslashes; `Path` treats both as separators on Windows.
fn toml_path(path: &std::path::Path) -> String {
    path.to_str()
        .expect("a TempDir path is UTF-8 on every platform the tests run on")
        .replace('\\', "/")
}

#[test]
fn a_first_run_builds_the_whole_home() {
    let temp = TempDir::new().unwrap();
    let root = temp.path().join("MixEngine");
    let host = mock::Host::with_home(&root);

    let home = open_home(None, &host).unwrap();

    assert_eq!(home.paths.root(), root);
    for directory in home.paths.directories() {
        assert!(directory.is_dir(), "{} is missing", directory.display());
    }
    assert!(home.paths.config_file().is_file());
    assert_eq!(home.config, config::Config::default());
}

#[test]
fn opening_an_existing_home_changes_nothing() {
    let temp = TempDir::new().unwrap();
    let host = mock::Host::with_home(temp.path());

    let first = open_home(None, &host).unwrap();
    std::fs::write(first.paths.data().join("marker"), b"user data").unwrap();

    let second = open_home(None, &host).unwrap();

    assert_eq!(first.paths, second.paths);
    assert_eq!(
        std::fs::read(second.paths.data().join("marker")).unwrap(),
        b"user data"
    );
}

#[test]
fn config_is_read_before_the_layout_is_built() {
    let temp = TempDir::new().unwrap();
    let root = temp.path().join("home");
    let bulk = temp.path().join("bulk");
    std::fs::create_dir_all(&root).unwrap();
    std::fs::write(
        root.join(config::FILE_NAME),
        format!(
            "[log]\nlevel = \"debug\"\n\n[paths]\ndata = \"{}\"\n",
            toml_path(&bulk)
        ),
    )
    .unwrap();

    let home = open_home(Some(&root), &mock::Host::without_home()).unwrap();

    // The override took effect on this very run, not on the next one.
    assert_eq!(home.paths.data(), bulk);
    assert!(bulk.is_dir());
    assert!(!root.join("data").exists());
    assert_eq!(home.config.log.level, LogLevel::Debug);
}

#[test]
fn a_broken_config_stops_startup_instead_of_being_ignored() {
    let temp = TempDir::new().unwrap();
    let root = temp.path().join("home");
    std::fs::create_dir_all(&root).unwrap();
    std::fs::write(root.join(config::FILE_NAME), "[paths]\nruntimes = 7\n").unwrap();

    let error = open_home(Some(&root), &mock::Host::without_home()).unwrap_err();

    assert!(
        matches!(error, mixengine_core::Error::Config { .. }),
        "{error:?}"
    );
    // Startup stopped at the config file: nothing further was created.
    assert!(!root.join("runtimes").exists());
}

#[test]
fn the_override_wins_over_the_platform_default() {
    let temp = TempDir::new().unwrap();
    let sandbox = temp.path().join("sandbox");
    let host = mock::Host::with_home(temp.path().join("default"));

    let home = open_home(Some(&sandbox), &host).unwrap();

    assert_eq!(home.paths.root(), sandbox);
    assert!(!temp.path().join("default").exists());
    assert_eq!(home.paths.database_file(), sandbox.join("mixengine.db"));
}
