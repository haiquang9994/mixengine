//! Starting a desktop application, against the real OS — roadmap task **T83**.
//!
//! `locate` is per system and each implementation proves its own mechanism in its own module;
//! what is here is the launcher every system shares, driven against the shell every system has,
//! and the mock's contract.

use std::collections::BTreeMap;
use std::ffi::OsString;
use std::path::PathBuf;

use mixengine_platform::{Host as _, InstalledApp, Located, Started, mock};

/// The shell, and the argument that makes it run one line.
fn shell() -> (PathBuf, &'static str) {
    if cfg!(windows) {
        (PathBuf::from(r"C:\Windows\System32\cmd.exe"), "/c")
    } else {
        (PathBuf::from("/bin/sh"), "-c")
    }
}

fn app_running(line: &str) -> (InstalledApp, Vec<OsString>) {
    let (program, flag) = shell();
    (
        InstalledApp {
            program,
            args: vec![OsString::from(flag)],
        },
        vec![OsString::from(line)],
    )
}

/// A program that exits 0 at once is a client that handed on — D8.
#[test]
fn a_clean_exit_inside_the_judgement_is_a_handoff() {
    let host = mixengine_platform::host();
    let (app, args) = app_running("exit 0");

    let started = host
        .desktop_apps()
        .launch(&app, &args, &BTreeMap::new())
        .expect("the shell runs");

    assert_eq!(started, Started::HandedOn);
}

/// A program that exits otherwise is a failure naming the status.
#[test]
fn a_failing_exit_inside_the_judgement_names_the_status() {
    let host = mixengine_platform::host();
    let (app, args) = app_running("exit 3");

    let started = host
        .desktop_apps()
        .launch(&app, &args, &BTreeMap::new())
        .expect("the shell runs");

    match started {
        Started::Failed { status } => assert!(status.contains('3'), "{status}"),
        other => panic!("{other:?}"),
    }
}

/// A program still up after the judgement is running, and is reaped later rather than left.
#[test]
fn a_program_still_up_after_a_second_is_running() {
    let host = mixengine_platform::host();
    let line = if cfg!(windows) {
        "ping -n 4 127.0.0.1 >NUL"
    } else {
        "sleep 3"
    };
    let (app, args) = app_running(line);

    let started = host
        .desktop_apps()
        .launch(&app, &args, &BTreeMap::new())
        .expect("the shell runs");

    assert!(
        matches!(started, Started::Running { pid } if pid > 0),
        "{started:?}"
    );
}

/// The variable reaches the child — the whole of D2's mechanism.
#[test]
fn the_environment_reaches_the_child() {
    let host = mixengine_platform::host();
    let line = if cfg!(windows) {
        "if not defined MIXENGINE_TEST_HANDOFF exit 5"
    } else {
        "test -n \"$MIXENGINE_TEST_HANDOFF\""
    };
    let (app, args) = app_running(line);
    let env = BTreeMap::from([("MIXENGINE_TEST_HANDOFF".to_owned(), "present".to_owned())]);

    assert_eq!(
        host.desktop_apps()
            .launch(&app, &args, &env)
            .expect("the shell runs"),
        Started::HandedOn
    );
    assert!(matches!(
        host.desktop_apps()
            .launch(&app, &args, &BTreeMap::new())
            .expect("the shell runs"),
        Started::Failed { .. }
    ));
}

/// A program that cannot be started is an error and not a state.
#[test]
fn a_program_that_is_not_there_is_an_error() {
    let host = mixengine_platform::host();
    let app = InstalledApp {
        program: std::env::temp_dir().join("mixengine-no-such-program"),
        args: Vec::new(),
    };

    assert!(
        host.desktop_apps()
            .launch(&app, &[], &BTreeMap::new())
            .is_err()
    );
}

/// The mock's ordinary machine has nothing installed; one built with an application finds it for
/// any hint and records what it started — names of variables, never values.
#[test]
fn the_mock_records_a_launch_without_its_values() {
    let bare = mock::Host::with_home("/tmp/mixengine-test");
    assert!(matches!(
        bare.desktop_apps().locate("MixDB.exe").expect("answers"),
        Located::NotInstalled { .. }
    ));

    let host = mock::Host::with_desktop_app("/tmp/mixengine-test", "/opt/mixdb/mixdb");
    let Located::Installed(app) = host.desktop_apps().locate("anything").expect("answers") else {
        panic!("installed");
    };
    let env = BTreeMap::from([("MIXENGINE_DB_PASSWORD".to_owned(), "s3cret".to_owned())]);
    let started = host
        .desktop_apps()
        .launch(&app, &[OsString::from("mixdb://connect")], &env)
        .expect("the mock starts anything");
    assert!(matches!(started, Started::Running { .. }));

    let launched = host.launched();
    assert_eq!(launched.len(), 1);
    assert_eq!(launched[0].program, PathBuf::from("/opt/mixdb/mixdb"));
    assert_eq!(launched[0].args, vec![OsString::from("mixdb://connect")]);
    assert_eq!(
        launched[0].env_names,
        vec!["MIXENGINE_DB_PASSWORD".to_owned()]
    );
    assert!(!format!("{launched:?}").contains("s3cret"));
}
