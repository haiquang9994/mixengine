//! `mix autostart` against a real `mixengined` — roadmap task **T85b**.
//!
//! **Only the read is end to end, and that is deliberate rather than a gap.** `autostart.enable`
//! registers a logon task, a LaunchAgent or a systemd user unit *for the account running it*, so a
//! suite that called it would be a `cargo test` that arranges for a daemon to start at every login
//! of whoever ran it — on a developer's laptop, and on a CI runner that keeps its home directory
//! between jobs. The two mutations are proved where they can be proved against something the test
//! owns: the real ones in `mixengine-platform`, against a scratch task name and a `TempDir` each
//! test creates and removes, and the dispatch and the wire shape in `mixengine-daemon`'s own tests
//! against `mock::Host`. This is the `mix path` suite's arrangement, one step more careful because
//! the thing being written outlives the machine's next reboot.
//!
//! What is left is the half nothing else covers: that a real daemon reads this machine, answers in
//! both renderings, and changes nothing while doing it.

mod harness;

use harness::Home;
use serde_json::Value;

/// The read, over the wire, against whatever this machine actually has.
///
/// **Nothing here asserts `enabled` either way.** Whether this account has an autostart entry is a
/// fact about the account — and on a machine that does have one, it names somebody's real home and
/// not the temporary one this test made — so a test that demanded an answer would be asserting about
/// the machine rather than about the command. What is asserted is the shape, and that a status is
/// not a write.
#[test]
fn status_reads_this_machine_and_changes_nothing() {
    let home = Home::new();
    let _daemon = home.start_daemon();

    let first = home.mix(&["autostart", "status", "--json"]);
    assert!(
        first.status.success(),
        "{}",
        String::from_utf8_lossy(&first.stderr)
    );

    let report: Value = serde_json::from_slice(&first.stdout).expect("one object on stdout");

    assert!(
        matches!(
            report["mechanism"].as_str(),
            Some("logon_task" | "launch_agent" | "systemd_user" | "none")
        ),
        "{report}"
    );
    assert!(report["location"].is_string(), "{report}");
    assert!(report["enabled"].is_boolean(), "{report}");
    assert_eq!(report["changed"], false, "a status never claims a write");

    // A temporary home has no entry of its own, whatever this machine holds for the account's real
    // one — which is the one thing this suite can assert about `for_this_home` without asserting
    // about the machine.
    assert_eq!(report["for_this_home"], false, "{report}");

    // Asking twice says the same thing, which is what "changes nothing" means from outside.
    let again = home.mix(&["autostart", "status", "--json"]);
    let repeated: Value = serde_json::from_slice(&again.stdout).expect("one object on stdout");
    assert_eq!(repeated, report);
}

/// The two renderings are the same answer, and the human one names where to go and look.
#[test]
fn the_two_renderings_agree_about_where_the_entry_would_live() {
    let home = Home::new();
    let _daemon = home.start_daemon();

    let json = home.mix(&["autostart", "status", "--json"]);
    let report: Value = serde_json::from_slice(&json.stdout).expect("one object on stdout");
    let location = report["location"].as_str().expect("a location").to_owned();

    let human = home.mix(&["autostart", "status"]);
    assert!(
        human.status.success(),
        "{}",
        String::from_utf8_lossy(&human.stderr)
    );

    let rendered = String::from_utf8_lossy(&human.stdout);

    assert!(rendered.contains(&location), "{rendered}\n--- {location}");
    assert!(rendered.contains("log in"), "{rendered}");
}

/// A subcommand that does not exist is refused by the client, without a daemon being started for it.
#[test]
fn a_subcommand_that_does_not_exist_is_refused_by_the_client() {
    let home = Home::new();

    let output = home.mix(&["autostart", "at-boot"]);

    assert!(!output.status.success());
    assert!(
        String::from_utf8_lossy(&output.stderr).contains("at-boot"),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
}
