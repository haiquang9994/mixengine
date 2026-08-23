//! `mix elevation grant` against a real `mixengined`, over a real endpoint. Roadmap task **T64**.
//!
//! **Nothing here grants anything, and that is the point rather than a gap.** A successful grant is
//! a real UAC dialog, a real `osascript` prompt or a real `pkexec` on the machine running
//! `cargo test`; `crates/mixengine-daemon/tests/elevation.rs` says the same about its own suite. So
//! every test below drives the half T64 added — the screen that comes *before* the prompt, and the
//! three answers that stop there — and none of them passes `--yes`.
//!
//! That is also what makes the suite safe to run anywhere: the operating system is never reached,
//! because a client that could not be answered never calls `elevation.grant` at all.
//!
//! Rows reach the queue the way they will on a user's machine: a site is created, and `site.create`
//! asks for the hosts file the home now needs (T41). Until T41 there was no producer at all and this
//! suite wrote its own row through `mixengine_testkit::privileged`, which is gone with that change —
//! a test that creates a site and *then* finds an operation waiting proves what a fixture could not.
//!
//! There is no `mix elevation enqueue` and never will be: what needs an administrator's permission
//! is decided by the operation that needs it, and a command that let a person queue an arbitrary
//! privileged operation would be a client deciding what runs as root. See
//! `crates/mixengine-cli/src/main.rs`.

mod harness;

use std::sync::atomic::{AtomicU32, Ordering};

use harness::{Home, json, stderr, stdout};

/// A domain nothing else on this machine could be using.
///
/// The daemon compares what the database wants against what the machine's **real** hosts file holds,
/// so a domain a developer might genuinely have in theirs would make this suite depend on the
/// machine it runs on. The pid is what makes it unique across two `cargo test` runs at once, and the
/// counter across the tests inside one.
fn a_domain_of_this_run() -> String {
    static NEXT: AtomicU32 = AtomicU32::new(0);

    format!(
        "t41-{}-{}.test",
        std::process::id(),
        NEXT.fetch_add(1, Ordering::Relaxed)
    )
}

/// A home with a daemon, a site in it, and therefore one operation waiting.
fn a_home_with_something_waiting() -> (Home, harness::Daemon, String) {
    let home = Home::new();
    let daemon = home.start_daemon();
    let domain = a_domain_of_this_run();

    // Inside the home, which is a temporary directory that outlives the daemon: `site.create`
    // records the path, and a later `site.list` walks it.
    let repository = home.path().join("project");
    std::fs::create_dir_all(&repository).expect("a directory for the project");
    let root = repository.display().to_string();

    let created = home.mix(&["project", "create", &root, "--name", "t41"]);
    assert!(created.status.success(), "{}", stderr(&created));

    let site = home.mix_in(
        &repository,
        &[],
        &[
            "site", "create", "--domain", &domain, "--kind", "static", "--json",
        ],
    );
    assert!(site.status.success(), "{}", stderr(&site));

    (home, daemon, domain)
}

/// An empty queue is the daemon's sentence to say, and there is no question in front of it.
///
/// The refusal is `elevation.grant`'s own — `PreconditionFailed`, no job row, no dialog — and it is
/// forwarded rather than anticipated: a client that composed "nothing is waiting" for itself would
/// be a second opinion on a precondition the daemon already holds, which is what
/// `CLAUDE.md`'s "no business logic in clients" forbids. What this asserts is the client's half:
/// nothing to ask about means nobody is asked.
#[test]
fn granting_with_nothing_waiting_asks_no_question() {
    let home = Home::new();
    let _daemon = home.start_daemon();

    let output = home.mix(&["elevation", "grant"]);

    assert!(!output.status.success(), "{}", stdout(&output));
    assert!(
        stderr(&output).contains("nothing is waiting"),
        "{}",
        stderr(&output)
    );
    assert!(
        !stderr(&output).contains("continue?"),
        "an empty queue is not turned into a question: {}",
        stderr(&output)
    );
}

