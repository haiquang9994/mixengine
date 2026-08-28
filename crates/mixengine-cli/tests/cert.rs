//! `mix cert …` against a real daemon.
//!
//! Roadmap task **T48**'s client half. What the daemon's own `tests/api.rs` proves is that the
//! authority exists and that the method answers; what is proved here is the part only `mix` can be
//! wrong about — that the answer reaches a screen in both renderings, that the private key does not
//! reach either, and that the short name this command did *not* take is still free.

mod harness;

use harness::{Home, json, stdout};

/// The authority a started daemon made, on the screen.
#[tokio::test(flavor = "multi_thread")]
async fn ca_status_names_the_authority_a_started_daemon_made() {
    let home = Home::new();
    let _daemon = home.start_daemon();

    let printed = stdout(&home.mix(&["cert", "ca-status"]));

    assert!(
        printed.contains("MixEngine Local CA"),
        "the authority is not named: {printed}"
    );
    assert!(
        printed.contains("expires"),
        "how long it has left is not said: {printed}"
    );
    assert!(
        !printed.contains("PRIVATE"),
        "the private key reached a terminal: {printed}"
    );
}

/// `--json` is the daemon's own value, and the field names are the guarantee.
#[tokio::test(flavor = "multi_thread")]
async fn ca_status_as_json_is_the_daemons_own_value() {
    let home = Home::new();
    let _daemon = home.start_daemon();

    let value = json(&home.mix(&["cert", "ca-status", "--json"]));

    assert_eq!(value["state"], "present", "{value}");
    assert_eq!(
        value["ca"]["fingerprint"].as_str().map(str::len),
        Some(64),
        "a SHA-256 in hex is 64 characters: {value}"
    );
    assert!(
        value["ca"]["certificate_pem"]
            .as_str()
            .is_some_and(|pem| pem.contains("-----BEGIN CERTIFICATE-----")),
        "{value}"
    );

    // The same assertion `cert_api`'s own test makes, made again where a person would actually be
    // piping this into something: the field names are the whole of the promise that a private key
    // has nowhere to travel.
    let encoded = value.to_string();
    for forbidden in ["PRIVATE", "key_pem", "private_key"] {
        assert!(
            !encoded.contains(forbidden),
            "the JSON carried {forbidden}: {encoded}"
        );
    }
}

/// **The name T48 held open, used** — roadmap task **T53**.
///
/// The test this replaces asserted that `mix cert status` *failed*, so that the short name could
/// not be taken by anything that was not the per-site check `.claude/features/tls.md` specifies.
/// It is that check now.
///
/// **A home with no front end is the case worth asserting**, because it is the state every fresh
/// home is in and the one a diagnostic still has to be useful in: there is no server to hand a
/// certificate over. There *is* a certificate — T50's producer signed one as the site was created —
/// and the answer keeps the two facts apart instead of reporting a certificate problem this site
/// does not have.
#[tokio::test(flavor = "multi_thread")]
async fn cert_status_reports_a_site_that_nothing_is_serving() {
    let home = Home::new();
    let _daemon = home.start_daemon();

    let repository = tempfile::Builder::new()
        .prefix("mixengine-t53")
        .tempdir()
        .expect("a temporary directory");
    let root = repository.path().display().to_string();

    home.mix(&["project", "create", &root, "--name", "blog"]);
    home.mix_in(
        repository.path(),
        &[],
        &[
            "site",
            "create",
            "--domain",
            "blog.test",
            "--kind",
            "static",
        ],
    );

    let answer = json(&home.mix(&["cert", "status", "--json"]));

    let site = answer["sites"]
        .as_array()
        .and_then(|sites| sites.iter().find(|site| site["domain"] == "blog.test"))
        .unwrap_or_else(|| panic!("blog.test is not in the report: {answer}"));

    assert_eq!(site["disk"]["state"], "present", "{answer}");
    assert_eq!(site["handshake"]["handshake"], "not_served", "{answer}");
    assert_eq!(site["problem"], "not_served", "{answer}");
}

