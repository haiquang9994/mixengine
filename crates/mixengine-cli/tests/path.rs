//! `mix path` against a real `mixengined` — roadmap task **T26**.
//!
//! **Only the read is end to end, and that is deliberate rather than a gap.** `path.install` writes
//! the PATH of the account running it: on Windows a value in this user's registry hive, on both
//! others a marked block in their shell profiles. A suite that called it would be a `cargo test`
//! that edits the environment of whoever ran it, on a developer's laptop and on a CI runner that
//! keeps its home directory between jobs. So the two mutations are proved where they can be proved
//! against something the test owns: the real ones in `mixengine-platform`, against a registry key
//! and a home directory each test creates and deletes, and the dispatch and the wire shape in
//! `mixengine-daemon`'s own tests against `mock::Host`.
//!
//! What is left for this file is the half nothing else covers, and it is not a small half: that a
//! real daemon **fills `bin/` on the way up** without being asked, that `mix path status` reads the
//! real machine and answers, and that what it reports is what is actually in the directory.
//!
//! **The table is never restated here.** `mixengine-core` is not a dependency of `mix` and is not
//! made one for a test: what is asserted is that the daemon's answer and the directory agree, plus
//! `php` by name — which is the one command Phase 2's milestone is about.

mod harness;

use std::collections::BTreeSet;

use harness::Home;
use serde_json::Value;

/// The command whose existence is the milestone, as the file in `bin/` is named on this system.
fn php() -> String {
    format!("php{}", std::env::consts::EXE_SUFFIX)
}

/// Every file in this home's `bin/`.
fn listing(home: &Home) -> BTreeSet<String> {
    std::fs::read_dir(home.path().join("bin"))
        .expect("a daemon that started has a bin/")
        .flatten()
        .map(|entry| entry.file_name().to_string_lossy().into_owned())
        .collect()
}

/// **The last piece of Phase 2's milestone.** T25 built a shim nothing put anywhere; a daemon that
/// has started is now a home whose `bin/` is a directory of commands.
#[test]
fn a_daemon_that_has_started_has_filled_bin() {
    let home = Home::new();
    let _daemon = home.start_daemon();

    let found = listing(&home);

    assert!(
        found.contains(&php()),
        "{} is missing from bin/, which holds {found:?}\n--- daemon.log ---\n{}",
        php(),
        home.daemon_log()
    );

    // More than one, because a `bin/` with only `php` in it would be a table that was not walked.
    assert!(found.len() > 1, "{found:?}");
}

/// The read, over the wire, against whatever this machine's PATH actually is.
///
/// **Nothing here asserts `on_path` either way.** Whether `<root>/bin` is on the PATH of the account
/// running the tests is a fact about that account — and the directory in question is a temporary one
/// that has never been on anybody's PATH — so a test that demanded an answer would be asserting
/// about the machine rather than about the command.
#[test]
fn status_reports_what_is_really_in_bin_in_both_renderings() {
    let home = Home::new();
    let _daemon = home.start_daemon();

    let json = home.mix(&["path", "status", "--json"]);
    assert!(
        json.status.success(),
        "{}",
        String::from_utf8_lossy(&json.stderr)
    );

    let report: Value = serde_json::from_slice(&json.stdout).expect("one object on stdout");

    assert_eq!(
        report["directory"].as_str(),
        Some(home.path().join("bin").display().to_string().as_str())
    );
    assert!(report["on_path"].is_boolean(), "{report}");
    assert!(report["places"].is_array(), "{report}");

    // The answer is read off the directory rather than composed from the daemon's own constants,
    // which is the whole reason it can be checked against one.
    let reported: BTreeSet<String> =
        serde_json::from_value(report["commands"].clone()).expect("the commands it found");
    assert_eq!(reported, listing(&home));

    // Nothing was left in `bin/` that answers to no command — the ordinary case, and the one that
    // would stop being true silently if a row were ever renamed without the sweep working.
    assert!(report.get("stale").is_none(), "{report}");

    let human = home.mix(&["path", "status"]);
    let rendered = String::from_utf8_lossy(&human.stdout);

    assert!(
        rendered.contains(&home.path().join("bin").display().to_string()),
        "{rendered}"
    );
    assert!(
        rendered.contains("this user's PATH") && rendered.contains("php"),
        "{rendered}"
    );
}

/// A subcommand that does not exist is refused by the client, without a daemon being started for it.
#[test]
fn a_subcommand_that_does_not_exist_is_refused_by_the_client() {
    let home = Home::new();

    let output = home.mix(&["path", "somewhere-else"]);

    assert!(!output.status.success());
    assert!(
        String::from_utf8_lossy(&output.stderr).contains("somewhere-else"),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
}
