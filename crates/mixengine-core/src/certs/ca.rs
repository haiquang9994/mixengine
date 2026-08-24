//! The internal certificate authority: making one, reading one, and refusing to replace one.
//!
//! **A certificate cannot carry a hash of itself**, which is what
//! `.claude/architecture/security-model.md` asks for when it names the subject
//! `MixEngine Local CA <short-fingerprint>`: the subject is inside the bytes the fingerprint is
//! over, so no ordering produces it. The eight characters in the name come from the **public key**
//! instead — computable the moment the key pair exists, and stable across re-signing the same key,
//! which is what makes two certificates for one authority recognisable as one authority. What
//! `cert.ca_status` reports as the *fingerprint* is still the hash of the certificate, because that
//! is the number a browser shows and therefore the only one a person can compare against anything.
//!
//! **Damage is reported here and never repaired.** Regenerating on finding a broken authority would
//! invalidate every leaf already issued and every trust store the old certificate reached, in answer
//! to a request nobody made. Rotation is roadmap task T54, and it exists because it has the steps
//! this would skip: reissue the leaves, then remove the old certificate from the stores.

use std::path::{Path, PathBuf};
use std::time::{Duration, SystemTime};

use mixengine_proto::{Ca, CaState, Timestamp, Unusable};
use rcgen::{
    BasicConstraints,
    CertificateParams,
    DistinguishedName,
    DnType,
    IsCa,
    KeyPair,
    KeyUsagePurpose,
    PKCS_ECDSA_P256_SHA256,
    // `subject_public_key_info` is a trait method: the DER of the public key is what both the
    // short identifier and the agreement check are computed over.
    PublicKeyData as _,
};
use sha2::Digest as _;

use crate::{Error, Result};

/// Ten years, as `security-model.md` asks for.
///
/// 3652 days: ten 365-day years plus the two leap days any ten-year window from now contains. Not
/// a calendar calculation, because a certificate's validity is a duration rather than an
/// anniversary, and one that fell on 29 February would have to decide what it meant.
const LIFETIME: Duration = Duration::from_secs(3_652 * 24 * 60 * 60);

/// How many hex characters of the key's hash go into the subject.
///
/// Eight is 32 bits: enough that two authorities on one machine do not collide in a name a person
/// reads off a screen, and short enough to be read off one. Collision resistance is the
/// fingerprint's job and not this one's.
const KEY_ID_LENGTH: usize = 8;

/// Milliseconds in a day, which is what [`Timestamp`] counts in.
const DAY: i64 = 24 * 60 * 60 * 1_000;

/// Where the private key lives.
#[must_use]
pub fn key_path(certs: &Path) -> PathBuf {
    certs.join("ca").join("root.key")
}

/// Where the certificate lives.
#[must_use]
pub fn certificate_path(certs: &Path) -> PathBuf {
    certs.join("ca").join("root.crt")
}

/// Make this home's authority if it has none, and report what is there either way.
///
/// **Creates only when there is nothing at all.** Anything else — a missing half, an unreadable
/// file, two files that are not each other's — is reported through the returned state and left
/// exactly as it was found. See the module documentation for why that is not timidity.
///
/// # Errors
///
/// [`Error::Io`] when `certs/ca/` cannot be created or written, and [`Error::Certificate`] when
/// this machine will not produce a key pair at all.
pub fn ensure(certs: &Path, now: SystemTime) -> Result<CaState> {
    match read(certs, now) {
        CaState::Absent {} => {}
        settled => return Ok(settled),
    }

    crate::paths::create_dir(&certs.join("ca"))?;

    let key =
        KeyPair::generate_for(&PKCS_ECDSA_P256_SHA256).map_err(|source| Error::Certificate {
            action: "generate a key pair for",
            subject: "this home's certificate authority".to_owned(),
            source: Box::new(source),
        })?;

    let certificate = self_signed(&key, now)?;

    // **The key first.** A crash between the two writes then leaves a key with no certificate,
    // which `read` recognises by name; the other order leaves a certificate with no key, which
    // looks exactly like a certificate whose key was lost — and is a thing T50 could try to issue
    // against.
    mixengine_platform::write_private(&key_path(certs), key.serialize_pem().as_bytes())?;

    let path = certificate_path(certs);
    std::fs::write(&path, certificate.pem()).map_err(|source| Error::Io {
        action: "write",
        path,
        source,
    })?;

    // Read back rather than describing what was just written: what a caller is told is what is on
    // disk, which is the same promise `read` makes to everybody else.
    Ok(read(certs, now))
}

