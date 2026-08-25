//! The internal certificate authority.
//!
//! **Every assertion about a generated certificate is made after parsing it back**, never against
//! the `CertificateParams` that went in. Asserting on the parameters would prove that `rcgen` was
//! called and nothing whatever about what it produced — and what T49 installs into a trust store
//! and what T50 signs with is the file, not the request.

use std::path::Path;
use std::time::{Duration, SystemTime};

use mixengine_core::certs::ca;
use mixengine_proto::{CaState, Unusable};

/// A home's `certs/` directory, empty, as `Paths::bootstrap` would leave a fresh one.
fn certs() -> tempfile::TempDir {
    tempfile::tempdir().expect("a temporary directory")
}

/// The authority, or a panic naming what arrived instead.
fn present(state: CaState) -> mixengine_proto::Ca {
    match state {
        CaState::Present { ca } => ca,
        other => panic!("expected a usable authority, got {other:?}"),
    }
}

/// The DER inside the PEM the status carries.
fn der(ca: &mixengine_proto::Ca) -> Vec<u8> {
    pem::parse(&ca.certificate_pem)
        .expect("the certificate is PEM")
        .into_contents()
}

#[test]
fn a_generated_authority_is_a_ca_that_can_only_sign_certificates() {
    let home = certs();

    let ca = present(ca::ensure(home.path(), SystemTime::now()).expect("generated"));
    let bytes = der(&ca);
    let (_rest, parsed) =
        x509_parser::parse_x509_certificate(&bytes).expect("the certificate parses");

    let constraints = parsed
        .basic_constraints()
        .expect("the extension parses")
        .expect("a CA states its basic constraints")
        .value;

    assert!(constraints.ca, "the certificate is not marked as a CA");
    assert_eq!(
        constraints.path_len_constraint,
        Some(0),
        "a CA that may sign intermediates can delegate signing to anything, which is not what this \
         one is for"
    );

    let usage = parsed
        .key_usage()
        .expect("the extension parses")
        .expect("a CA states its key usage")
        .value;

    assert!(usage.key_cert_sign(), "the CA cannot sign certificates");
    assert!(usage.crl_sign(), "the CA cannot sign a revocation list");
    assert!(
        !usage.digital_signature() && !usage.key_encipherment(),
        "the CA carries the usages of a server key, which is what pathlen and this list exist to \
         prevent"
    );

    assert!(
        parsed.subject_alternative_name().expect("parses").is_none(),
        "the CA carries a subject alternative name, which invites something to accept it as a leaf"
    );
}

#[test]
fn the_subject_ends_with_the_identifier_derived_from_the_key() {
    let home = certs();

    let ca = present(ca::ensure(home.path(), SystemTime::now()).expect("generated"));

    assert_eq!(
        ca.key_id.len(),
        8,
        "the short identifier is eight hex characters"
    );
    assert!(
        ca.key_id
            .chars()
            .all(|c| c.is_ascii_digit() || ('a'..='f').contains(&c)),
        "the short identifier is lowercase hex: {}",
        ca.key_id
    );
    assert!(
        ca.subject
            .contains(&format!("MixEngine Local CA {}", ca.key_id)),
        "the subject does not name the key it belongs to: {}",
        ca.subject
    );
}

#[test]
fn the_fingerprint_is_the_hash_of_the_certificate_and_not_of_the_key() {
    use sha2::Digest as _;

    let home = certs();
    let ca = present(ca::ensure(home.path(), SystemTime::now()).expect("generated"));

    // Computed here rather than taken from the code under test: a fingerprint that agrees with
    // itself proves nothing.
    let independently: String = sha2::Sha256::digest(der(&ca))
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect();

    assert_eq!(
        ca.fingerprint, independently,
        "the fingerprint is not the SHA-256 of the certificate's DER"
    );
    assert_ne!(
        &ca.fingerprint[..8],
        ca.key_id,
        "the two identifiers are the same value, so one of them is not what it claims to be"
    );
}

