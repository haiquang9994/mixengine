//! The credential store, against the real OS.
//!
//! **Not `#[ignore]`d, and the reason is the same one `access.rs` gives.** These touch the real
//! Credential Manager, Keychain or secret service, but only under a service name this test invents
//! for itself and deletes on the way out — the credential-store equivalent of a `TempDir`, not a
//! system file with other people's entries in it. Nothing here reads or removes anything MixEngine
//! or anybody else wrote.
//!
//! **A machine with no store is a passing run, not a failure.** A headless Linux — a CI container,
//! an SSH session with no desktop — has no secret service, and the answer this capability owes there
//! is [`Error::UnsupportedPlatform`], which these tests assert instead of skipping silently. That
//! way the two outcomes are "the store works" and "the OS said, in the typed way, that it has none",
//! and a *third* one — a store that quietly forgets — cannot pass.

use mixengine_platform::{Error, Host as _, host, mock};

/// A namespace nothing else on this machine is using, and nothing else in this run either.
///
/// The pid alone is not enough: two test binaries of this workspace can be in flight at once, and a
/// re-run reuses a pid within minutes.
fn namespace(what: &str) -> String {
    let unique = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .expect("a clock later than 1970")
        .as_nanos();

    format!("mixengine.test.{what}.{}.{unique}", std::process::id())
}

/// Whether this machine has a credential store at all, judged by what the capability answered.
///
/// Returns `false` only for [`Error::UnsupportedPlatform`]; every other failure is a store that is
/// there and misbehaving, which the caller re-raises rather than treating as an absent one.
fn store_is_absent<T>(outcome: &Result<T, Error>) -> bool {
    match outcome {
        Err(Error::UnsupportedPlatform { capability, .. }) => {
            assert_eq!(
                *capability, "Keyring",
                "another capability's unsupported answer arrived from the keyring"
            );
            true
        }
        Err(other) => panic!(
            "the credential store is present and refused: {}",
            chain(other)
        ),
        Ok(_) => false,
    }
}

/// Every sentence in an error's chain, joined.
///
/// `Display` on one error never includes its source, and this crate's errors take that seriously:
/// `Error::Secret` names the entry and stops, because the store's own complaint is the `#[source]`.
/// That is right for a log line built from the chain and wrong for a panic message, which is the
/// only thing a CI reader gets — a bare `{error}` says *what* failed and never *why*, which is the
/// difference between "cannot read the credential" and "No DBus session or Secret Service provider
/// found".
fn chain(error: &Error) -> String {
    let mut sentences = vec![error.to_string()];
    let mut cause = std::error::Error::source(error);

    while let Some(source) = cause {
        sentences.push(source.to_string());
        cause = source.source();
    }

    sentences.join(": ")
}

#[test]
fn a_stored_credential_comes_back_and_can_be_removed() {
    let host = host();
    let keyring = host.keyring();
    let service = namespace("roundtrip");

    let stored = keyring.set_secret(&service, "root", "correct horse battery staple");
    if store_is_absent(&stored) {
        return;
    }
    stored.expect("checked above");

    assert_eq!(
        keyring
            .secret(&service, "root")
            .expect("the store answered"),
        Some("correct horse battery staple".to_owned()),
        "the value that came back is not the value that went in"
    );

    keyring
        .forget_secret(&service, "root")
        .expect("removing a credential this test wrote");

    assert_eq!(
        keyring
            .secret(&service, "root")
            .expect("the store answered"),
        None,
        "the credential survived being forgotten"
    );
}

#[test]
fn a_credential_that_was_never_stored_is_absent_rather_than_an_error() {
    let host = host();
    let service = namespace("missing");

    let read = host.keyring().secret(&service, "root");
    if store_is_absent(&read) {
        return;
    }

    assert_eq!(
        read.expect("checked above"),
        None,
        "an entry nothing ever wrote has to read as absent, not as an empty password"
    );
}

