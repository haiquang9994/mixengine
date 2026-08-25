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
use std::time::{Duration, SystemTime};

use mixengine_proto::{CaState, CertState, SiteCert, Timestamp, Unusable};
use rcgen::{
    CertificateParams,
    ExtendedKeyUsagePurpose,
    IsCa,
    KeyPair,
    KeyUsagePurpose,
    PKCS_ECDSA_P256_SHA256,
    // `subject_public_key_info` is a trait method: the DER of the public key is what the agreement
    // check between the two halves is computed over.
    PublicKeyData as _,
};
use sha2::Digest as _;

use super::ca;
use crate::{Error, Result};

/// Ninety days, as `.claude/features/tls.md` asks for.
const LIFETIME: Duration = Duration::from_secs(90 * 24 * 60 * 60);

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

/// Whether [`ensure`] had to write anything.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Issued {
    /// A fresh key and certificate were written.
    Written,

    /// What was already there answered all four questions — see [`ensure`].
    Reused,
}

/// Give this site a certificate covering exactly `domains`, if what is there is not already one.
///
/// `domains[0]` is the primary: it names the files and becomes the common name. The whole slice is
/// the subject alternative name list, in the order given.
///
/// **Four questions, and an existing pair is reused only when all four answer yes:**
///
/// 1. Both halves are there, parse, and are each other's.
/// 2. The names it covers **equal** the names asked for. Not cover — equal. A certificate with a
///    spare name keeps working after somebody deliberately removed that name.
/// 3. More than [`RENEW_WITHIN_DAYS`] remain.
/// 4. It was signed by the authority this home has **now**.
///
/// The fourth is what makes rotation work, and it is the one `.claude/features/tls.md` does not
/// have: after T54 replaces the authority, every old leaf still parses, still covers the right
/// names and still has eighty days left, so a three-question rule would declare every site fine
/// while every browser rejected it. The comparison is the leaf's issuer name against the
/// authority's subject name, which is free because T48 put the key's identity *into* that name —
/// and it gets both rotations right: onto a new key, the identity changes and every leaf is
/// reissued; re-signing the same key keeps it, and the leaves stay valid.
///
/// # Errors
///
/// [`Error::Certificate`] when this home has no usable authority to sign with, when `domains` is
/// empty, or when the machine will not produce a key pair; [`Error::Io`] when the pair cannot be
/// written.
pub fn ensure(certs: &Path, domains: &[String], now: SystemTime) -> Result<(Issued, CertState)> {
    let primary = domains
        .first()
        .ok_or_else(|| refused("no domains were given"))?;

    let CaState::Present { ca } = ca::read(certs, now) else {
        return Err(refused("this home has no usable certificate authority"));
    };

    let state = read(certs, primary, now);

    if reusable(&state, domains, &ca.subject) {
        return Ok((Issued::Reused, state));
    }

    let authority = std::fs::read_to_string(ca::key_path(certs)).map_err(|source| Error::Io {
        action: "read",
        path: ca::key_path(certs),
        source,
    })?;
    let authority = KeyPair::from_pem(&authority).map_err(|source| Error::Certificate {
        action: "read the signing key of",
        subject: "this home's certificate authority".to_owned(),
        source: Box::new(source),
    })?;
    let issuer =
        rcgen::Issuer::from_ca_cert_pem(&ca.certificate_pem, authority).map_err(|source| {
            Error::Certificate {
                action: "read",
                subject: "this home's certificate authority".to_owned(),
                source: Box::new(source),
            }
        })?;

    let key =
        KeyPair::generate_for(&PKCS_ECDSA_P256_SHA256).map_err(|source| Error::Certificate {
            action: "generate a key pair for",
            subject: primary.clone(),
            source: Box::new(source),
        })?;

    let certificate = params(domains, now)?
        .signed_by(&key, &issuer)
        .map_err(|source| Error::Certificate {
            action: "sign a certificate for",
            subject: primary.clone(),
            source: Box::new(source),
        })?;

    crate::paths::create_dir(&certs.join(SITES))?;

    // **The key first**, exactly as `ca::ensure` writes it: a crash between the two leaves a key
    // with no certificate, which `read` names, rather than a certificate with no key, which looks
    // like a certificate whose key was lost.
    mixengine_platform::write_private(&key_path(certs, primary), key.serialize_pem().as_bytes())?;

    let path = certificate_path(certs, primary);
    std::fs::write(&path, certificate.pem()).map_err(|source| Error::Io {
        action: "write",
        path,
        source,
    })?;

    // Read back rather than describing what was just written — `ca::ensure`'s promise, kept here.
    Ok((Issued::Written, read(certs, primary, now)))
}

