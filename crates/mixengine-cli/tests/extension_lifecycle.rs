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

/// A `web-app` on the phpMyAdmin fixture's shape, served on an internal domain — roadmap task
/// **T81b**.
const PHPMYADMIN: &str = r#"
schema = 1

[extension]
id = "phpmyadmin"
name = "phpMyAdmin"
version = "5.2.1"
kind = "web-app"
description = "Web front end for MySQL and MariaDB"
homepage = "https://www.phpmyadmin.net"

[artifact.any]
url = "https://files.phpmyadmin.net/phpMyAdmin/5.2.1/phpMyAdmin-5.2.1-all-languages.zip"
sha256 = "0000000000000000000000000000000000000000000000000000000000000004"

[web-app]
root = "{install_dir}/app"
domain = "phpmyadmin"

[web-app.database]
engines = ["mariadb", "mysql"]

[web-app.runtime]
kind = "php"
requires = "^8.1"

[web-app.config]
path = "config.inc.php"
text = """
<?php
$cfg['blowfish_secret'] = '{secret}';
$cfg['TempDir'] = '{data_dir}';
if (true) {
    $cfg['Servers'][1]['host'] = '{db_host}';
    $cfg['Servers'][1]['port'] = '{db_port}';
    $cfg['Servers'][1]['user'] = '{db_user}';
}
@include '{data_dir}/config.user.php';
"""

[permissions]
services = ["read"]
network = "loopback"
filesystem = ["own-data"]
"#;

/// **T81b.** A web-app is served on a site its extension owns: the plan names it, the listings show
/// it, sharing it is refused, and the uninstall releases it.
#[tokio::test(flavor = "multi_thread")]
async fn a_web_app_is_a_site_its_extension_owns() {
    let home = Home::new();
    let _daemon = home.start_daemon();
    mixengine_testkit::declare::php_pool(&home.database_file(), "8.3.34").await;
    mixengine_testkit::declare::database(&home.database_file(), "mariadb@main", "mariadb", 3306)
        .await;
    let directory = extension(PHPMYADMIN);
    std::fs::create_dir_all(directory.path().join("app")).expect("a doc root");
    let path = directory.path().display().to_string();

    let planned = stdout(&home.mix(&["extension", "plan", "--path", &path]));
    assert!(
        planned.contains("https://phpmyadmin.mixengine.test, on php-fpm@8.3.34"),
        "{planned}"
    );

    let installed = home.mix(&["extension", "install", "--path", &path, "--yes"]);
    assert!(
        installed.status.success(),
        "{}\n{}\n{}",
        stdout(&installed),
        stderr(&installed),
        home.daemon_log()
    );

    let sites = stdout(&home.mix(&["site", "list"]));
    assert!(sites.contains("phpmyadmin.mixengine.test"), "{sites}");
    assert!(sites.contains("extension phpmyadmin"), "{sites}");

    let extensions = stdout(&home.mix(&["extension", "list"]));
    assert!(
        extensions.contains("phpmyadmin.mixengine.test"),
        "{extensions}"
    );

    let shared = home.mix(&["site", "share", "phpmyadmin.mixengine.test"]);
    assert!(!shared.status.success(), "{}", stdout(&shared));
    assert!(
        stderr(&shared).contains("belongs to the phpmyadmin extension"),
        "{}",
        stderr(&shared)
    );

    let removed = stdout(&home.mix(&["extension", "uninstall", "phpmyadmin"]));
    assert!(
        removed.contains("released phpmyadmin.mixengine.test"),
        "{removed}"
    );
    assert!(
        stdout(&home.mix(&["site", "list"])).contains("no sites are declared"),
        "the site outlived its extension"
    );
}

/// **T82.** The generated configuration is written into the served root, with the database the
/// install linked substituted into it — and the link is what makes `service.delete` refuse.
///
/// Four claims in one test because they are one mechanism: writing the link is what arms the
/// refusal (the design's D4), so proving the refusal separately would prove it against a row nothing
/// wrote.
#[tokio::test(flavor = "multi_thread")]
async fn a_web_app_is_configured_from_the_database_it_was_linked_to() {
    let home = Home::new();
    let _daemon = home.start_daemon();
    mixengine_testkit::declare::php_pool(&home.database_file(), "8.3.34").await;
    mixengine_testkit::declare::database(&home.database_file(), "mariadb@main", "mariadb", 13306)
        .await;
    let directory = extension(PHPMYADMIN);
    std::fs::create_dir_all(directory.path().join("app")).expect("a doc root");
    let path = directory.path().display().to_string();

    let installed = home.mix(&["extension", "install", "--path", &path, "--yes"]);
    assert!(
        installed.status.success(),
        "{}\n{}\n{}",
        stdout(&installed),
        stderr(&installed),
        home.daemon_log()
    );

    let config = home
        .path()
        .join("extensions")
        .join("phpmyadmin")
        .join("app")
        .join("config.inc.php");
    let written = std::fs::read_to_string(&config)
        .unwrap_or_else(|error| panic!("{} should be there: {error}", config.display()));

    assert!(written.contains("'127.0.0.1'"), "{written}");
    assert!(written.contains("'13306'"), "{written}");
    assert!(written.contains("'root'"), "{written}");
    // The application's own braces are the destination language's punctuation, not ours — D8.
    assert!(written.contains("if (true) {"), "{written}");
    // And `{secret}` was answered by something, rather than left standing as a literal brace.
    assert!(!written.contains("{secret}"), "{written}");

    // **The link armed the refusal that was already there** — D4. No new refusal exists for this.
    let deleted = home.mix(&["service", "delete", "mariadb@main"]);
    assert!(!deleted.status.success(), "{}", stdout(&deleted));
    assert!(
        stderr(&deleted).contains("phpmyadmin.mixengine.test"),
        "{}",
        stderr(&deleted)
    );

    // The generated file goes with the install directory; what a person wrote does not.
    let user_half = home
        .path()
        .join("data")
        .join("extensions")
        .join("phpmyadmin")
        .join("config.user.php");
    std::fs::write(&user_half, b"<?php // mine\n").expect("a user configuration");

    let removed = home.mix(&["extension", "uninstall", "phpmyadmin"]);
    assert!(removed.status.success(), "{}", stderr(&removed));
    assert!(!config.exists(), "the generated file outlived the install");
    assert!(user_half.exists(), "an uninstall keeps what a person wrote");
}