/// The DER inside a certificate's PEM envelope, or [`None`] when it is not one.
///
/// **Here rather than in the daemon**, so that whatever asks a machine to trust this authority hands
/// over the same bytes this module wrote — roadmap task **T49a** passes them to `mixengine-elevate`,
/// which compares what a store already holds against exactly these. A second decoder somewhere else
/// would be a second answer to "which bytes are the certificate".
#[must_use]
pub fn der(certificate_pem: &str) -> Option<Vec<u8>> {
    pem::parse(certificate_pem)
        .ok()
        .map(pem::Pem::into_contents)
}

/// What is on disk, without changing any of it.
#[must_use]
pub fn read(certs: &Path, now: SystemTime) -> CaState {
    let key = std::fs::read_to_string(key_path(certs)).ok();
    let certificate = std::fs::read_to_string(certificate_path(certs)).ok();

    let (key, certificate) = match (key, certificate) {
        (Some(key), Some(certificate)) => (key, certificate),
        (None, Some(_)) => return unusable(Unusable::KeyMissing),
        (Some(_), None) => return unusable(Unusable::CertificateMissing),
        (None, None) => return CaState::Absent {},
    };

    let Ok(key) = KeyPair::from_pem(&key) else {
        return unusable(Unusable::KeyUnreadable);
    };

    let Ok(der) = pem::parse(&certificate).map(pem::Pem::into_contents) else {
        return unusable(Unusable::CertificateUnreadable);
    };

    let Ok((_rest, parsed)) = x509_parser::parse_x509_certificate(&der) else {
        return unusable(Unusable::CertificateUnreadable);
    };

    // **The one check that is not about a file being readable.** Both halves parse and both are
    // what they claim to be; the question is whether they are each other's. A home restored from a
    // backup that caught one file and not the other is how they come apart, and left unchecked the
    // symptom surfaces much later as leaf certificates nothing trusts.
    if parsed.public_key().raw != key.subject_public_key_info().as_slice() {
        return unusable(Unusable::KeyAndCertificateDisagree);
    }

    let not_before = Timestamp(parsed.validity().not_before.timestamp() * 1_000);
    let not_after = Timestamp(parsed.validity().not_after.timestamp() * 1_000);

    CaState::Present {
        ca: Ca {
            subject: parsed.subject().to_string(),
            fingerprint: hex(&sha2::Sha256::digest(&der)),
            key_id: key_id(&key),
            not_before,
            not_after,
            days_left: (not_after.0 - Timestamp::from_system_time(now).0).div_euclid(DAY),
            certificate_pem: certificate,
        },
    }
}

/// The self-signed certificate for `key`.
fn self_signed(key: &KeyPair, now: SystemTime) -> Result<rcgen::Certificate> {
    let mut name = DistinguishedName::new();
    name.push(
        DnType::CommonName,
        format!("MixEngine Local CA {}", key_id(key)),
    );

    let mut params = CertificateParams::default();
    params.distinguished_name = name;
    // `Constrained(0)` is `pathlen:0`: this authority may sign leaves and may not sign another
    // authority, so a leaked leaf key cannot become a CA of its own.
    params.is_ca = IsCa::Ca(BasicConstraints::Constrained(0));
    // Exactly these two. An authority that also carried `digitalSignature` could be used as a
    // server key, which is the whole thing `pathlen` and this list exist to prevent.
    params.key_usages = vec![KeyUsagePurpose::KeyCertSign, KeyUsagePurpose::CrlSign];
    // No subject alternative name at all: an authority is not a server, and a name on one is an
    // invitation for something to accept it as a leaf. `CertificateParams::default()` carries none,
    // and this says so out loud because `CertificateParams::new` — the constructor most examples
    // use — takes a list of them.
    params.subject_alt_names = Vec::new();
    // `OffsetDateTime` is never named here, so this crate needs no dependency on `time`: the
    // field's own type does the conversion, and the `Duration` added to it is `std`'s.
    params.not_before = now.into();
    params.not_after = params.not_before + LIFETIME;

    params
        .self_signed(key)
        .map_err(|source| Error::Certificate {
            action: "sign",
            subject: "this home's certificate authority".to_owned(),
            source: Box::new(source),
        })
}

/// The short identifier the subject carries: the first [`KEY_ID_LENGTH`] hex characters of the
/// SHA-256 of the SubjectPublicKeyInfo.
fn key_id(key: &KeyPair) -> String {
    let mut id = hex(&sha2::Sha256::digest(key.subject_public_key_info()));
    id.truncate(KEY_ID_LENGTH);
    id
}

/// Lowercase hex, no separators — the way a fingerprint is compared.
fn hex(bytes: &[u8]) -> String {
    use std::fmt::Write as _;

    let mut rendered = String::with_capacity(bytes.len() * 2);
    for byte in bytes {
        // Cannot fail: writing to a `String` is infallible and the format carries no user input.
        let _ = write!(rendered, "{byte:02x}");
    }
    rendered
}

fn unusable(because: Unusable) -> CaState {
    CaState::Unusable { because }
}
