//! `mix domain` against a real daemon.
//!
//! Roadmap task **T46**'s client half. What the daemon's own `tests/sites.rs` proves is that the
//! three methods do what they say; what is proved here is the part only `mix` can be wrong about —
//! that the flags a person types reach the right method, and that a diagnostic a person reads is a
//! rendering of what the daemon answered rather than a second opinion assembled in the client.
//!
//! **The negative half of this file needs a control.** A test concluding something from a name that
//! did *not* resolve, without proving in the same run and with the same instrument that a name which
//! must resolve does, is making a claim about `getaddrinfo` rather than about the machine. Four of
//! the six measurement rounds behind T45 were void for exactly that (T46 design, D9).

mod harness;

use std::net::ToSocketAddrs as _;

use harness::{Home, json, stdout};

/// A directory to register a project in.
fn repository() -> tempfile::TempDir {
    tempfile::Builder::new()
        .prefix("mixengine-domain")
        .tempdir()
        .expect("a temporary directory")
}

/// A name goes on, a name comes off, and the primary is neither.
#[tokio::test(flavor = "multi_thread")]
async fn a_name_is_added_and_taken_away_and_the_primary_stays() {
    let home = Home::new();
    let _daemon = home.start_daemon();
    let repository = repository();
    let root = repository.path().display().to_string();

    home.mix(&["project", "create", &root, "--name", "blog"]);
    home.mix(&[
        "site",
        "create",
        "--project",
        "blog",
        "--domain",
        "blog.test",
        "--kind",
        "static",
    ]);

    let added = json(&home.mix(&[
        "domain",
        "add",
        "www.blog.test",
        "--site",
        "blog.test",
        "--json",
    ]));

    assert_eq!(
        added["domains"],
        serde_json::json!(["blog.test", "www.blog.test"]),
        "{added}"
    );

    // The primary is refused, and the flag that would reorder is `mix site update`'s.
    let refused = home.mix(&["domain", "remove", "blog.test"]);
    assert!(!refused.status.success(), "{refused:?}");

    let removed = json(&home.mix(&["domain", "remove", "www.blog.test", "--json"]));
    assert_eq!(
        removed["domains"],
        serde_json::json!(["blog.test"]),
        "{removed}"
    );

    // And now it is the last one, which is the other refusal.
    let last = home.mix(&["domain", "remove", "blog.test"]);
    assert!(!last.status.success(), "{last:?}");
}

/// The diagnostic on a machine nothing has wired, which is what every machine running this is.
#[tokio::test(flavor = "multi_thread")]
async fn the_diagnostic_reports_a_name_nothing_routes_and_says_why() {
    // **CONTROL, before anything is concluded.**
    assert!(
        ("localhost", 80u16)
            .to_socket_addrs()
            .is_ok_and(|mut found| found.next().is_some()),
        "localhost does not resolve on this machine; nothing below would mean anything"
    );

    let home = Home::new();
    let _daemon = home.start_daemon();
    let repository = repository();
    let root = repository.path().display().to_string();

    home.mix(&["project", "create", &root, "--name", "blog"]);
    home.mix(&[
        "site",
        "create",
        "--project",
        "blog",
        "--domain",
        "blog.test",
        "--kind",
        "static",
    ]);

    let report = json(&home.mix(&["domain", "status", "blog.test", "--json"]));
    let row = &report["domains"][0];

    assert_eq!(row["domain"], "blog.test", "{report}");
    assert_eq!(row["site"], "blog.test", "{report}");
    assert_eq!(
        row["wildcard"], false,
        "no suite wires a resolver: {report}"
    );
    assert!(row["because"].is_string(), "{report}");

    // The same thing as a person sees it: the name, and the sentence under it.
    let table = stdout(&home.mix(&["domain", "status", "blog.test"]));

    assert!(table.contains("blog.test"), "{table}");
    assert!(table.contains("does not resolve"), "{table}");
}

/// A name nothing declares is answered rather than refused — the T46 design, D5.
#[tokio::test(flavor = "multi_thread")]
async fn a_name_nothing_declares_is_still_answered() {
    let home = Home::new();
    let _daemon = home.start_daemon();

    let report = json(&home.mix(&["domain", "status", "nobody.test", "--json"]));

    assert!(report["domains"][0]["site"].is_null(), "{report}");
    assert!(
        report["domains"][0]["because"]
            .as_str()
            .is_some_and(|because| because.contains("declares")),
        "{report}"
    );
}

/// A public TLD is refused by the same module `site.create` asks, so the answer is one answer.
#[tokio::test(flavor = "multi_thread")]
async fn a_public_tld_is_refused_here_too() {
    let home = Home::new();
    let _daemon = home.start_daemon();

    let refused = home.mix(&["domain", "status", "example.com"]);

    assert!(!refused.status.success(), "{refused:?}");
    assert!(
        String::from_utf8_lossy(&refused.stderr).contains("test"),
        "{refused:?}"
    );
}
