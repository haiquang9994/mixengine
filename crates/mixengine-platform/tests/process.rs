//! Running a program for its answer, against the real OS.
//!
//! `run_once`'s deadline, its pipes and its environment are exercised wherever a probe or a stop
//! command is; what is here is the half nothing else reaches — a one-shot that is handed something
//! to read.

use std::collections::BTreeMap;
use std::time::Duration;

/// The program that copies its standard input to its standard output, on this system.
fn echoing_stdin() -> (std::path::PathBuf, Vec<std::ffi::OsString>) {
    if cfg!(windows) {
        (
            std::path::PathBuf::from(r"C:\Windows\System32\cmd.exe"),
            vec!["/c".into(), "more".into()],
        )
    } else {
        (std::path::PathBuf::from("/bin/cat"), Vec::new())
    }
}

/// A one-shot can be handed something to read, and it reads it.
///
/// The whole reason this exists: `mariadbd --bootstrap` takes its SQL on stdin, which is what keeps
/// a password-less root off a listening port during the one window it would otherwise exist in.
#[tokio::test]
async fn a_one_shot_reads_what_it_was_given() {
    let (program, args) = echoing_stdin();

    let ran = mixengine_platform::process::run_once_with_input(
        &program,
        &args,
        &std::env::temp_dir(),
        &BTreeMap::new(),
        Duration::from_secs(30),
        "mixengine
",
    )
    .await
    .expect("the program ran");

    assert!(ran.succeeded(), "{ran:?}");
    // `complaint` is the last line of whatever the program said, which for one that was given a
    // line and copies it is that line — there is no other accessor, and none is owed: a one-shot is
    // run for its exit status, and what it printed is evidence for a log.
    assert_eq!(ran.complaint(), Some("mixengine"), "{ran:?}");
}

/// A one-shot given nothing to read still gets an end of file rather than a terminal.
///
/// The other half of the same arrangement: `run_once` hands its child the null device, so a program
/// that decides to ask a question is a deadline rather than a daemon waiting on a prompt nobody can
/// see.
#[tokio::test]
async fn a_one_shot_given_nothing_reads_nothing() {
    let (program, args) = echoing_stdin();

    let ran = mixengine_platform::process::run_once(
        &program,
        &args,
        &std::env::temp_dir(),
        &BTreeMap::new(),
        Duration::from_secs(30),
    )
    .await
    .expect("the program ran");

    assert!(ran.succeeded(), "{ran:?}");
}

/// What a supervised child says reaches its reader, and the type it arrives in is the platform's.
///
/// **The type is the point.** `take_stdout` handed back `std::process::ChildStdout` until T34a,
/// which cannot be built from a handle this crate creates — and on Windows this crate now creates
/// one, because a child created from an unrestricted token is a child PostgreSQL will not be. What
/// this asserts is the shape that made that possible: something implementing [`std::io::Read`],
/// owned by the caller, whichever way the child was started.
#[test]
fn a_supervised_child_says_what_it_says_through_a_platform_pipe() {
    use std::io::Read as _;

    let (program, args) = saying_a_word();

    let mut child = mixengine_platform::process::spawn_supervised(
        &program,
        &args,
        &std::env::temp_dir(),
        &BTreeMap::new(),
    )
    .expect("a program that prints one line can be started");

    let mut pipe: mixengine_platform::process::OutputPipe =
        child.take_stdout().expect("a supervised child is piped");

    let mut said = String::new();
    pipe.read_to_string(&mut said)
        .expect("its stdout is readable");

    assert!(said.contains("supervised"), "{said:?}");

    let _ = child.wait();
}

/// A program every system has, printing a word the test above can look for.
fn saying_a_word() -> (std::path::PathBuf, Vec<std::ffi::OsString>) {
    if cfg!(windows) {
        (
            std::path::PathBuf::from(r"C:\Windows\System32\cmd.exe"),
            vec!["/c".into(), "echo supervised".into()],
        )
    } else {
        (
            std::path::PathBuf::from("/bin/sh"),
            vec!["-c".into(), "echo supervised".into()],
        )
    }
}
