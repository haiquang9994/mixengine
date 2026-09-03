//! `mix database client` and `mix database open` against the real locator of this system —
//! roadmap task **T83**.
//!
//! **The (P) half.** What answers here is this machine's own registry, Spotlight or XDG walk, asked
//! for an application no machine has: each system's lookup runs for real and is expected to say
//! "not installed" in its own words. What the methods do when the application *is* there is proved
//! on a mock host in `crates/mixengine-daemon/src/databases.rs`, and once against a real MariaDB
//! and a real credential in `mariadb.rs`.

mod harness;

use harness::{Home, json, stderr, stdout};

/// A `desktop-app` whose hints name something that exists nowhere.
const NOWHERE: &str = r#"schema = 1

[extension]
id = "nowhere"
name = "Nowhere"
version = "0.1.0"
kind = "desktop-app"
description = "A desktop client no machine has"
homepage = "https://example.invalid/nowhere"

[desktop-app]
scheme = "nowhere"

[desktop-app.detect]
windows = "mixengine-test-nothing.exe"
macos = "test.mixengine.nothing"
linux = "mixengine-test-nothing.desktop"

[permissions]
network = "loopback"
"#;

/// A directory holding one `extension.toml`.
fn extension(body: &str) -> tempfile::TempDir {
    let directory = tempfile::Builder::new()
        .prefix("mixengine-desktop")
        .tempdir()
        .expect("a temporary directory");

    std::fs::write(directory.path().join("extension.toml"), body).expect("a manifest");

    directory
}

/// No `desktop-app` extension is a state both commands print, and `open` says what to install.
#[test]
fn with_no_desktop_app_installed_both_commands_say_no_client() {
    let home = Home::new();
    let _daemon = home.start_daemon();
    mixengine_testkit::declare::database_blocking(
        &home.database_file(),
        "redis@main",
        "redis",
        6379,
    );

    let report = json(&home.mix(&["database", "client", "redis@main", "--json"]));
    assert_eq!(report["protocol"], "redis", "{report}");
    assert_eq!(report["client"]["state"], "no_client", "{report}");

    let opened = home.mix(&["database", "open", "redis@main"]);
    assert_eq!(opened.status.code(), Some(1), "{}", stderr(&opened));
    assert!(
        stdout(&opened).contains("mix extension install mixdb"),
        "{}",
        stdout(&opened)
    );
}

/// The extension without the application: this system's own lookup answers, and says where it
/// looked and where to get it.
#[test]
fn an_application_no_machine_has_is_not_installed_through_this_systems_own_lookup() {
    let home = Home::new();
    let _daemon = home.start_daemon();
    mixengine_testkit::declare::database_blocking(
        &home.database_file(),
        "redis@main",
        "redis",
        6379,
    );

    let directory = extension(NOWHERE);
    let path = directory.path().display().to_string();
    let installed = home.mix(&["extension", "install", "--path", &path, "--yes"]);
    assert!(installed.status.success(), "{}", stderr(&installed));

    let report = json(&home.mix(&["database", "client", "redis@main", "--json"]));
    assert_eq!(report["client"]["state"], "not_installed", "{report}");
    assert_eq!(report["client"]["name"], "Nowhere", "{report}");
    assert!(
        !report["client"]["searched"]
            .as_str()
            .unwrap_or_default()
            .is_empty(),
        "{report}"
    );

    let opened = home.mix(&["database", "open", "redis@main"]);
    assert_eq!(opened.status.code(), Some(1), "{}", stderr(&opened));
    let said = stdout(&opened);
    assert!(said.contains("Nowhere is not installed"), "{said}");
    assert!(said.contains("https://example.invalid/nowhere"), "{said}");

    let human = stdout(&home.mix(&["database", "client", "redis@main"]));
    assert!(human.contains("redis"), "{human}");
    assert!(human.contains("not installed"), "{human}");
}

/// **The plan answers this machine, before anybody agrees to anything** — roadmap task **T84**,
/// the design's D2, and its (P) half: what says "not installed" here is this system's own registry
/// walk, Spotlight query or XDG walk, because no machine has the application the fixture names.
#[test]
fn a_desktop_app_plan_answers_this_machine_and_names_where_to_get_it() {
    let home = Home::new();
    let _daemon = home.start_daemon();

    let directory = extension(NOWHERE);
    let path = directory.path().display().to_string();

    let plan = json(&home.mix(&["extension", "plan", "--path", &path, "--json"]));
    assert_eq!(plan["client"]["state"], "not_installed", "{plan}");
    assert!(
        !plan["client"]["searched"]
            .as_str()
            .unwrap_or_default()
            .is_empty(),
        "it says where this system looked: {plan}"
    );
    assert_eq!(
        plan["homepage"], "https://example.invalid/nowhere",
        "{plan}"
    );

    let human = stdout(&home.mix(&["extension", "plan", "--path", &path]));
    assert!(human.contains("Nowhere is not on this machine"), "{human}");
    assert!(human.contains("https://example.invalid/nowhere"), "{human}");
    assert!(
        human.contains("MixEngine finds it rather than installing it"),
        "the version shown is the entry's, and the line says so: {human}"
    );

    // And installing it says the same thing again, where a person ends up — `--yes` skipped the
    // plan's render.
    let installed = home.mix(&["extension", "install", "--path", &path, "--yes"]);
    assert!(installed.status.success(), "{}", stderr(&installed));
    assert!(
        stderr(&installed).contains("Nowhere is not on this machine yet"),
        "{}",
        stderr(&installed)
    );
}

/// A `service` extension pays for none of that: nothing asks this machine about desktop
/// applications for a kind that is not one — roadmap task **T84**.
#[test]
fn a_plan_for_another_kind_carries_no_application_state() {
    let home = Home::new();
    let _daemon = home.start_daemon();

    let directory = extension(mixengine_testkit::extension::MAILPIT);
    let path = directory.path().display().to_string();

    let plan = json(&home.mix(&["extension", "plan", "--path", &path, "--json"]));
    assert!(plan.get("client").is_none(), "{plan}");
    assert_eq!(plan["homepage"], "https://mailpit.axllent.org", "{plan}");
}

/// A service no client opens is a state to `client` and a refusal to `open`.
#[test]
fn a_service_no_client_opens_is_said_in_those_words() {
    let home = Home::new();
    let _daemon = home.start_daemon();
    mixengine_testkit::declare::database_blocking(
        &home.database_file(),
        "memcached@main",
        "memcached",
        11211,
    );

    let report = json(&home.mix(&["database", "client", "memcached@main", "--json"]));
    assert!(report.get("protocol").is_none(), "{report}");

    let human = stdout(&home.mix(&["database", "client", "memcached@main"]));
    assert!(
        human.contains("not a database a desktop client opens"),
        "{human}"
    );

    let refused = home.mix(&["database", "open", "memcached@main"]);
    assert!(!refused.status.success());
    assert!(
        stderr(&refused).contains("memcached@main"),
        "{}",
        stderr(&refused)
    );
}
