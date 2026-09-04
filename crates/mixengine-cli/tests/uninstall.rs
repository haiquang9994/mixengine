//! `mix uninstall` against a real daemon — roadmap task **T87**.
//!
//! **The unignored half of this file may never remove anything from the machine running it.** Every
//! test here runs on somebody's own workstation, so what is proved without `--ignored` is the plan,
//! the refusals and the `--keep-home` path. The round trip that actually takes MixEngine off a
//! machine is `#[ignore]`d and runs on a fresh runner in CI's `system` job, which is the clean VM
//! the task asks for.
//!
//! **And nothing here may assume the machine running it is clean.** A developer's machine has a
//! helper installed, an audit log, a `PATH` entry and a hosts block of its own home's; every
//! assertion below is therefore about *this* home's plan and about what the command did not do,
//! never about the machine being empty to begin with.

mod harness;

use harness::{Home, json, stderr, stdout};

/// The plan names every row, whatever this machine holds, and changes nothing.
#[tokio::test(flavor = "multi_thread")]
async fn a_dry_run_names_every_row_and_changes_nothing() {
    let home = Home::new();
    let _daemon = home.start_daemon();

    let before = home.contents();

    // **Read before the plan and compared after, rather than asserted empty.** A daemon that has
    // just started has already asked for whatever its first run needs — the certificate authority,
    // on this machine — and a test demanding an empty queue would be asserting that first-run setup
    // does not happen. What is being proved is that the *plan* added nothing to it.
    let queued = json(&home.mix(&["elevation", "status", "--json"]))["pending"].clone();

    let report = json(&home.mix(&["uninstall", "--dry-run", "--json"]));

    let items = report["items"].as_array().expect("a list of rows");
    assert!(items.len() >= 11, "{report}");

    for row in items {
        assert!(
            row["what"].as_str().is_some_and(|what| !what.is_empty()),
            "{row}"
        );
        assert!(
            row["location"]
                .as_str()
                .is_some_and(|place| !place.is_empty()),
            "every row says where to go and look: {row}"
        );

        // A dry run acts on nothing, so no row may claim a removal or a queue entry.
        let removal = row["outcome"]["removal"]
            .as_str()
            .expect("a tagged outcome");
        assert!(
            matches!(removal, "planned" | "absent" | "kept" | "failed"),
            "{row}"
        );
    }

    assert!(report["granting"].is_null(), "a plan raises no prompt");
    assert_eq!(home.contents(), before, "a dry run wrote something");

    // And it asked for nothing: an operation a plan left behind would be the prompt it promised not
    // to raise, arriving at whatever the person did next.
    let waiting = json(&home.mix(&["elevation", "status", "--json"]));
    assert_eq!(waiting["pending"], queued, "the plan enqueued something");
}

/// And the home row names the home, so the one irreversible thing on the list is the one a person
/// cannot miss.
#[tokio::test(flavor = "multi_thread")]
async fn the_plan_names_the_home_it_would_remove() {
    let home = Home::new();
    let _daemon = home.start_daemon();

    let printed = stdout(&home.mix(&["uninstall", "--dry-run"]));

    assert!(
        printed.contains(&home.path().display().to_string()),
        "{printed}"
    );
    assert!(printed.contains("data/"), "{printed}");
}

/// `--keep-home` says so on the home row rather than leaving it out. A person reading the plan has
/// to see that the home was considered and deliberately left.
#[tokio::test(flavor = "multi_thread")]
async fn keeping_the_home_is_a_row_and_not_a_silence() {
    let home = Home::new();
    let _daemon = home.start_daemon();

    let report = json(&home.mix(&["uninstall", "--dry-run", "--keep-home", "--json"]));

    let kept = report["items"]
        .as_array()
        .expect("rows")
        .iter()
        .find(|row| row["id"] == "home")
        .expect("the home is always a row");

    assert_eq!(kept["outcome"]["removal"], "kept", "{report}");
}

/// Nobody at the keyboard is not a yes. `mix` reads end of file as *there was nobody to ask* and
/// names the flag that answers in advance — `mix elevation grant`'s standing rule, on the one
/// command where getting it wrong removes somebody's databases.
#[tokio::test(flavor = "multi_thread")]
async fn an_unattended_uninstall_without_yes_removes_nothing() {
    let home = Home::new();
    let _daemon = home.start_daemon();

    let refused = home.mix(&["uninstall"]);

    assert!(
        home.path().exists(),
        "the home was removed with nobody asked"
    );
    assert!(stderr(&refused).contains("--yes"), "{}", stderr(&refused));
    assert_ne!(refused.status.code(), Some(0), "{}", stdout(&refused));
}

/// Typing anything but yes is a decline, and a decline removes nothing and fails nothing.
#[tokio::test(flavor = "multi_thread")]
async fn answering_no_removes_nothing_and_is_not_a_failure() {
    let home = Home::new();
    let _daemon = home.start_daemon();

    let before = home.contents();
    let answered = home.mix_answering("n\n", &["uninstall"]);

    assert_eq!(answered.status.code(), Some(0), "{}", stderr(&answered));
    assert!(home.path().exists());
    assert_eq!(home.contents(), before);
}
