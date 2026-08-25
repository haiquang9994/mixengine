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
