//! Raising a prompt: what a mock can prove, and what a real machine will answer without one.
//!
//! Nothing here raises a prompt. The mock half is the surface T40b's queue will be written against;
//! the real-host half asserts the refusals, which happen before any mechanism is reached and are
//! therefore safe to run on a developer's machine and in CI's ordinary `test` job. The half that
//! needs an administrative token is `#[ignore]`d and run by CI's `system` job.

use std::path::{Path, PathBuf};

use mixengine_platform::{ElevationSupport, Host as _, host, mock};
use mixengine_proto::privileged::ElevationOutcome;

/// Two absolute paths, which is all the mock looks at.
fn a_helper_and_a_request() -> (PathBuf, PathBuf) {
    let root = if cfg!(windows) {
        PathBuf::from(r"C:\Program Files\MixEngine")
    } else {
        PathBuf::from("/opt/mixengine")
    };

    (
        root.join("mixengine-elevate"),
        root.join("run")
            .join("elevate")
            .join("one")
            .join("request.json"),
    )
}

#[test]
fn a_mock_records_every_prompt_it_was_asked_to_raise() {
    let (helper, request) = a_helper_and_a_request();
    let machine = mock::Host::with_home("/tmp/mixengine-test");

    let outcome = machine
        .elevation()
        .run(&helper, &request)
        .expect("the mock raises nothing and refuses nothing");

    assert_eq!(outcome, ElevationOutcome::Completed);
    assert_eq!(
        machine.prompts_raised(),
        vec![mock::Prompt { helper, request }],
        "the pair is the assertion T40b's queue needs: one prompt, on the request it just wrote"
    );
}

/// The distinction T40b's degraded mode turns on: a machine that *could* prompt and was refused will
/// accept the same operation later, and one that cannot prompt at all never will.
#[test]
fn a_declined_prompt_is_still_a_machine_that_can_prompt() {
    let (helper, request) = a_helper_and_a_request();
    let machine = mock::Host::declining_elevation("/tmp/mixengine-test");

    assert_eq!(machine.elevation().probe(), ElevationSupport::Available);
    assert_eq!(
        machine.elevation().run(&helper, &request).unwrap(),
        ElevationOutcome::Declined
    );
}

#[test]
fn a_machine_that_cannot_prompt_says_so_before_it_is_asked() {
    let (helper, request) = a_helper_and_a_request();
    let machine = mock::Host::unable_to_elevate("/tmp/mixengine-test", "no polkit agent");

    assert_eq!(
        machine.elevation().probe(),
        ElevationSupport::Unavailable {
            reason: "no polkit agent".to_owned()
        }
    );
    assert_eq!(
        machine.elevation().run(&helper, &request).unwrap(),
        ElevationOutcome::Unavailable {
            reason: "no polkit agent".to_owned()
        }
    );
}

/// On the real machine, and on all three of them: a helper that is not there never reaches a
/// mechanism, so this test raises nothing.
#[test]
fn a_helper_that_is_not_there_is_refused_before_any_prompt() {
    let absent = std::env::temp_dir().join("mixengine-elevate-that-is-not-installed");

    let error = host()
        .elevation()
        .run(&absent, &absent)
        .expect_err("a helper that is not there is not run as root");

    assert!(
        error.to_string().contains("run as the elevation helper"),
        "{error}"
    );
}

#[test]
fn a_relative_helper_path_is_refused() {
    let error = host()
        .elevation()
        .run(Path::new("mixengine-elevate"), Path::new("request.json"))
        .expect_err("a relative path is resolved against a directory somebody else chose");

    assert!(
        error.to_string().contains("run as the elevation helper"),
        "{error}"
    );
}

/// D4, on the machine it is about. The quotation mark is refused **before** the request is looked
/// for, because a Windows path cannot contain one — so "there is no file there" would name the wrong
/// problem.
#[cfg(windows)]
#[test]
fn windows_refuses_a_request_path_carrying_a_quotation_mark() {
    let helper = std::env::current_exe().expect("this test binary has a path");

    let error = host()
        .elevation()
        .run(&helper, Path::new("C:\\a\"b\\request.json"))
        .expect_err("a quotation mark never reaches a command line");

    assert!(
        error.to_string().contains("quote for the elevation prompt"),
        "{error}"
    );
}