#[test]
fn forgetting_a_credential_that_is_not_there_succeeds() {
    let host = host();
    let service = namespace("idempotent");

    let forgotten = host.keyring().forget_secret(&service, "root");
    if store_is_absent(&forgotten) {
        return;
    }

    forgotten.expect("removing nothing leaves nothing to remove");
}

/// Two services may use the same account name without reading each other's credential — which is the
/// whole reason the address is a pair.
#[test]
fn one_key_under_two_services_is_two_credentials() {
    let host = host();
    let keyring = host.keyring();
    let (mariadb, postgres) = (namespace("mariadb"), namespace("postgres"));

    let stored = keyring.set_secret(&mariadb, "root", "one");
    if store_is_absent(&stored) {
        return;
    }
    stored.expect("checked above");
    keyring
        .set_secret(&postgres, "root", "two")
        .expect("a second namespace");

    assert_eq!(
        keyring
            .secret(&mariadb, "root")
            .expect("the store answered"),
        Some("one".to_owned())
    );
    assert_eq!(
        keyring
            .secret(&postgres, "root")
            .expect("the store answered"),
        Some("two".to_owned())
    );

    keyring.forget_secret(&mariadb, "root").expect("tidy up");
    keyring.forget_secret(&postgres, "root").expect("tidy up");
}

/// What a caller sees on a machine with no store, without needing to be on one.
#[test]
fn a_host_without_a_keyring_says_so_in_the_typed_way() {
    let host = mock::Host::without_keyring("/tmp/mixengine-test", "no secret service on this bus");

    let error = host
        .keyring()
        .secret("mixengine.mariadb@main", "root")
        .expect_err("a host with no store cannot answer");

    assert!(
        matches!(
            &error,
            Error::UnsupportedPlatform { capability: "Keyring", reason } if reason.contains("bus")
        ),
        "{error:?}"
    );
}

/// The mock is a store, not a stub: a test that writes a credential and reads it back has to see it,
/// or every supervisor test built on it would be proving something about `None`.
#[test]
fn the_mock_remembers_what_it_was_given_and_says_what_it_did() {
    let host = mock::Host::with_home("/tmp/mixengine-test");
    let keyring = host.keyring();

    assert_eq!(keyring.secret("mixengine", "root").unwrap(), None);

    keyring.set_secret("mixengine", "root", "hunter2").unwrap();
    assert_eq!(
        keyring.secret("mixengine", "root").unwrap(),
        Some("hunter2".to_owned())
    );

    keyring.forget_secret("mixengine", "root").unwrap();
    assert_eq!(keyring.secret("mixengine", "root").unwrap(), None);

    assert_eq!(
        host.secret_operations(),
        [
            mock::SecretOp::Stored {
                service: "mixengine".to_owned(),
                key: "root".to_owned(),
            },
            mock::SecretOp::Forgotten {
                service: "mixengine".to_owned(),
                key: "root".to_owned(),
            },
        ],
        "the recorder is what supervisor tests assert on, and it does not record reads"
    );
}

/// A generated secret is the length asked for, and is made of characters no quoting rule can break.
///
/// **The alphabet is a safety property, not a style choice.** The value is interpolated into a SQL
/// string literal by the MariaDB recipe's bootstrap step (T33), so a quote or a backslash in it
/// would be an escaping bug with a credential on the other side of it. Restricting the alphabet is
/// what makes that interpolation safe without an escaper nobody would test.
#[test]
fn a_generated_secret_is_the_length_asked_for_and_needs_no_escaping() {
    let secret = mixengine_platform::generate_secret(32).expect("this machine has entropy");

    assert_eq!(secret.chars().count(), 32);
    assert!(
        secret.chars().all(|c| c.is_ascii_alphanumeric()),
        "{secret:?} contains something that would have to be escaped"
    );
}

/// Two of them differ. A weak assertion on purpose — this is not a statistical test, it is the one
/// that catches a stub returning a constant.
#[test]
fn two_generated_secrets_are_not_the_same() {
    let (first, second) = (
        mixengine_platform::generate_secret(32).expect("entropy"),
        mixengine_platform::generate_secret(32).expect("entropy"),
    );

    assert_ne!(first, second);
}
