//! `mix extension install/list/uninstall` against a real daemon — roadmap task **T81**.
//!
//! `extension_manifest.rs` proves the reading of a manifest; this proves the *doing*. What is only
//! true here, and is what the design spends its arguments on:
//!
//! - a `--path` install is marked unsigned wherever it is printed, for as long as it is installed;
//! - installing asks about what the extension declares before it installs anything, and `--yes` is
//!   the answer given in advance;
//! - the ports it holds are real, and appear in the listing;
//! - and an uninstall keeps the data directory, saying where it still is.
//!
//! The extension installed here runs nothing: its `[service]` names a program that does not exist,
//! which is fine because nothing starts it. What a *started* extension does is the supervisor's,
//! and it is the same walk `mix service start` takes — the design's D11.

mod harness;

use harness::{Home, json, stderr, stdout};

/// A `recipe` extension: no artifact, nothing to supervise, and a php.ini line.
///
/// Written out here rather than pulled from `mixengine-testkit`, on `extension_manifest.rs`' rule:
/// `mixengine-core` is not a dependency of `mix` and is not made one for a test.
const SENDMAIL: &str = r#"
schema = 1

[extension]
id = "sendmail-to-mailpit"
name = "Send mail to Mailpit"
version = "1.0.0"
kind = "recipe"
description = "Point every managed PHP's mail() at a Mailpit already on this machine"

[[recipe.php_ini]]
key = "sendmail_path"
value = "{install_dir}/sendmail.sh"

[permissions]
filesystem = ["own-data"]
"#;

/// A `service` extension holding two ports.
const MAILPIT: &str = r#"
schema = 1

[extension]
id = "mailpit"
name = "Mailpit"
version = "1.20.0"
kind = "service"
description = "Local SMTP capture and web UI"

[ports]
ui_port = 18025
smtp_port = 11025

[service]
program = "{install_dir}/mailpit"
cwd = "{data_dir}"
args = ["--listen", "{listen}:{ui_port}", "--smtp", "{listen}:{smtp_port}"]
ready = { type = "tcp", addr = "{listen}:{ui_port}", timeout = "10s" }

[permissions]
services = ["read"]
network = "loopback"
filesystem = ["own-data"]
"#;

/// A directory holding one `extension.toml` and a file its `[service]` names.
fn extension(body: &str) -> tempfile::TempDir {
    let directory = tempfile::Builder::new()
        .prefix("mixengine-extension")
        .tempdir()
        .expect("a temporary directory");

    std::fs::write(directory.path().join("extension.toml"), body).expect("a manifest");
    std::fs::write(directory.path().join("mailpit"), b"#!/bin/true\n").expect("a program");

    directory
}

/// **The whole loop, and the unsigned marker along it** — the design's D9 and D12.
#[tokio::test(flavor = "multi_thread")]
async fn a_directory_install_is_unsigned_all_the_way_through() {
    let home = Home::new();
    let _daemon = home.start_daemon();
    let directory = extension(MAILPIT);
    let path = directory.path().display().to_string();

    // What it declares is shown before anything is installed, and the plan installs nothing.
    let planned = stdout(&home.mix(&["extension", "plan", "--path", &path]));
    assert!(planned.contains("UNSIGNED"), "{planned}");
    assert!(
        planned.contains("127.0.0.1"),
        "the reach is shown: {planned}"
    );
    assert!(
        planned.contains("not a permission MixEngine enforces"),
        "`services` is a declaration shown to somebody, not a grant: {planned}"
    );
    assert!(
        stdout(&home.mix(&["extension", "list"])).contains("nothing is installed"),
        "a plan installed something"
    );

    let installed = home.mix(&["extension", "install", "--path", &path, "--yes"]);
    assert!(installed.status.success(), "{}", stderr(&installed));

    let listed = stdout(&home.mix(&["extension", "list"]));
    assert!(listed.contains("mailpit"), "{listed}");
    assert!(
        listed.contains("unsigned"),
        "a directory install stays marked: {listed}"
    );
    assert!(
        listed.contains("ui_port=18025"),
        "the ports it holds are what it was given: {listed}"
    );

    let answered = json(&home.mix(&["extension", "list", "--json"]));
    let one = &answered["extensions"][0];
    assert_eq!(one["id"], "mailpit", "{answered}");
    assert_eq!(one["signed"], false, "{answered}");
    assert_eq!(one["service"], "mailpit", "{answered}");

    // **Uninstalling keeps the data** — and says where it is.
    let removed = stdout(&home.mix(&["extension", "uninstall", "mailpit"]));
    assert!(removed.contains("was uninstalled"), "{removed}");
    assert!(removed.contains("its data was kept"), "{removed}");

    assert!(
        stdout(&home.mix(&["extension", "list"])).contains("nothing is installed"),
        "the row survived the uninstall"
    );
}

/// A kind that runs nothing installs, lists and uninstalls with no service beside it.
#[tokio::test(flavor = "multi_thread")]
async fn something_that_runs_nothing_still_installs() {
    let home = Home::new();
    let _daemon = home.start_daemon();
    let directory = extension(SENDMAIL);
    let path = directory.path().display().to_string();

    let installed = home.mix(&["extension", "install", "--path", &path, "--yes"]);
    assert!(installed.status.success(), "{}", stderr(&installed));

    let answered = json(&home.mix(&["extension", "list", "--json"]));
    let one = &answered["extensions"][0];
    assert_eq!(one["id"], "sendmail-to-mailpit", "{answered}");
    assert_eq!(one["kind"], "recipe", "{answered}");
    assert_eq!(one["service"], serde_json::Value::Null, "{answered}");

    let removed = home.mix(&["extension", "uninstall", "sendmail-to-mailpit"]);
    assert!(removed.status.success(), "{}", stderr(&removed));
}

/// Installing one twice is refused by name rather than by a constraint.
#[tokio::test(flavor = "multi_thread")]
async fn one_extension_is_installed_once() {
    let home = Home::new();
    let _daemon = home.start_daemon();
    let directory = extension(MAILPIT);
    let path = directory.path().display().to_string();

    home.mix(&["extension", "install", "--path", &path, "--yes"]);

    let again = home.mix(&["extension", "install", "--path", &path, "--yes"]);

    assert!(!again.status.success(), "{}", stdout(&again));
    let said = stderr(&again);
    assert!(said.contains("already installed"), "{said}");
}

/// Naming neither a registry entry nor a directory is a usage error, not a call.
#[tokio::test(flavor = "multi_thread")]
async fn an_install_names_one_source_or_the_other() {
    let home = Home::new();
    let _daemon = home.start_daemon();

    let refused = home.mix(&["extension", "install", "--yes"]);

    assert!(!refused.status.success(), "{}", stdout(&refused));
    assert!(stderr(&refused).contains("--path"), "{}", stderr(&refused));
}

/// Starting something that runs no process says so about the extension rather than failing as if
/// the call were wrong.
#[tokio::test(flavor = "multi_thread")]
async fn starting_something_that_runs_nothing_says_what_it_is() {
    let home = Home::new();
    let _daemon = home.start_daemon();
    let directory = extension(SENDMAIL);
    let path = directory.path().display().to_string();

    home.mix(&["extension", "install", "--path", &path, "--yes"]);

    let refused = home.mix(&["extension", "start", "sendmail-to-mailpit"]);

    assert!(!refused.status.success(), "{}", stdout(&refused));
    let said = stderr(&refused);
    assert!(said.contains("runs no process"), "{said}");
}