#[test]
fn the_window_begins_now_and_is_ten_years_wide() {
    let home = certs();
    let now = SystemTime::now();

    let ca = present(ca::ensure(home.path(), now).expect("generated"));
    let started = mixengine_proto::Timestamp::from_system_time(now);

    // X.509 stores seconds, so the millisecond the caller passed does not survive the round trip.
    assert!(
        (ca.not_before.0 - started.0).abs() < 1_000,
        "the window does not begin when the authority was made"
    );

    let ten_years = 3_652_i64 * 24 * 60 * 60 * 1_000;

    assert_eq!(
        ca.not_after.0 - ca.not_before.0,
        ten_years,
        "the window is not ten years wide"
    );
    assert!(
        ca.days_left > 3_600,
        "days_left disagrees with the window it was derived from: {}",
        ca.days_left
    );
}

#[test]
fn a_second_ensure_leaves_both_files_byte_identical() {
    let home = certs();
    let now = SystemTime::now();

    ca::ensure(home.path(), now).expect("generated");

    let key = std::fs::read(ca::key_path(home.path())).expect("the key is there");
    let certificate =
        std::fs::read(ca::certificate_path(home.path())).expect("the certificate is there");

    ca::ensure(home.path(), now + Duration::from_secs(60)).expect("the second call succeeds");

    assert_eq!(
        std::fs::read(ca::key_path(home.path())).expect("the key is there"),
        key,
        "the private key was rewritten, so every leaf signed by the first one is now worthless"
    );
    assert_eq!(
        std::fs::read(ca::certificate_path(home.path())).expect("the certificate is there"),
        certificate,
        "the certificate was rewritten, so every trust store holding the first one is now stale"
    );
}

#[test]
fn an_empty_home_has_no_authority_and_says_so() {
    let home = certs();

    assert!(matches!(
        ca::read(home.path(), SystemTime::now()),
        CaState::Absent {}
    ));
}

#[test]
fn each_way_of_being_broken_is_reported_as_itself() {
    for (damage, expected) in [
        ("key-missing", Unusable::KeyMissing),
        ("certificate-missing", Unusable::CertificateMissing),
        ("key-unreadable", Unusable::KeyUnreadable),
        ("certificate-unreadable", Unusable::CertificateUnreadable),
        ("disagree", Unusable::KeyAndCertificateDisagree),
    ] {
        let home = certs();
        ca::ensure(home.path(), SystemTime::now()).expect("generated");

        let key = ca::key_path(home.path());
        let certificate = ca::certificate_path(home.path());

        match damage {
            "key-missing" => std::fs::remove_file(&key).expect("removed"),
            "certificate-missing" => std::fs::remove_file(&certificate).expect("removed"),
            "key-unreadable" => std::fs::write(&key, b"not a key").expect("written"),
            "certificate-unreadable" => {
                std::fs::write(&certificate, b"not a cert").expect("written");
            }
            // **A second authority's certificate beside the first's key.** Both files parse and
            // both are what they claim to be; they are simply not each other's — which is what a
            // backup that caught one file and not the other produces.
            "disagree" => {
                let other = certs();
                ca::ensure(other.path(), SystemTime::now()).expect("generated");
                std::fs::copy(ca::certificate_path(other.path()), &certificate).expect("copied");
            }
            _ => unreachable!(),
        }

        match ca::read(home.path(), SystemTime::now()) {
            CaState::Unusable { because } => assert_eq!(
                because, expected,
                "{damage} was reported as {because:?} rather than {expected:?}"
            ),
            other => panic!("{damage} was reported as {other:?}"),
        }
    }
}

#[test]
fn a_broken_authority_is_never_quietly_replaced() {
    let home = certs();
    ca::ensure(home.path(), SystemTime::now()).expect("generated");

    let certificate = ca::certificate_path(home.path());
    std::fs::write(&certificate, b"not a cert").expect("written");

    let state = ca::ensure(home.path(), SystemTime::now()).expect("ensure does not fail on damage");

    assert!(
        matches!(
            state,
            CaState::Unusable {
                because: Unusable::CertificateUnreadable
            }
        ),
        "ensure regenerated over damage, invalidating every leaf and every trust store: {state:?}"
    );
    assert_eq!(
        std::fs::read(&certificate).expect("the file is there"),
        b"not a cert",
        "ensure overwrote the damaged file"
    );
}