/// And the private key still has nowhere to travel.
///
/// The assertion `ca-status` makes about itself, made again on the only other command that reads
/// certificates — and this one reads them off a socket as well as off a disk.
#[tokio::test(flavor = "multi_thread")]
async fn cert_status_carries_no_private_key() {
    let home = Home::new();
    let _daemon = home.start_daemon();

    let encoded = json(&home.mix(&["cert", "status", "--json"])).to_string();

    for forbidden in ["PRIVATE", "key_pem", "private_key"] {
        assert!(!encoded.contains(forbidden), "{forbidden} is in {encoded}");
    }
}

/// Whether this machine trusts it, on the screen — roadmap task **T49a**.
///
/// **T48's own test asserted this line was absent**, because there was no such fact in the answer
/// then. There is now, and what is checked is that the client prints the daemon's word rather than
/// deciding: on a runner nothing has installed into, the honest answer is "no" or "n/a", and "yes"
/// would mean the client made it up.
#[tokio::test(flavor = "multi_thread")]
async fn ca_status_says_whether_this_machine_trusts_the_authority() {
    let home = Home::new();
    let _daemon = home.start_daemon();

    let printed = stdout(&home.mix(&["cert", "ca-status"]));

    assert!(printed.contains("trusted"), "{printed}");
    assert!(
        !printed.contains("trusted    yes"),
        "nothing installed this authority, so the screen cannot say it is trusted: {printed}"
    );

    // And the private key still reaches neither rendering, which is the assertion T48 made and this
    // change had every opportunity to break: the trust half reads a store full of certificates.
    assert!(!printed.contains("PRIVATE"), "{printed}");
}

/// The same fact over `--json`, tagged rather than spelled out.
#[tokio::test(flavor = "multi_thread")]
async fn the_trust_answer_is_a_word_a_client_can_match_on() {
    let home = Home::new();
    let _daemon = home.start_daemon();

    let value = json(&home.mix(&["cert", "ca-status", "--json"]));

    // Nested rather than flattened, and deliberately: `Trust::NotInstalled` and `CaState::Unusable`
    // both call their reason `because`, so one would have overwritten the other.
    let state = value["trust"]["state"].as_str().unwrap_or_default();

    assert!(
        matches!(state, "not_installed" | "no_store" | "unknown"),
        "a runner cannot already trust this authority: {value}"
    );
    assert!(
        value["trust"]["because"]
            .as_str()
            .is_some_and(|s| !s.is_empty()),
        "nothing said about why: {value}"
    );

    let encoded = value.to_string();
    for forbidden in ["PRIVATE", "key_pem", "private_key"] {
        assert!(!encoded.contains(forbidden), "{encoded}");
    }
}

/// **The browsers are a second line and a second question** — roadmap task T49b. Firefox and Chrome
/// on Linux read certificate databases of their own, so a machine whose system store holds this
/// authority can still show a red padlock in both; the screen says so rather than letting the
/// trust line stand for both.
///
/// What a runner answers depends on the runner — Windows and macOS are not searched, a Linux leg
/// with no browser profile finds none, and a Linux leg with no `libnss3-tools` has no tool — so
/// what is asserted is that a line is printed and that it never claims a trust nothing installed.
#[tokio::test(flavor = "multi_thread")]
async fn ca_status_says_what_this_machines_browsers_hold() {
    let home = Home::new();
    let _daemon = home.start_daemon();

    let printed = stdout(&home.mix(&["cert", "ca-status"]));

    assert!(printed.contains("browsers"), "{printed}");
    assert!(
        !printed.contains("browsers   yes"),
        "nothing installed this authority into a browser, so the screen cannot say one holds it: \
         {printed}"
    );
    assert!(!printed.contains("PRIVATE"), "{printed}");
}

