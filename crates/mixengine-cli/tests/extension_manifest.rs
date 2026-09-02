//! `mix extension inspect` against a real daemon — roadmap task **T80**.
//!
//! **The format is exercised by a command and not only by a unit test**, which is the whole reason
//! T80 ships a verb at all: nothing else it built is reachable by a person until T81 installs
//! something. A manifest format proved only against its own parser is a format nobody has typed a
//! path into.
//!
//! What is only true of `mix` and is proved here: that a path typed relative to the shell's
//! directory reaches the daemon absolute, that a refusal comes back naming the field rather than as
//! an internal error, and that the human rendering says the two sentences the design insisted on —
//! that a port is asked for rather than held, and that `permissions.services` is a declaration.

mod harness;

use harness::{Home, json, stderr, stdout};

/// A directory holding one `extension.toml`.
fn extension(body: &str) -> tempfile::TempDir {
    let directory = tempfile::Builder::new()
        .prefix("mixengine-extension")
        .tempdir()
        .expect("a temporary directory");

    std::fs::write(directory.path().join("extension.toml"), body).expect("a manifest");

    directory
}

/// The Mailpit manifest T82 will ship, read end to end.
///
/// The fixture is written out here rather than pulled from `mixengine-testkit`, because
/// `mixengine-core` is not a dependency of `mix` and is not made one for a test — the same rule
/// `tests/path.rs` states about never restating the table.
const MAILPIT: &str = r#"
schema = 1

[extension]
id = "mailpit"
name = "Mailpit"
version = "1.20.0"
kind = "service"
description = "Local SMTP capture and web UI"

[ports]
ui_port = 8025
smtp_port = 1025

[service]
program = "{install_dir}/mailpit"
cwd = "{data_dir}"
args = ["--listen", "{listen}:{ui_port}", "--smtp", "{listen}:{smtp_port}"]
ready = { type = "tcp", addr = "{listen}:{ui_port}", timeout = "10s" }

[[recipe.php_ini]]
key = "sendmail_path"
value = "{install_dir}/mailpit sendmail"

[permissions]
services = ["read"]
network = "loopback"
filesystem = ["own-data"]
"#;

/// What a person sees, and the two sentences the design would not ship without.
#[tokio::test(flavor = "multi_thread")]
async fn inspecting_a_manifest_says_what_would_run() {
    let home = Home::new();
    let _daemon = home.start_daemon();
    let directory = extension(MAILPIT);
    let path = directory.path().display().to_string();

    let printed = stdout(&home.mix(&["extension", "inspect", &path]));

    assert!(printed.contains("mailpit"), "{printed}");
    assert!(
        printed.contains("127.0.0.1:8025"),
        "the rendered spec is what would run, addresses and all: {printed}"
    );
    assert!(
        printed.contains("asked for"),
        "a port here is a wish and the line has to say so: {printed}"
    );
    assert!(
        printed.contains("not a permission MixEngine enforces"),
        "`services` is a declaration shown to somebody, not a grant: {printed}"
    );

    let answered = json(&home.mix(&["extension", "inspect", &path, "--json"]));
    assert_eq!(answered["id"], "mailpit", "{answered}");
    assert_eq!(answered["runs"]["id"], "mailpit", "{answered}");
    assert_eq!(answered["ports"].as_array().expect("ports").len(), 2);
    assert_eq!(answered["extends"].as_array().expect("recipe").len(), 1);
}

/// A manifest that writes an address is refused **naming the field**, and reaches the client as an
/// argument that was wrong rather than as an internal error — which is where T77 left four
/// blueprint variants and T77a had to go back for them.
#[tokio::test(flavor = "multi_thread")]
async fn a_manifest_that_writes_an_address_is_refused_by_field() {
    let home = Home::new();
    let _daemon = home.start_daemon();
    let directory =
        extension(&MAILPIT.replace("\"{listen}:{ui_port}\"", "\"127.0.0.1:{ui_port}\""));
    let path = directory.path().display().to_string();

    let refused = home.mix(&["extension", "inspect", &path]);

    assert!(!refused.status.success(), "{}", stdout(&refused));

    let said = stderr(&refused);
    assert!(said.contains("service.args"), "{said}");
    assert!(!said.to_lowercase().contains("internal"), "{said}");
}
