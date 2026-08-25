//! The leaves this home's authority signs — roadmap task **T50**.
//!
//! One certificate per site, ninety days, `serverAuth` and nothing else, covering exactly the names
//! that site answers to. `.claude/features/tls.md` gives the reason ninety days is short for a
//! certificate nothing public will ever see: browsers already refuse public certificates over 398
//! days, the direction of travel is downwards, and a private authority that had drifted to ten-year
//! leaves would meet the next tightening as a support load rather than as a renewal.
//!
//! **Shaped like [`super::ca`] on purpose.** Same four exports, same order of writes, same refusal
//! to repair what it finds damaged.

use std::path::{Path, PathBuf};
use std::time::SystemTime;

use mixengine_proto::{CertState, SiteCert, Timestamp, Unusable};
use rcgen::{
    KeyPair,
    // `subject_public_key_info` is a trait method: the DER of the public key is what the agreement
    // check between the two halves is computed over.
    PublicKeyData as _,
};
use sha2::Digest as _;

/// Below this many days left, a certificate is reissued rather than reused.
///
/// A third of the lifetime, which is what makes T52's daily check able to miss two months of
/// laptop-asleep and still renew in time.
pub const RENEW_WITHIN_DAYS: i64 = 30;

/// Milliseconds in a day, which is what [`Timestamp`] counts in.
const DAY: i64 = 24 * 60 * 60 * 1_000;

/// Where the leaves live, under the certificates directory.
const SITES: &str = "sites";

/// Where this site's private key lives.
///
/// **Named after the primary domain**, which `mixengine_core::domains::normalised` has already
/// restricted to lowercase `[a-z0-9.-]` with a managed TLD — so there is no path separator, no `*`
/// and no `:` for this join to have to defend against. Windows' reserved device names were measured
/// rather than assumed: `nul.test.crt` is an ordinary file, because the rule applies to the stem
/// before the final extension and not to the first label.
#[must_use]
pub fn key_path(certs: &Path, domain: &str) -> PathBuf {
    certs.join(SITES).join(format!("{domain}.key"))
}

/// Where this site's certificate lives.
#[must_use]
pub fn certificate_path(certs: &Path, domain: &str) -> PathBuf {
    certs.join(SITES).join(format!("{domain}.crt"))
}

