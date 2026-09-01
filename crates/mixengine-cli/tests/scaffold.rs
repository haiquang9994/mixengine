//! Importing a blueprint somebody else wrote, and running its own command — roadmap task **T78a**.
//!
//! What is proved here is the whole of the task end to end and what no unit test can: that a
//! hand-written blueprint arrives *untrusted*, that its `[scaffold]` command does not run because
//! the apply was sent, and that it does run — in the new project's directory — when somebody says
//! so with the gesture an unsigned blueprint takes.
//!
//! **Offline by construction**, as `tests/blueprint.rs` is: the fixture is a static site with no
//! runtime and no services, so nothing here reaches the package index. The command the blueprint
//! carries is written per operating system by this file, which is the honest consequence of running
//! it through the OS shell.

mod harness;

use harness::{Home, json, stderr, stdout};

/// A directory for a new project.
fn repository() -> tempfile::TempDir {
    tempfile::Builder::new()
        .prefix("mixengine-scaffold")
        .tempdir()
        .expect("a temporary directory")
}

/// A command that writes a file, on this system.
fn a_command_that_writes_a_file() -> &'static str {
    if cfg!(windows) {
        "echo hello> made.txt"
    } else {
        "printf hello > made.txt"
    }
}

/// A blueprint written by hand, carrying the one section a capture never writes.
fn a_blueprint_with_a_command(command: &str) -> String {
    format!(
        r#"schema = 1

[blueprint]
name = "borrowed"
description = "somebody else's stack"
created_at = "2026-09-01T00:00:00Z"
created_on = {{ os = "linux", version = "0.1.0" }}

[site]
kind = "static"
doc_root = "public"
https = false
domain_pattern = "{{project}}.test"

[scaffold]
command = "{command}"
"#
    )
}

/// Write one into the home and import it, answering with the summary the daemon wrote down.
fn imported(home: &Home) -> serde_json::Value {
    let file = home.path().join("borrowed.toml");
    std::fs::write(
        &file,
        a_blueprint_with_a_command(a_command_that_writes_a_file()),
    )
    .expect("a blueprint to import");

    json(&home.mix(&["blueprint", "import", &file.display().to_string(), "--json"]))
}

/// **Nothing vouched for it, so it is untrusted** — and it stays that way, because no method in this
/// build raises the flag once the row is written.
#[tokio::test(flavor = "multi_thread")]
async fn a_hand_written_blueprint_arrives_untrusted() {
    let home = Home::new();
    let _daemon = home.start_daemon();

    let summary = imported(&home);

    assert_eq!(summary["source"], "imported", "{summary}");
    assert_eq!(summary["trusted"], false, "{summary}");

    // **Found by slug rather than by position** — roadmap task T79 put six built-in blueprints in
    // every home, so index zero is whichever gallery slug sorts first and not this one.
    let listed = json(&home.mix(&["blueprint", "list", "--json"]));
    let slug = summary["slug"].as_str().expect("a slug");
    let found = listed["blueprints"]
        .as_array()
        .expect("a listing")
        .iter()
        .find(|one| one["slug"] == slug)
        .unwrap_or_else(|| panic!("the imported blueprint is not listed: {listed}"));

    assert_eq!(found["trusted"], false, "a listing says it too: {listed}");
}

/// **The command runs only when somebody agrees to it, and then it really runs** — the task's whole
/// subject. Both halves in one test, because what makes the second meaningful is the first: a build
/// that ran the command either way would pass half of this.
#[tokio::test(flavor = "multi_thread")]
async fn a_blueprints_own_command_runs_only_when_asked_for() {
    let home = Home::new();
    let _daemon = home.start_daemon();

    imported(&home);

    // Without the gesture: everything else is applied, and the command is left as a sentence.
    let first = repository();
    let one = first.path().join("one");
    let said = stdout(&home.mix(&[
        "blueprint",
        "apply",
        "borrowed",
        "--project",
        "one",
        "--path",
        &one.display().to_string(),
    ]));

    assert!(said.contains("not run"), "{said}");
    assert!(
        !one.join("made.txt").exists(),
        "nobody agreed to it, so it did not run"
    );

    // With it: the command runs, in the project's own directory.
    let second = repository();
    let two = second.path().join("two");
    let ran = stdout(&home.mix(&[
        "blueprint",
        "apply",
        "borrowed",
        "--project",
        "two",
        "--path",
        &two.display().to_string(),
        "--run-untrusted-scaffold",
    ]));

    assert!(ran.contains("done"), "{ran}");
    assert!(
        two.join("made.txt").is_file(),
        "the command ran in the new project's directory: {ran}"
    );
}

/// **`--run-scaffold` is not the gesture an unsigned blueprint takes** — roadmap task **T78a**, its
/// design's D15 — and the refusal names the one that is rather than leaving somebody guessing.
#[tokio::test(flavor = "multi_thread")]
async fn the_flag_for_a_signed_blueprint_does_not_answer_for_an_unsigned_one() {
    let home = Home::new();
    let _daemon = home.start_daemon();

    imported(&home);

    let directory = repository();
    let root = directory.path().join("three");
    let output = home.mix(&[
        "blueprint",
        "apply",
        "borrowed",
        "--project",
        "three",
        "--path",
        &root.display().to_string(),
        "--run-scaffold",
    ]);

    let complaint = stderr(&output);

    assert!(
        complaint.contains("--run-untrusted-scaffold"),
        "{complaint}"
    );
    assert!(
        !root.join("made.txt").exists(),
        "and nothing ran while it was being refused"
    );
}

/// **A closed standard input is not agreement.** A person who declines gets the project without the
/// command rather than no project — the same outcome as sending no consent at all.
#[tokio::test(flavor = "multi_thread")]
async fn declining_the_question_leaves_the_command_and_keeps_the_project() {
    let home = Home::new();
    let _daemon = home.start_daemon();

    imported(&home);

    let directory = repository();
    let root = directory.path().join("four");
    let said = stdout(&home.mix_answering(
        "n",
        &[
            "blueprint",
            "apply",
            "borrowed",
            "--project",
            "four",
            "--path",
            &root.display().to_string(),
        ],
    ));

    assert!(
        said.contains(a_command_that_writes_a_file()),
        "the question shows the command exactly as it would run: {said}"
    );
    assert!(!root.join("made.txt").exists(), "{said}");

    // And the project it made is there, which is what "declining is not a failure" means.
    let shown = json(&home.mix(&["project", "show", "four", "--json"]));
    assert_eq!(shown["project"]["name"], "four", "{shown}");
}