/// The claim T64 exists for, now driven by the product: the list comes first, and the question
/// comes after it.
#[test]
fn granting_says_what_each_operation_will_change_before_it_asks() {
    let (home, _daemon, domain) = a_home_with_something_waiting();

    let output = home.mix(&["elevation", "grant"]);
    let said = stderr(&output);

    let described = said
        .find(&domain)
        .unwrap_or_else(|| panic!("the domain that will be written is printed: {said}"));
    let asked = said
        .find("continue?")
        .unwrap_or_else(|| panic!("the question is asked: {said}"));

    assert!(
        described < asked,
        "the list has to come before the question: {said}"
    );
}

/// A cron job, a CI step, a service manager: standard input is closed, and there is nobody to ask.
///
/// Refused rather than assumed either way. Assuming yes raises an administrator's dialog on a
/// machine nobody is sitting at; assuming no would be a silent decline that a script could not tell
/// from a granted one.
#[test]
fn granting_refuses_when_there_is_nobody_to_answer() {
    let (home, _daemon, _domain) = a_home_with_something_waiting();

    let output = home.mix(&["elevation", "grant"]);

    assert!(!output.status.success(), "{}", stderr(&output));
    assert!(
        stderr(&output).contains("--yes"),
        "the flag that answers in advance is named: {}",
        stderr(&output)
    );

    // A terminal echoes the newline a person types; end of file echoes nothing, so the line has to
    // be closed here or the refusal is printed onto the end of the question that was never answered.
    assert!(
        stderr(&output).contains("\nerror:"),
        "the unanswered question is closed before anything else is said: {}",
        stderr(&output)
    );

    nothing_was_granted(&home);
}

/// Saying no is an answer and not an error — `.claude/decisions/0005-on-demand-elevation.md`.
///
/// What it must not do is lose anything: the queue is exactly as it was, so the same command can be
/// run again when the person is ready.
#[test]
fn answering_no_leaves_everything_waiting() {
    let (home, _daemon, _domain) = a_home_with_something_waiting();

    let output = home.mix_answering("n\n", &["elevation", "grant"]);

    assert!(
        output.status.success(),
        "a decline is a normal answer: {}",
        stderr(&output)
    );

    nothing_was_granted(&home);
}

/// `--json` is a machine reading the answer, and a machine cannot be asked a question.
#[test]
fn json_cannot_be_asked_so_it_has_to_be_told_in_advance() {
    let (home, _daemon, _domain) = a_home_with_something_waiting();

    let output = home.mix_answering("y\n", &["elevation", "grant", "--json"]);

    assert!(!output.status.success(), "{}", stderr(&output));
    assert!(stderr(&output).contains("--yes"), "{}", stderr(&output));

    nothing_was_granted(&home);
}

/// The degraded mode a decline leaves behind is visible from the command people actually type.
///
/// **A guard rather than a driver.** T40b already put the count on `daemon.status`, so this passed
/// the day it was written; it is here because it is one of T64's claims, and the claim is about the
/// pair — a decline that returns to the shell, and a count that survives it.
#[test]
fn status_keeps_counting_what_is_waiting_after_a_decline() {
    let (home, _daemon, _domain) = a_home_with_something_waiting();

    home.mix_answering("n\n", &["elevation", "grant"]);

    let status = json(&home.mix(&["status", "--json"]));
    assert_eq!(status["daemon"]["elevation"]["pending"], 1, "{status}");

    let said = stdout(&home.mix(&["status"]));
    assert!(said.contains("waiting"), "{said}");
}

/// The queue is untouched and no grant was ever attempted.
///
/// `last` is the sharp half: the daemon records the outcome of every grant it runs, so its absence
/// is proof that `elevation.grant` was not called rather than that it failed politely.
fn nothing_was_granted(home: &Home) {
    let status = json(&home.mix(&["elevation", "status", "--json"]));

    assert_eq!(
        status["pending"].as_array().map(Vec::len),
        Some(1),
        "{status}"
    );
    assert!(
        status.get("last").is_none(),
        "no grant was attempted: {status}"
    );
}