/// The four questions of [`ensure`], asked of what is on disk.
fn reusable(state: &CertState, domains: &[String], authority: &str) -> bool {
    let CertState::Present { cert } = state else {
        // Question one: `Absent` and `Unusable` both fail it.
        return false;
    };

    cert.sans == domains && cert.days_left > RENEW_WITHIN_DAYS && cert.issuer == authority
}

/// What the certificate says about itself before it is signed.
fn params(domains: &[String], now: SystemTime) -> Result<CertificateParams> {
    let primary = domains
        .first()
        .ok_or_else(|| refused("no domains were given"))?;

    let mut params =
        CertificateParams::new(domains.to_vec()).map_err(|source| Error::Certificate {
            action: "describe a certificate for",
            subject: primary.clone(),
            source: Box::new(source),
        })?;

    params
        .distinguished_name
        .push(rcgen::DnType::CommonName, primary.clone());
    // A leaf is not an authority, and says so rather than leaving it to a default.
    params.is_ca = IsCa::ExplicitNoCa;
    // **Exactly one purpose.** A certificate that could also authenticate a client, or sign code,
    // is a certificate doing something nobody asked a local web server to do.
    params.extended_key_usages = vec![ExtendedKeyUsagePurpose::ServerAuth];
    params.key_usages = vec![KeyUsagePurpose::DigitalSignature];
    // **rcgen leaves this off by default and RFC 5280 says a conforming issuer includes it**, so it
    // is set rather than inherited. It is also what makes `reusable`'s fourth question honest: the
    // comparison there is of names, and the test beside it asserts that this extension agrees with
    // that comparison — an assertion there is nothing to make if the extension is absent.
    params.use_authority_key_identifier_extension = true;
    params.not_before = now.into();
    params.not_after = params.not_before + LIFETIME;

    Ok(params)
}

/// A refusal that is about the request rather than about the machine.
fn refused(because: &str) -> Error {
    Error::Certificate {
        action: "issue a certificate:",
        subject: because.to_owned(),
        source: Box::new(std::io::Error::other(because.to_owned())),
    }
}

#[cfg(test)]
mod tests {
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

    /// A first issue writes both halves and covers exactly what was asked for.
    #[test]
    fn a_first_issue_writes_a_pair_covering_what_was_asked_for() {
        let home = a_home();
        let domains = vec!["blog.test".to_owned(), "www.blog.test".to_owned()];

        let (issued, state) = ensure(home.path(), &domains, SystemTime::now()).expect("it issues");

        assert_eq!(issued, Issued::Written);
        assert!(key_path(home.path(), "blog.test").is_file());
        assert!(certificate_path(home.path(), "blog.test").is_file());

        let CertState::Present { cert } = state else {
            panic!("what was just issued did not read as present");
        };
        assert_eq!(cert.sans, domains);
    }

    /// **Question one through four all answer yes**, so nothing is written and nothing is signed.
    #[test]
    fn a_second_issue_with_the_same_names_writes_nothing() {
        let home = a_home();
        let domains = vec!["blog.test".to_owned()];
        let now = SystemTime::now();

        let (_, first) = ensure(home.path(), &domains, now).expect("it issues");
        let (issued, second) = ensure(home.path(), &domains, now).expect("it answers");

        assert_eq!(issued, Issued::Reused);
        assert_eq!(first, second, "the certificate was replaced");
    }

    /// A name **added** fails question two.
    #[test]
    fn a_domain_added_reissues() {
        let home = a_home();
        let now = SystemTime::now();

        let (_, before) = ensure(home.path(), &["blog.test".to_owned()], now).expect("it issues");
        let (issued, after) = ensure(
            home.path(),
            &["blog.test".to_owned(), "www.blog.test".to_owned()],
            now,
        )
        .expect("it issues");

        assert_eq!(issued, Issued::Written);
        assert_ne!(before, after);
    }

    /// **And a name removed fails it too**, which is the case a "covers" rule passes and an
    /// "equals" rule catches: a certificate with a spare name keeps working after somebody
    /// deliberately took that name away.
    #[test]
    fn a_domain_removed_reissues() {
        let home = a_home();
        let now = SystemTime::now();

        let (_, before) = ensure(
            home.path(),
            &["blog.test".to_owned(), "www.blog.test".to_owned()],
            now,
        )
        .expect("it issues");
        let (issued, after) =
            ensure(home.path(), &["blog.test".to_owned()], now).expect("it issues");

        assert_eq!(issued, Issued::Written);
        assert_ne!(before, after);

        let CertState::Present { cert } = after else {
            panic!("not present");
        };
        assert_eq!(cert.sans, vec!["blog.test"]);
    }