/// The same fact over `--json`, tagged rather than spelled out, and beside `trust` rather than
/// inside it.
#[tokio::test(flavor = "multi_thread")]
async fn the_browsers_answer_is_a_word_a_client_can_match_on() {
    let home = Home::new();
    let _daemon = home.start_daemon();

    let value = json(&home.mix(&["cert", "ca-status", "--json"]));

    let state = value["browsers"]["state"].as_str().unwrap_or_default();

    assert!(
        matches!(state, "reached" | "no_tool" | "not_searched" | "unknown"),
        "not one of the four states: {value}"
    );

    // Every state that names no database says why; `reached` names them instead.
    if state == "reached" {
        for database in value["browsers"]["databases"]
            .as_array()
            .unwrap_or(&Vec::new())
        {
            assert_eq!(
                database["installed"], false,
                "a runner cannot already have this authority in a browser: {value}"
            );
            assert!(
                database["path"].as_str().is_some_and(|s| !s.is_empty()),
                "a database with no path: {value}"
            );
        }
    } else {
        assert!(
            value["browsers"]["because"]
                .as_str()
                .is_some_and(|s| !s.is_empty()),
            "nothing said about why: {value}"
        );
    }
}

/// A site gets a certificate, and asking again writes nothing — roadmap task **T50**.
///
/// **One test through the whole stack**, because what it proves is that the RPC, the daemon's walk
/// over the rows and the renderer agree; each of those has its own unit tests and none of them can
/// show that.
#[tokio::test(flavor = "multi_thread")]
async fn a_site_is_issued_a_certificate_and_the_second_ask_reuses_it() {
    let home = Home::new();
    let _daemon = home.start_daemon();

    let repository = tempfile::Builder::new()
        .prefix("mixengine-cert")
        .tempdir()
        .expect("a temporary directory");
    let root = repository.path().display().to_string();

    home.mix(&["project", "create", &root, "--name", "blog"]);
    home.mix_in(
        repository.path(),
        &[],
        &[
            "site",
            "create",
            "--domain",
            "blog.test",
            "--kind",
            "static",
        ],
    );

    let first = json(&home.mix(&["cert", "issue", "--json"]));

    let issued = first["sites"]
        .as_array()
        .and_then(|sites| sites.iter().find(|site| site["domain"] == "blog.test"))
        .unwrap_or_else(|| panic!("blog.test is not in the report: {first}"));

    // **Not `issued`.** The daemon's own producer runs at start and after every site create, so by
    // the time a person types this the certificate is already there — which is the guarantee T50
    // exists for, and reading `reused` here is what proves it rather than a defect.
    assert!(
        matches!(
            issued["outcome"]["outcome"].as_str(),
            Some("issued" | "reused")
        ),
        "{first}"
    );

    assert!(
        home.path().join("certs/sites/blog.test.crt").is_file(),
        "no certificate on disk: {first}"
    );
    assert!(
        home.path().join("certs/sites/blog.test.key").is_file(),
        "no private key on disk: {first}"
    );

    let second = json(&home.mix(&["cert", "issue", "--json"]));
    let again = second["sites"]
        .as_array()
        .and_then(|sites| sites.iter().find(|site| site["domain"] == "blog.test"))
        .unwrap_or_else(|| panic!("blog.test is not in the second report: {second}"));

    assert_eq!(again["outcome"]["outcome"], "reused", "{second}");

    // And the site is named on a screen as well as in a pipe.
    let printed = stdout(&home.mix(&["cert", "issue"]));
    assert!(printed.contains("blog.test"), "{printed}");

    // Nothing anywhere in either answer carries a private key.
    for answer in [first, second] {
        let encoded = answer.to_string();
        for forbidden in ["PRIVATE", "key_pem", "private_key"] {
            assert!(!encoded.contains(forbidden), "{encoded}");
        }
    }
}

