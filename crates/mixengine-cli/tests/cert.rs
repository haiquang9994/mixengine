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

/// `mix cert status` is **not** this command.
///
/// `.claude/features/tls.md` gives that name to the per-site diagnostics with a live TLS handshake,
/// which is **T53**. This asserts the name is still free rather than asserting a spelling: taking
/// the short name here would have meant renaming it later, or giving one command two unrelated
/// jobs, and neither is a thing a later task would notice on its own.
#[tokio::test(flavor = "multi_thread")]
async fn the_short_name_is_left_for_the_per_site_check() {
    let home = Home::new();
    let _daemon = home.start_daemon();

    let refused = home.mix(&["cert", "status"]);

    assert!(
        !refused.status.success(),
        "`mix cert status` succeeded, so T53 cannot have the name it is specified to use: {}",
        stdout(&refused)
    );
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