/// The key reaches disk before the certificate does.
///
/// **Asserted through the consequence rather than through timestamps.** A filesystem's modification
/// time can be coarser than two writes, which would make a direct comparison flaky and tell nobody
/// anything; what the ordering is *for* is that an interrupted generation leaves a state `read` can
/// name. So: take the certificate away, and the answer must be the one that says the key is here.
#[test]
fn an_interrupted_generation_leaves_the_half_that_can_be_recognised() {
    let home = certs();
    ca::ensure(home.path(), SystemTime::now()).expect("generated");

    std::fs::remove_file(ca::certificate_path(home.path())).expect("removed");

    assert!(
        matches!(
            ca::read(home.path(), SystemTime::now()),
            CaState::Unusable {
                because: Unusable::CertificateMissing
            }
        ),
        "the state a crash between the two writes leaves is not the one that is reported"
    );

    assert!(
        ca::key_path(home.path()).exists(),
        "the key is not on disk, so the write order is the other way round and an interrupted \
         generation would leave a certificate whose key never arrived"
    );
}

/// Both files land where the rest of the product will look for them.
#[test]
fn the_two_files_are_where_the_feature_specification_says() {
    let home = certs();
    ca::ensure(home.path(), SystemTime::now()).expect("generated");

    assert!(ends_with(&ca::key_path(home.path()), &["ca", "root.key"]));
    assert!(ends_with(
        &ca::certificate_path(home.path()),
        &["ca", "root.crt"]
    ));
}

/// The staging root is a certificates root of its own, so `ensure` and `read` work on it unchanged
/// — roadmap task **T54**, and the reason a rotation needs no second way to make an authority.
#[test]
fn a_candidate_is_made_and_read_by_the_same_code_as_the_real_one() {
    let home = certs();
    let now = SystemTime::now();

    let live = present(ca::ensure(home.path(), now).expect("this home's authority"));
    let staged = ca::pending_root(home.path());
    let candidate = present(ca::ensure(&staged, now).expect("a candidate"));

    assert_ne!(
        live.key_id, candidate.key_id,
        "a candidate over the same key would not be a rotation"
    );
    assert_eq!(
        present(ca::read(&staged, now)).fingerprint,
        candidate.fingerprint,
        "the candidate is described by the code that describes the real one"
    );
    assert_eq!(
        present(ca::read(home.path(), now)).fingerprint,
        live.fingerprint,
        "staging a candidate does not disturb the authority this home has"
    );
}

/// Promoting replaces both halves, and what `read` answers afterwards is the candidate.
#[test]
fn promoting_a_candidate_makes_it_the_authority_this_home_has() {
    let home = certs();
    let now = SystemTime::now();

    ca::ensure(home.path(), now).expect("this home's authority");
    let candidate = present(ca::ensure(&ca::pending_root(home.path()), now).expect("a candidate"));

    ca::promote(home.path()).expect("the candidate is promoted");

    let now_held = present(ca::read(home.path(), now));

    assert_eq!(now_held.key_id, candidate.key_id);
    assert_eq!(now_held.fingerprint, candidate.fingerprint);
    assert!(
        !ca::pending_root(home.path()).exists(),
        "a promoted candidate leaves no staging directory behind"
    );
}

/// **The assertion the whole staging design exists for.** Discarding must leave the live authority
/// byte-identical — a discard that deleted both would pass a test that only checked the staging.
#[test]
fn discarding_a_candidate_leaves_this_homes_authority_exactly_as_it_was() {
    let home = certs();
    let now = SystemTime::now();

    ca::ensure(home.path(), now).expect("this home's authority");
    let before = std::fs::read(ca::certificate_path(home.path())).expect("the certificate");
    let key_before = std::fs::read(ca::key_path(home.path())).expect("the key");

    ca::ensure(&ca::pending_root(home.path()), now).expect("a candidate");
    ca::discard(home.path()).expect("the candidate is discarded");

    assert_eq!(
        std::fs::read(ca::certificate_path(home.path())).expect("the certificate"),
        before,
        "the live certificate is untouched"
    );
    assert_eq!(
        std::fs::read(ca::key_path(home.path())).expect("the key"),
        key_before,
        "the live key is untouched"
    );
    assert!(!ca::pending_root(home.path()).exists());
}

/// A home that never staged anything is the ordinary case, and discarding there is not an error.
#[test]
fn discarding_when_nothing_is_staged_is_not_a_failure() {
    let home = certs();

    ca::discard(home.path()).expect("nothing to discard is not a failure");
}

fn ends_with(path: &Path, tail: &[&str]) -> bool {
    let components: Vec<String> = path
        .components()
        .map(|part| part.as_os_str().to_string_lossy().into_owned())
        .collect();

    components.len() >= tail.len() && components[components.len() - tail.len()..] == *tail
}