/// What is on disk for this site, without changing any of it.
#[must_use]
pub fn read(certs: &Path, domain: &str, now: SystemTime) -> CertState {
    let key = std::fs::read_to_string(key_path(certs, domain)).ok();
    let certificate = std::fs::read_to_string(certificate_path(certs, domain)).ok();

    let (key, certificate) = match (key, certificate) {
        (Some(key), Some(certificate)) => (key, certificate),
        (None, Some(_)) => return unusable(Unusable::KeyMissing),
        (Some(_), None) => return unusable(Unusable::CertificateMissing),
        (None, None) => return CertState::Absent {},
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

    // The same check `ca::read` makes, for the same reason: both halves parsing says nothing about
    // whether they are each other's, and a home restored from a backup that caught one file is how
    // they come apart.
    if parsed.public_key().raw != key.subject_public_key_info().as_slice() {
        return unusable(Unusable::KeyAndCertificateDisagree);
    }

    let not_before = Timestamp(parsed.validity().not_before.timestamp() * 1_000);
    let not_after = Timestamp(parsed.validity().not_after.timestamp() * 1_000);

    CertState::Present {
        cert: SiteCert {
            subject: parsed.subject().to_string(),
            issuer: parsed.issuer().to_string(),
            sans: names(&parsed),
            fingerprint: hex(&sha2::Sha256::digest(&der)),
            not_before,
            not_after,
            days_left: (not_after.0 - Timestamp::from_system_time(now).0).div_euclid(DAY),
        },
    }
}

/// Every DNS name in the subject alternative name extension, in the order it carries them.
///
/// **DNS names only.** Nothing here issues an IP or an email SAN — the T50 design, D4 — so anything
/// else in that extension came from somewhere this module did not write, and reporting it as a name
/// the certificate covers would be reporting something a browser will not match a hostname against.
fn names(certificate: &x509_parser::certificate::X509Certificate<'_>) -> Vec<String> {
    let Ok(Some(extension)) = certificate.subject_alternative_name() else {
        return Vec::new();
    };

    extension
        .value
        .general_names
        .iter()
        .filter_map(|name| match name {
            x509_parser::extensions::GeneralName::DNSName(dns) => Some((*dns).to_owned()),
            _ => None,
        })
        .collect()
}

/// One of the ways a pair on disk is not usable.
fn unusable(because: Unusable) -> CertState {
    CertState::Unusable { because }
}

/// Lowercase hex, no separators — the same spelling [`super::ca`] reports a fingerprint in.
///
/// A second private copy rather than borrowing that module's: making one `pub(crate)` for one
/// caller would put a formatting helper in this crate's own surface, and the whole of it is three
/// lines.
fn hex(bytes: &[u8]) -> String {
    use std::fmt::Write as _;

    let mut rendered = String::with_capacity(bytes.len() * 2);
    for byte in bytes {
        // Cannot fail: writing to a `String` is infallible and the format carries no user input.
        let _ = write!(rendered, "{byte:02x}");
    }
    rendered
}

#[cfg(test)]
mod tests {
    use std::time::Duration;

    use mixengine_proto::CaState;
    use rcgen::{CertificateParams, PKCS_ECDSA_P256_SHA256};

    use super::super::ca;
    use super::*;

    /// A home with an authority and nothing else.
    fn a_home() -> tempfile::TempDir {
        let home = tempfile::tempdir().expect("a temp home");
        ca::ensure(home.path(), SystemTime::now()).expect("an authority is made");
        home
    }

    /// A site with no certificate is `Absent`, which is what every fresh home answers.
    #[test]
    fn a_site_with_no_certificate_is_absent() {
        let home = a_home();

        assert_eq!(
            read(home.path(), "blog.test", SystemTime::now()),
            CertState::Absent {}
        );
    }

    /// Half a pair is named rather than reported as absent — the state a crash between the two
    /// writes leaves, and the reason the key is written first.
    #[test]
    fn half_a_pair_is_named() {
        let home = a_home();
        std::fs::create_dir_all(home.path().join("sites")).expect("the directory is made");

        std::fs::write(key_path(home.path(), "blog.test"), "not a key").expect("written");
        assert_eq!(
            read(home.path(), "blog.test", SystemTime::now()),
            CertState::Unusable {
                because: Unusable::CertificateMissing
            }
        );

        std::fs::remove_file(key_path(home.path(), "blog.test")).expect("removed");
        std::fs::write(certificate_path(home.path(), "blog.test"), "not a cert").expect("written");
        assert_eq!(
            read(home.path(), "blog.test", SystemTime::now()),
            CertState::Unusable {
                because: Unusable::KeyMissing
            }
        );
    }

    /// Two halves that are not each other's is the state a backup catching one file leaves, and it
    /// is a different answer from either being missing.
    #[test]
    fn a_key_and_a_certificate_that_are_not_each_others_are_named() {
        let home = a_home();
        std::fs::create_dir_all(home.path().join("sites")).expect("the directory is made");

        // A real certificate, and a different real key beside it.
        let (certificate, _its_key) = a_leaf(home.path(), "blog.test");
        let stranger = KeyPair::generate_for(&PKCS_ECDSA_P256_SHA256).expect("a second key pair");

        std::fs::write(certificate_path(home.path(), "blog.test"), certificate).expect("written");
        std::fs::write(key_path(home.path(), "blog.test"), stranger.serialize_pem())
            .expect("written");

        assert_eq!(
            read(home.path(), "blog.test", SystemTime::now()),
            CertState::Unusable {
                because: Unusable::KeyAndCertificateDisagree
            }
        );
    }

    /// What a whole pair reports: the names it covers, in order, and a fingerprint of the right
    /// length.
    #[test]
    fn a_whole_pair_reports_what_it_covers() {
        let home = a_home();
        std::fs::create_dir_all(home.path().join("sites")).expect("the directory is made");

        let (certificate, key) = a_leaf(home.path(), "blog.test");
        std::fs::write(certificate_path(home.path(), "blog.test"), certificate).expect("written");
        std::fs::write(key_path(home.path(), "blog.test"), key).expect("written");

        let CertState::Present { cert } = read(home.path(), "blog.test", SystemTime::now()) else {
            panic!("a whole pair did not read as present");
        };

        assert_eq!(cert.sans, vec!["blog.test", "www.blog.test"]);
        assert_eq!(cert.fingerprint.len(), 64);
        assert!(cert.subject.contains("blog.test"));
        assert!(
            cert.days_left > 80 && cert.days_left <= 90,
            "{}",
            cert.days_left
        );
    }

    /// A certificate whose `not_after` has passed is `Present` with a negative count, never
    /// `Unusable`: what it needs is reissuing, and saying so needs it read.
    #[test]
    fn an_expired_certificate_is_present_with_a_negative_count() {
        let home = a_home();
        std::fs::create_dir_all(home.path().join("sites")).expect("the directory is made");

        let (certificate, key) = a_leaf(home.path(), "blog.test");
        std::fs::write(certificate_path(home.path(), "blog.test"), certificate).expect("written");
        std::fs::write(key_path(home.path(), "blog.test"), key).expect("written");

        let later = SystemTime::now() + Duration::from_secs(200 * 24 * 60 * 60);

        let CertState::Present { cert } = read(home.path(), "blog.test", later) else {
            panic!("an expired certificate did not read as present");
        };

        assert!(cert.days_left < 0, "{}", cert.days_left);
    }

    /// A certificate and its key, signed by this home's authority, covering two names.
    ///
    /// Hand-rolled here rather than reusing `ensure`, which does not exist yet — and which is the
    /// point: this module's reader is tested against certificates it did not write.
    fn a_leaf(certs: &Path, domain: &str) -> (String, String) {
        let CaState::Present { ca } = ca::read(certs, SystemTime::now()) else {
            panic!("the fixture home has no authority");
        };

        let authority_key = KeyPair::from_pem(
            &std::fs::read_to_string(ca::key_path(certs)).expect("the authority's key"),
        )
        .expect("it parses");
        let issuer =
            rcgen::Issuer::from_ca_cert_pem(&ca.certificate_pem, authority_key).expect("an issuer");

        let key = KeyPair::generate_for(&PKCS_ECDSA_P256_SHA256).expect("a key pair");
        let mut params = CertificateParams::new(vec![domain.to_owned(), format!("www.{domain}")])
            .expect("the names are valid");
        // The common name the real issuer sets, so this fixture is the shape `ensure` will write
        // rather than a shape only the reader ever sees.
        params
            .distinguished_name
            .push(rcgen::DnType::CommonName, domain.to_owned());
        params.not_before = SystemTime::now().into();
        params.not_after = params.not_before + Duration::from_secs(90 * 24 * 60 * 60);

        let certificate = params.signed_by(&key, &issuer).expect("a certificate");

        (certificate.pem(), key.serialize_pem())
    }
}
