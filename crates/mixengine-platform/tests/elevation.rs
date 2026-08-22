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

// ---------------------------------------------------------------------------------------------
// The half that needs a real machine. `#[ignore]`d, so a developer's `cargo test` says how many it
// skipped rather than reporting a pass it did not earn; CI's `system` job runs these elevated on all
// three runners. See the T40a design, "Testing".
// ---------------------------------------------------------------------------------------------

/// The `mixengine-elevate` built alongside this test.
///
/// `CARGO_BIN_EXE_…` reaches only binaries of the package the test itself is in, and the helper is
/// another one — so it is found next to the test binary, which is where a workspace build puts both.
/// The same mechanism `mixengine-testkit` uses to find `fakeservice`.
fn helper() -> PathBuf {
    let name = format!("mixengine-elevate{}", std::env::consts::EXE_SUFFIX);
    let test = std::env::current_exe().expect("this test binary has a path");
    let directory = test.parent().expect("this test binary is in a directory");

    // `target/<profile>/deps/` first, then `target/<profile>/` above it: cargo builds integration
    // tests into the former and binaries into the latter.
    let beside = directory.join(&name);
    if beside.is_file() {
        return beside;
    }

    let above = directory
        .parent()
        .expect("the deps directory is inside the profile directory")
        .join(&name);

    assert!(
        above.is_file(),
        "{} is not there — this suite runs the real helper, so build it first: \
         `cargo build -p mixengine-elevate`",
        above.display()
    );

    above
}

/// A request on disk, in a home this test owns, exactly where the daemon would put one.
struct Pending {
    /// Kept alive so the directory below outlives the helper reading it.
    _home: tempfile::TempDir,
    directory: PathBuf,
    path: PathBuf,
}

/// Write one, and hand it to whoever invoked `sudo`.
///
/// **A root-owned request is what the helper refuses** — T40/D4 makes the owner of the request file
/// the identity of the caller — and on the two Unix legs this suite is itself running as root. A
/// no-op on Windows, where an administrator's own files belong to `BUILTIN\Administrators` and that
/// is the ordinary case rather than the refused one.
///
/// The document is built from the type rather than from a JSON literal: `PrivilegedRequest` is
/// `deny_unknown_fields`, so a field renamed in `mixengine-proto` should break this at compile time
/// and not as a puzzling refusal inside an elevated process.
fn a_pending_request() -> Pending {
    let home = tempfile::TempDir::new().expect("the system temporary directory is writable");
    let directory = home.path().join("run").join("elevate").join("t40a");
    std::fs::create_dir_all(&directory).expect("the request directory");

    let body = mixengine_proto::privileged::PrivilegedRequest {
        version: mixengine_proto::PROTOCOL_VERSION,
        home: home.path().to_path_buf(),
        nonce: "t40a".to_owned(),
        ops: vec![
            serde_json::to_value(mixengine_proto::privileged::PrivilegedOp::Probe {})
                .expect("an operation encodes"),
        ],
    };

    let path = directory.join("request.json");
    std::fs::write(&path, serde_json::to_vec(&body).expect("a request encodes"))
        .expect("the request");

    #[cfg(unix)]
    if let Some(uid) = std::env::var("SUDO_UID")
        .ok()
        .and_then(|uid| uid.parse().ok())
    {
        for owned in [home.path(), directory.as_path(), path.as_path()] {
            std::os::unix::fs::chown(owned, Some(uid), None).expect("root may give a file away");
        }
    }

    Pending {
        _home: home,
        directory,
        path,
    }
}

/// The report the helper left, or a failure that says what was there instead.
fn report(pending: &Pending) -> mixengine_proto::privileged::PrivilegedResponse {
    let path = pending
        .directory
        .join(mixengine_proto::privileged::RESPONSE_FILE_NAME);

    let text = std::fs::read_to_string(&path).unwrap_or_else(|error| {
        panic!(
            "no report at {} ({error}) — `Completed` means the helper ran, not that it left one",
            path.display()
        )
    });

    serde_json::from_str(&text).expect("the helper writes what proto describes")
}

/// The whole round trip, prompt-free: this runner already holds a full administrator token (T2b), so
/// `runas` raises nothing and what is under test is the launcher itself — the argument it builds, the
/// working directory it sets, the handle it waits on.
#[cfg(windows)]
#[test]
#[ignore = "needs an administrative token; run in CI's system job"]
fn windows_runs_the_helper_and_a_report_appears_beside_the_request() {
    let pending = a_pending_request();

    let outcome = host()
        .elevation()
        .run(&helper(), &pending.path)
        .expect("the helper is installed and both paths are absolute");

    assert_eq!(outcome, ElevationOutcome::Completed);

    let report = report(&pending);
    assert_eq!(report.nonce, "t40a");
    assert!(
        report.elevated,
        "this leg is only meaningful under an administrative token"
    );
}

/// **A measurement, and written as one.** If this leg turns out to authenticate rather than run
/// straight through, it hangs, the step's own `timeout-minutes` ends it, and the finding is recorded
/// in the phase file — this row is then reduced to `probe()` rather than guessed at in advance. That
/// is T29's method applied to a yes/no question: measure it, do not reason about it.
#[cfg(target_os = "macos")]
#[test]
#[ignore = "needs an administrative token; run in CI's system job"]
fn macos_runs_the_helper_and_a_report_appears_beside_the_request() {
    let pending = a_pending_request();

    let outcome = host()
        .elevation()
        .run(&helper(), &pending.path)
        .expect("the helper is installed and both paths are absolute");

    assert_eq!(outcome, ElevationOutcome::Completed);

    let report = report(&pending);
    assert_eq!(report.nonce, "t40a");
    assert!(report.elevated);
}

/// ADR 0005's worst branch, asserted on a machine that genuinely has no authentication agent. A
/// GitHub Linux runner has no graphical session, so this is not a second-class assertion — it is the
/// only place the fallback is ever exercised for real.
#[cfg(target_os = "linux")]
#[test]
#[ignore = "asserts a property of a headless machine; run in CI's system job"]
fn linux_says_it_cannot_prompt_and_hands_back_the_command_to_run_by_hand() {
    let pending = a_pending_request();
    let machine = host();

    assert!(
        matches!(
            machine.elevation().probe(),
            ElevationSupport::Unavailable { .. }
        ),
        "a runner has no graphical session, so there is no agent to show a prompt in"
    );

    let outcome = machine
        .elevation()
        .run(&helper(), &pending.path)
        .expect("both paths are absolute and both files are there");

    let ElevationOutcome::Unavailable { reason } = outcome else {
        panic!(
            "a machine with no agent must not be reported as having raised a prompt: {outcome:?}"
        )
    };

    assert!(reason.contains("pkexec"), "{reason}");
    assert!(
        reason.contains(&helper().display().to_string()),
        "the fallback is only useful if it is the whole command: {reason}"
    );
    assert!(
        reason.contains(&pending.path.display().to_string()),
        "{reason}"
    );
}
