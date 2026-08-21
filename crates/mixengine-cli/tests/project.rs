//! `mix project` against a real daemon.
//!
//! Roadmap task **T39**'s client half. What the daemon's own `tests/projects.rs` proves is that the
//! methods do what they say; what is proved here is the part that is only true of `mix` — that the
//! arguments a person types reach the right method, that `create` and `import` are one subcommand
//! under two names, that a command typed inside a project finds it without being told, and that the
//! human rendering says the sentence a person needs.

mod harness;

use harness::{Home, json, stdout};

/// A directory to register, and its manifest when it has one.
fn repository(body: Option<&str>) -> tempfile::TempDir {
    let directory = tempfile::Builder::new()
        .prefix("mixengine-project")
        .tempdir()
        .expect("a temporary directory");

    if let Some(body) = body {
        std::fs::write(directory.path().join("mixengine.toml"), body).expect("a manifest");
    }

    directory
}

/// The sequence a person actually types, in the order they type it.
#[tokio::test(flavor = "multi_thread")]
async fn a_project_is_created_shown_from_inside_and_exported_from_the_command_line() {
    let home = Home::new();
    let _daemon = home.start_daemon();
    let repository = repository(None);
    let root = repository.path().display().to_string();

    let empty = stdout(&home.mix(&["project", "list"]));
    assert!(
        empty.contains("no projects"),
        "an empty home says so rather than printing a heading with nothing under it: {empty}"
    );

    let created = json(&home.mix(&["project", "create", &root, "--name", "blog", "--json"]));
    assert_eq!(created["project"]["name"], "blog", "{created}");

    let listed = stdout(&home.mix(&["project", "list"]));
    assert!(listed.contains("blog"), "{listed}");

    // **From inside**, with nothing named: which project this is is the daemon's answer, and `mix`
    // only says which directory it is in.
    let inside = repository.path().join("public");
    std::fs::create_dir(&inside).expect("a directory");
    let shown = json(&home.mix_in(&inside, &[], &["project", "show", "--json"]));
    assert_eq!(shown["project"]["name"], "blog", "{shown}");

    let exported = json(&home.mix_in(&inside, &[], &["project", "export", "--json"]));
    assert_eq!(exported["created"], true, "{exported}");
    let written =
        std::fs::read_to_string(repository.path().join("mixengine.toml")).expect("the manifest");
    assert!(written.contains("name = \"blog\""), "{written}");

    // Compared against the spelling the daemon registered rather than the one this test typed:
    // `paths::in_full` settles 8.3 aliases and symlinks on the way in, and `%TEMP%` on a Windows
    // runner and `/tmp` on macOS are both paths that come back spelled differently.
    let registered = created["project"]["root"]
        .as_str()
        .expect("a create answers with the root it registered");

    let removed = stdout(&home.mix(&["project", "delete", "blog"]));
    assert!(
        removed.contains(registered),
        "the directory that was kept is named: {removed}"
    );
}

/// **D2.** `import` is a second name for `create`, so both reach the same state.
#[tokio::test(flavor = "multi_thread")]
async fn create_and_import_are_one_subcommand_under_two_names() {
    let home = Home::new();
    let _daemon = home.start_daemon();
    let repository = repository(Some(
        "[project]\nname = \"shop\"\n\n[runtimes]\nphp = \"^8.3\"\n",
    ));

    let imported = json(&home.mix(&[
        "project",
        "import",
        &repository.path().display().to_string(),
        "--json",
    ]));

    assert_eq!(imported["project"]["name"], "shop", "{imported}");
    assert_eq!(imported["pins"][0]["constraint"], "^8.3", "{imported}");
    assert_eq!(imported["pins"][0]["source"]["from"], "manifest");
}

/// A name that is not a handle is refused by the daemon, and `mix` prints its sentence and exits
/// non-zero — which is what a script branches on.
#[tokio::test(flavor = "multi_thread")]
async fn a_refused_create_exits_non_zero_and_says_why() {
    let home = Home::new();
    let _daemon = home.start_daemon();
    let repository = repository(None);

    let output = home.mix(&[
        "project",
        "create",
        &repository.path().display().to_string(),
        "--name",
        "blog/site",
    ]);

    assert!(!output.status.success(), "{output:?}");
    assert!(
        String::from_utf8_lossy(&output.stderr).contains("path separator"),
        "{output:?}"
    );
}