    /// Question three, from both sides of the line.
    #[test]
    fn a_certificate_is_reissued_once_it_is_inside_the_renewal_window() {
        let home = a_home();
        let domains = vec!["blog.test".to_owned()];
        let now = SystemTime::now();

        ensure(home.path(), &domains, now).expect("it issues");

        let day = Duration::from_secs(24 * 60 * 60);

        // 90 - 50 = 40 days left: outside the window.
        let (issued, _) = ensure(home.path(), &domains, now + day * 50).expect("it answers");
        assert_eq!(issued, Issued::Reused);

        // 90 - 70 = 20 days left: inside it.
        let (issued, _) = ensure(home.path(), &domains, now + day * 70).expect("it issues");
        assert_eq!(issued, Issued::Written);
    }

    /// **Question four, and the one that makes T54 work.** A leaf signed by an authority this home
    /// no longer has still parses, still covers the right names and still has eighty days left — so
    /// a three-question rule declares it fine and every browser rejects it.
    #[test]
    fn a_leaf_from_another_authority_is_reissued() {
        let home = a_home();
        let domains = vec!["blog.test".to_owned()];
        let now = SystemTime::now();

        ensure(home.path(), &domains, now).expect("it issues");
        let before = read(home.path(), "blog.test", now);

        // Rotate: a second authority, written over the first, exactly as T54 will.
        let elsewhere = tempfile::tempdir().expect("a second temp home");
        ca::ensure(elsewhere.path(), now).expect("a second authority");
        std::fs::copy(ca::key_path(elsewhere.path()), ca::key_path(home.path()))
            .expect("the key is replaced");
        std::fs::copy(
            ca::certificate_path(elsewhere.path()),
            ca::certificate_path(home.path()),
        )
        .expect("the certificate is replaced");

        let (issued, after) = ensure(home.path(), &domains, now).expect("it issues");

        assert_eq!(
            issued,
            Issued::Written,
            "a leaf from the old authority was kept"
        );
        assert_ne!(before, after);
    }

    /// A home with no authority refuses and writes nothing — the state a start whose generation
    /// failed leaves, and the one `mix cert issue` reports rather than crashes on.
    #[test]
    fn issuing_without_an_authority_writes_nothing() {
        let home = tempfile::tempdir().expect("a temp home");

        let refused = ensure(home.path(), &["blog.test".to_owned()], SystemTime::now());

        assert!(refused.is_err());
        assert!(!certificate_path(home.path(), "blog.test").exists());
    }

    /// An empty list of names is refused rather than producing a certificate covering nothing.
    #[test]
    fn issuing_for_no_names_at_all_is_refused() {
        let home = a_home();

        assert!(ensure(home.path(), &[], SystemTime::now()).is_err());
    }

    /// **The cheap check of question four agrees with the expensive one.** D6 compares issuer and
    /// subject names because T48 put the key's identity in the name; this asserts that the
    /// `authorityKeyIdentifier` rcgen writes says the same thing, so the shortcut stays honest.
    #[test]
    fn the_issued_leaf_points_at_the_authoritys_key() {
        let home = a_home();
        ensure(home.path(), &["blog.test".to_owned()], SystemTime::now()).expect("it issues");

        let leaf = std::fs::read_to_string(certificate_path(home.path(), "blog.test"))
            .expect("the certificate");
        let leaf = pem::parse(&leaf).expect("an envelope").into_contents();
        let (_, leaf) = x509_parser::parse_x509_certificate(&leaf).expect("it parses");

        let root =
            std::fs::read_to_string(ca::certificate_path(home.path())).expect("the authority");
        let root = pem::parse(&root).expect("an envelope").into_contents();
        let (_, root) = x509_parser::parse_x509_certificate(&root).expect("it parses");

        let authority_key_id = leaf
            .get_extension_unique(&x509_parser::oid_registry::OID_X509_EXT_AUTHORITY_KEY_IDENTIFIER)
            .expect("one at most")
            .expect("the leaf carries one");
        let subject_key_id = root
            .get_extension_unique(&x509_parser::oid_registry::OID_X509_EXT_SUBJECT_KEY_IDENTIFIER)
            .expect("one at most")
            .expect("the authority carries one");

        let x509_parser::extensions::ParsedExtension::AuthorityKeyIdentifier(authority) =
            authority_key_id.parsed_extension()
        else {
            panic!("the authority key identifier did not parse");
        };
        let x509_parser::extensions::ParsedExtension::SubjectKeyIdentifier(subject) =
            subject_key_id.parsed_extension()
        else {
            panic!("the subject key identifier did not parse");
        };

        assert_eq!(
            authority.key_identifier.as_ref().map(|id| id.0),
            Some(subject.0),
            "the leaf does not point at the authority that signed it"
        );
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