/// A home with no site says so in a sentence rather than printing nothing.
#[tokio::test(flavor = "multi_thread")]
async fn a_home_with_no_site_is_told_so() {
    let home = Home::new();
    let _daemon = home.start_daemon();

    let printed = stdout(&home.mix(&["cert", "issue"]));

    assert!(printed.contains("no site"), "{printed}");
}

/// **What a fresh home's removal actually finds**, which is a machine holding none of it.
///
/// The authority this daemon just made is seconds old and in no store on this machine, so the
/// honest answer is that nothing is left — reached without a prompt, because
/// `Certificates::require_untrust` enqueues nothing for a store that is not holding it. T41's D11,
/// one capability along: no prompt is spent on a row whose only outcome is `AlreadyDone`.
#[tokio::test(flavor = "multi_thread")]
async fn ca_uninstall_finds_nothing_left_in_a_store_that_never_held_it() {
    let home = Home::new();
    let _daemon = home.start_daemon();

    let report = json(&home.mix(&["cert", "ca-uninstall", "--yes", "--json"]));

    assert_eq!(
        report["outcome"], "removed",
        "a store that never held it holds none of it now: {report}"
    );
    assert_eq!(
        report["status"]["state"], "present",
        "the certificate is still on disk — this command takes trust and never a file: {report}"
    );

    assert!(
        home.path().join("certs/ca/root.crt").is_file(),
        "the certificate was deleted: {report}"
    );
    assert!(
        home.path().join("certs/ca/root.key").is_file(),
        "the private key was deleted: {report}"
    );

    let encoded = report.to_string();
    for forbidden in ["PRIVATE", "key_pem", "private_key"] {
        assert!(!encoded.contains(forbidden), "{encoded}");
    }
}

/// A home with no authority is told there is nothing to take out, rather than being failed.
#[tokio::test(flavor = "multi_thread")]
async fn ca_uninstall_on_a_home_with_no_authority_says_so() {
    let home = Home::new();
    let _daemon = home.start_daemon();

    std::fs::remove_dir_all(home.path().join("certs/ca"))
        .expect("this home's authority is removed");

    let report = json(&home.mix(&["cert", "ca-uninstall", "--yes", "--json"]));

    assert_eq!(report["outcome"], "nothing_to_remove", "{report}");
    assert_eq!(report["status"]["state"], "absent", "{report}");
}

/// **The question comes before the change**, and `--json` has nobody to put it to.
///
/// The rule `mix elevation grant` obeys: a pipe, a cron job and a CI step are all end of file, and a
/// command that assumed "yes" there would take an authority out of a machine nobody was sitting at.
#[tokio::test(flavor = "multi_thread")]
async fn ca_uninstall_refuses_when_there_is_nobody_to_answer() {
    let home = Home::new();
    let _daemon = home.start_daemon();

    let output = home.mix(&["cert", "ca-uninstall"]);

    assert!(!output.status.success(), "{}", stdout(&output));
    assert!(
        harness::stderr(&output).contains("--yes"),
        "the flag that answers in advance is named: {}",
        harness::stderr(&output)
    );
    assert!(
        home.path().join("certs/ca/root.crt").is_file(),
        "an unanswered question changed something"
    );
}

