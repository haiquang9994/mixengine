//! Reading this machine's trust store, which every daemon start does.
//!
//! **What is proved here is that the read succeeds, not what it returns.** A clean runner holds no
//! MixEngine authority, so `installed: false` is the expected answer; the claim under test is that
//! an *ordinary account* can ask at all.
//!
//! **These are deliberately not `#[ignore]`d.** `.claude/standards/testing.md` rule 1 names the trust
//! store, and the gate it asks for is for tests that **touch** one — these only read. Being in CI's
//! ordinary `test` job, on all three runners and under no administrative token, is the entire point:
//! the T49a design's D13 records that neither of the two assumptions below could be measured on the
//! machine that design was written on, and names this file as where they are measured instead.
//!
//! If either fails, the producer in `mixengine-daemon` and `mix doctor`'s check both need a
//! different shape, because both are built on reading a store being cheap and unprivileged.

use mixengine_platform::TrustStoreMethod;

/// **The assumption on Windows and macOS that the design could not test.**
///
/// Windows: enumerating `LocalMachine\Root` without an administrative token. macOS:
/// `security find-certificate -a -p /Library/Keychains/System.keychain` as an ordinary user. Linux
/// was measurable where the design was written — the anchors directory is `drwxr-xr-x root root` and
/// the generated bundle is world-readable — and is asserted here anyway, because a machine in CI is
/// not the machine that was measured.
#[test]
fn this_machine_can_be_asked_what_it_trusts_without_an_administrative_token() {
    let host = mixengine_platform::host();

    let state = host
        .trust_store()
        .probe(b"not any certificate this machine holds")
        .expect("reading a trust store needs no privilege on any of the three systems");

    assert!(
        !state.installed,
        "a runner should not already hold an authority nothing has installed: {state:?}"
    );
}

/// Which mechanism this machine has, and on Linux whether it has one at all.
#[test]
fn this_machine_says_which_trust_store_it_has() {
    let host = mixengine_platform::host();

    let method = host.trust_store().method().expect("an answer");

    #[cfg(windows)]
    assert_eq!(method, TrustStoreMethod::SystemRoot);

    #[cfg(target_os = "macos")]
    assert_eq!(method, TrustStoreMethod::SystemKeychain);

    // D7: Linux is whichever family this machine is, **or neither** — all three are true answers,
    // and a runner with no anchors directory is a machine MixEngine supports over HTTP.
    #[cfg(target_os = "linux")]
    assert!(
        matches!(
            method,
            TrustStoreMethod::CaCertificates
                | TrustStoreMethod::CaTrustAnchors
                | TrustStoreMethod::None
        ),
        "{method:?}"
    );
}

/// A store that does not hold it says why, in a sentence somebody can act on.
///
/// `mix doctor`'s `CaNotTrusted` prints this, so an empty reason would be a check that reports a
/// problem and says nothing about it.
#[test]
fn a_machine_that_does_not_hold_it_says_so_in_words() {
    let host = mixengine_platform::host();

    let state = host
        .trust_store()
        .probe(b"not a certificate")
        .expect("a state");

    assert!(
        state.missing.is_some_and(|because| !because.is_empty()),
        "nothing said about why this machine does not trust it"
    );
}
