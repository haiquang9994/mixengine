//! The single-instance lock, against the real OS.
//!
//! Every one of these locks a file inside a `TempDir` this test owns, so nothing here can meet the
//! real `run/mixengined.lock` or another test's.
//!
//! Two processes are not needed to prove any of it, and that is a property of the mechanism rather
//! than a shortcut: `flock` belongs to the open file description and Windows' share mode to the
//! handle, so a *second acquire in this same process* is refused exactly as a second daemon would
//! be. What genuinely cannot be covered here is the lock outliving a killed process — that needs a
//! real daemon, and `mixengine-daemon/tests/lifecycle.rs` is where it happens.

use std::path::Path;

use mixengine_platform::lock::{Acquired, Lock};
use tempfile::TempDir;

/// A `run/` directory of a home that exists only for this test.
fn run_dir() -> TempDir {
    TempDir::new().expect("the system temporary directory is writable")
}

/// Take the lock, or fail saying who was found holding it.
fn held(acquired: Acquired) -> Lock {
    match acquired {
        Acquired::Held(lock) => lock,
        Acquired::Taken(holder) => panic!("the lock was already held by {holder}"),
    }
}

#[test]
fn a_lock_nobody_holds_is_taken_and_names_this_process() {
    let run = run_dir();
    let file = run.path().join("mixengined.lock");

    let _lock = held(Lock::acquire(&file).unwrap());

    // The pid is what a second daemon puts in its message, and it is written to the file rather
    // than kept in memory precisely because the process that reads it is not this one.
    assert_eq!(recorded(&file), Some(std::process::id()));
}

#[test]
fn a_second_attempt_finds_the_first_one_holding_it() {
    let run = run_dir();
    let file = run.path().join("mixengined.lock");

    let _first = held(Lock::acquire(&file).unwrap());

    match Lock::acquire(&file).unwrap() {
        Acquired::Taken(holder) => assert_eq!(holder.pid(), Some(std::process::id())),
        Acquired::Held(_) => panic!("two locks were handed out for one file"),
    }
}

#[test]
fn the_pid_of_the_holder_survives_being_read_while_it_is_held() {
    // Windows' share mode is the whole mechanism there, so this asserts the half of it that is
    // easy to lose: readers are still admitted. A lock that kept everybody out would take the
    // "a daemon is already running, here is its pid" message down with it.
    let run = run_dir();
    let file = run.path().join("mixengined.lock");

    let _lock = held(Lock::acquire(&file).unwrap());

    assert_eq!(
        std::fs::read_to_string(&file).unwrap().trim(),
        std::process::id().to_string()
    );
}

#[test]
fn releasing_it_lets_the_next_daemon_in() {
    let run = run_dir();
    let file = run.path().join("mixengined.lock");

    drop(held(Lock::acquire(&file).unwrap()));

    // And the file is still there afterwards. Removing it is what would make two daemons able to
    // hold two different files under one name, so its survival is the behaviour and not a leak.
    let _next = held(Lock::acquire(&file).unwrap());
    assert!(file.exists());
}

#[test]
fn a_lock_file_left_behind_by_a_dead_daemon_keeps_nobody_out() {
    // The state after a machine loses power: the file is there, with a pid that means nothing.
    // Only the handle is the lock, so this must not even slow the next start down.
    let run = run_dir();
    let file = run.path().join("mixengined.lock");
    std::fs::write(&file, "424242\n").unwrap();

    let _lock = held(Lock::acquire(&file).unwrap());

    // And the stale pid is replaced rather than appended to, which is the reason the file is
    // truncated after the lock is taken instead of at open.
    assert_eq!(recorded(&file), Some(std::process::id()));
}

#[test]
fn a_lock_in_a_directory_that_is_not_there_is_a_failure_naming_it() {
    let run = run_dir();
    let file = run.path().join("nowhere").join("mixengined.lock");

    let error = Lock::acquire(&file).expect_err("there is no directory to create the file in");

    assert!(
        error.to_string().contains("mixengined.lock"),
        "the message names the file that could not be locked: {error}"
    );
}

/// What the lock file says, as a pid.
fn recorded(file: &Path) -> Option<u32> {
    std::fs::read_to_string(file).ok()?.trim().parse().ok()
}