/// **A rotation end to end, and it is a system test because the first draft was not** — T54.
///
/// This was written as an ordinary `#[test]` on the assumption that no machine running `cargo test`
/// could raise an elevation prompt, so the rotation would always refuse and the assertion could be
/// "nothing changed". **Measured on 2026-08-26, that assumption is false**: a real UAC dialog
/// appeared in the middle of `cargo test` on Windows, a person clicked Yes, and the run installed a
/// certificate authority into `LocalMachine\Root`. That is exactly what rule 1 of
/// `.claude/standards/testing.md` forbids, and no amount of arranging the *home* prevents it — the
/// store a rotation reaches is the machine's.
///
/// So it is gated, and what it asserts is the **invariant** rather than either outcome: a rotation
/// either replaces the authority or leaves it exactly as it was, and in neither case does it leave a
/// candidate private key on disk. Asserting one outcome would make the test a statement about
/// whoever answered the prompt.
///
/// **CI's `system` job sets the variable and runs this on all three systems**, which is what makes
/// the invariant shape earn its keep rather than merely being careful: Windows and macOS hold a
/// token that can grant, so they take the *replaced* arm, while a Linux runner has no polkit agent
/// and takes the *left alone* one. One test, two machines' worth of answer, and neither leg is a
/// statement about the other.
#[tokio::test(flavor = "multi_thread")]
#[ignore = "writes this machine's trust store — set MIXENGINE_SYSTEM_TESTS=1"]
async fn a_rotation_either_replaces_the_authority_or_leaves_it_alone() {
    if std::env::var("MIXENGINE_SYSTEM_TESTS").as_deref() != Ok("1") {
        return;
    }

    let home = Home::new();
    let _daemon = home.start_daemon();

    let certificate = home.path().join("certs/ca/root.crt");
    let before = std::fs::read(&certificate).expect("this home's authority");
    let was = json(&home.mix(&["cert", "ca-status", "--json"]));

    let report = json(&home.mix(&["cert", "ca-rotate", "--yes", "--json"]));
    let now = std::fs::read(&certificate).expect("this home's authority");

    match report["outcome"].as_str() {
        Some("rotated") => {
            assert_ne!(
                now, before,
                "a rotation that reported success left the old certificate: {report}"
            );
            assert_ne!(
                was["ca"]["key_id"], report["status"]["ca"]["key_id"],
                "a rotation over the same key would not be one: {report}"
            );
        }
        Some("not_committed") => assert_eq!(
            now, before,
            "a rotation that committed nothing still replaced the certificate: {report}"
        ),
        other => panic!("neither outcome a rotation can have: {other:?} in {report}"),
    }

    assert!(
        !home.path().join("certs/pending").exists(),
        "a candidate private key was left lying about: {report}"
    );
}

/// A home with no authority is told there is nothing to rotate, rather than being given one.
///
/// Making one is what a daemon start already does; a rotation that also created would be a second
/// producer of the same thing, and a destructive command is the wrong place to put it.
#[tokio::test(flavor = "multi_thread")]
async fn ca_rotate_on_a_home_with_no_authority_refuses_rather_than_making_one() {
    let home = Home::new();
    let _daemon = home.start_daemon();

    std::fs::remove_dir_all(home.path().join("certs/ca"))
        .expect("this home's authority is removed");

    let report = json(&home.mix(&["cert", "ca-rotate", "--yes", "--json"]));

    assert_eq!(report["outcome"], "nothing_to_rotate", "{report}");
    assert!(
        !home.path().join("certs/ca/root.crt").exists(),
        "a refusal made one anyway: {report}"
    );
    assert!(
        !home.path().join("certs/pending").exists(),
        "a refusal staged one anyway: {report}"
    );
}

/// **The question comes before the change**, and a rotation is the most destructive question `mix`
/// asks. A pipe, a cron job and a CI step are all end of file, and assuming "yes" there would
/// replace an authority on a machine nobody was sitting at.
#[tokio::test(flavor = "multi_thread")]
async fn ca_rotate_refuses_when_there_is_nobody_to_answer() {
    let home = Home::new();
    let _daemon = home.start_daemon();

    let before = std::fs::read(home.path().join("certs/ca/root.crt")).expect("the authority");

    let output = home.mix(&["cert", "ca-rotate"]);

    assert!(!output.status.success(), "{}", stdout(&output));
    assert!(
        harness::stderr(&output).contains("--yes"),
        "the flag that answers in advance is named: {}",
        harness::stderr(&output)
    );
    assert_eq!(
        std::fs::read(home.path().join("certs/ca/root.crt")).expect("the authority"),
        before,
        "an unanswered question replaced the authority"
    );
}
