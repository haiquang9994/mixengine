//! The one thing the platform layer decides about paths: where the root goes by default.
//!
//! This reads the environment but never writes it, and never creates a directory — `default_home`
//! answers a question, it does not act on the machine.

use mixengine_platform::{Host as _, host, mock};

#[test]
fn the_default_root_follows_this_os_convention() {
    let host = host();

    let root = host.home_dirs().default_home().unwrap();

    assert!(root.is_absolute(), "{} is not absolute", root.display());
    // Windows and macOS name application directories the way their users see them; XDG is
    // lowercase.
    let expected = if cfg!(target_os = "linux") {
        "mixengine"
    } else {
        "MixEngine"
    };
    assert_eq!(root.file_name().unwrap(), expected);
    // Under the user's data directory, not at the top of a drive or a filesystem.
    assert!(
        root.parent()
            .is_some_and(|parent| parent.parent().is_some())
    );
}

#[test]
fn asking_twice_gives_the_same_answer() {
    let host = host();

    assert_eq!(
        host.home_dirs().default_home().unwrap(),
        host.home_dirs().default_home().unwrap()
    );
}

#[test]
fn the_mock_answers_with_whatever_the_test_chose() {
    let host = mock::Host::with_home("/somewhere/for/this/test");

    assert_eq!(
        host.home_dirs().default_home().unwrap(),
        std::path::Path::new("/somewhere/for/this/test")
    );
}

#[test]
fn a_mock_without_a_home_reports_it_instead_of_guessing() {
    let host = mock::Host::without_home();

    let error = host.home_dirs().default_home().unwrap_err();

    assert!(
        matches!(error, mixengine_platform::Error::NoHomeDirectory { .. }),
        "{error:?}"
    );
    assert!(error.to_string().contains("MIXENGINE_HOME"), "{error}");
}
