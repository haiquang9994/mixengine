//! Is this the certificate `mixengine_core::certs::ca` generates? — the T49a design, D4.
//!
//! **Not a security boundary against a compromised daemon, and saying so is the point.** One that
//! could forge a `TrustCaInstall` already holds the private key of the authority this machine
//! trusts, and can sign any certificate for any name without installing a second root. What this
//! file does is keep the set of certificates an install could ever have created **enumerable**, so
//! that `mix cert ca-uninstall` (T54) and uninstall (T87) can be sure they removed all of it. An
//! unconstrained install could leave behind a root called anything at all, which nothing would ever
//! find again.
//!
//! **The removal direction is where a check has teeth**, and it is not here: the wire type carries
//! no fingerprint at all, so nothing arriving at the helper can describe a certificate this project
//! did not make. See `mixengine_proto::privileged::TrustTarget`.
//!
//! Pure, and compiled on all three systems so that a developer on any one of them can test the check
//! for all three — the arrangement [`crate::resolver`] and [`crate::port_access`] already use.

use super::der::{self, Element, Malformed};

/// Larger than any certificate T48 produces, and small enough that a request cannot be a denial of
/// service by length alone. A P-256 authority is a little over 400 bytes.
pub const MAX_DER: usize = 8 * 1024;

/// What T48 writes into the common name, before the identifier.
const SUBJECT_PREFIX: &str = "MixEngine Local CA ";

/// `mixengine_core::certs::ca`'s `KEY_ID_LENGTH`.
///
/// **Duplicated deliberately.** This check runs inside a binary excluded from auto-update, so it
/// cannot depend on a constant a newer daemon might have changed — and a check that asked the thing
/// it is checking how long its own identifier is would not be a check.
const KEY_ID_LENGTH: usize = 8;

/// The most years between `notBefore` and `notAfter`.
///
/// T48 asks for ten; the extra one is a leap-day and time-zone margin rather than a policy.
const MAX_YEARS: i32 = 11;

/// `2.5.4.3`, commonName.
const OID_COMMON_NAME: &[u8] = &[0x55, 0x04, 0x03];
/// `2.5.29.15`, keyUsage.
const OID_KEY_USAGE: &[u8] = &[0x55, 0x1d, 0x0f];
/// `2.5.29.17`, subjectAltName.
const OID_SUBJECT_ALT_NAME: &[u8] = &[0x55, 0x1d, 0x11];
/// `2.5.29.19`, basicConstraints.
const OID_BASIC_CONSTRAINTS: &[u8] = &[0x55, 0x1d, 0x13];

/// `SEQUENCE { BOOLEAN TRUE, INTEGER 0 }` — `CA:TRUE, pathlen:0`, and nothing else.
///
/// **Compared as bytes rather than parsed.** There is exactly one DER encoding this may have, so the
/// comparison is both the check and its own proof, and it cannot be widened by a parser being
/// lenient somewhere. A test generates a real T48-shaped certificate and asserts these are the bytes
/// inside it, so a change in `rcgen` fails loudly rather than quietly accepting more.
const BASIC_CONSTRAINTS_CA_PATHLEN_0: &[u8] = &[0x30, 0x06, 0x01, 0x01, 0xff, 0x02, 0x01, 0x00];

/// `BIT STRING`, one unused bit, `keyCertSign | cRLSign` — and no other bit set.
const KEY_USAGE_CERT_SIGN_AND_CRL_SIGN: &[u8] = &[0x03, 0x02, 0x01, 0x06];

/// An authority this build is willing to put into a store, or to take out of one.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Authority {
    /// The eight hex characters after the prefix — what a removal names.
    pub key_id: String,

    /// The whole common name, for the audit line and for the outcome a person reads.
    pub subject: String,
}

/// Why these bytes will not be installed.
pub type Refused = Malformed;

/// Are `der`'s bytes shaped like an authority T48 generated?
///
/// # Errors
///
/// One sentence naming the rule that was broken, which is what reaches the audit log and the
/// operation's outcome.
pub fn ours(der: &[u8]) -> Result<Authority, Refused> {
    if der.len() > MAX_DER {
        return Err(Malformed(format!(
            "{} bytes, and no authority MixEngine makes is over {MAX_DER}",
            der.len()
        )));
    }

    let certificate = der::only(der)?;
    let inside = der::children(certificate.expect(der::SEQUENCE, "the certificate")?)?;
    let body = inside
        .first()
        .ok_or_else(|| Malformed("a certificate with nothing in it".to_owned()))?;

    let mut fields =
        der::children(body.expect(der::SEQUENCE, "the certificate body")?)?.into_iter();

    // `[0] EXPLICIT version` is optional in the grammar and present on everything modern.
    let mut field = fields
        .next()
        .ok_or_else(|| Malformed("a certificate body with nothing in it".to_owned()))?;
    if field.tag == der::CONTEXT_0 {
        field = fields
            .next()
            .ok_or_else(|| Malformed("no serial number".to_owned()))?;
    }

    // The serial number and the signature algorithm, neither of which this check reads: a serial is
    // whatever the generator chose, and the algorithm is settled by the key the store will hold.
    field.expect(der::INTEGER, "the serial number")?;
    let _algorithm = fields
        .next()
        .ok_or_else(|| Malformed("no signature algorithm".to_owned()))?;

    let issuer = fields
        .next()
        .ok_or_else(|| Malformed("no issuer".to_owned()))?;
    let validity = fields
        .next()
        .ok_or_else(|| Malformed("no validity".to_owned()))?;
    let subject = fields
        .next()
        .ok_or_else(|| Malformed("no subject".to_owned()))?;

    // **An authority MixEngine generated signed itself**, so the two names are the same bytes. A
    // slice comparison, needing no name parsing at all — which is why `Element` carries `raw`.
    if issuer.raw != subject.raw {
        return Err(Malformed(
            "the issuer and the subject differ, so this authority did not sign itself".to_owned(),
        ));
    }

    within_years(validity)?;

    let subject = common_name(subject)?;
    let key_id = key_id(&subject)?;

    // Past the public key and the two optional unique identifiers, to the extensions.
    let extensions = fields
        .find(|field| field.tag == der::CONTEXT_3)
        .ok_or_else(|| Malformed("no extensions, so no basic constraints either".to_owned()))?;

    check_extensions(extensions)?;

    Ok(Authority { key_id, subject })
}

/// Is `candidate` the identifier of a MixEngine authority?
///
/// **Checked before a store is opened**, on a value that arrived over the wire — see
/// `mixengine_proto::privileged::TrustTarget`. Eight lowercase hex characters cannot name a
/// corporate root, which is the whole of why the removal operation carries this and not a hash.
#[must_use]
pub fn is_key_id(candidate: &str) -> bool {
    candidate.len() == KEY_ID_LENGTH
        && candidate
            .bytes()
            .all(|byte| byte.is_ascii_digit() || matches!(byte, b'a'..=b'f'))
}

/// The subject a certificate must carry to be this authority.
#[must_use]
pub fn subject_of(key_id: &str) -> String {
    format!("{SUBJECT_PREFIX}{key_id}")
}

/// `basicConstraints` and `keyUsage` are exactly right, and there is no `subjectAltName`.
fn check_extensions(extensions: Element<'_>) -> Result<(), Refused> {
    let sequence = der::only(extensions.contents)?;
    let mut seen_basic_constraints = false;
    let mut seen_key_usage = false;

    for extension in der::children(sequence.expect(der::SEQUENCE, "the extensions")?)? {
        let parts = der::children(extension.expect(der::SEQUENCE, "an extension")?)?;
        let oid = parts
            .first()
            .ok_or_else(|| Malformed("an extension with no identifier".to_owned()))?
            .expect(der::OID, "an extension's identifier")?;

        // `critical` is an optional BOOLEAN between the two, so the value is whichever part is the
        // OCTET STRING rather than whichever part is second.
        let value = parts
            .iter()
            .find(|part| part.tag == der::OCTET_STRING)
            .ok_or_else(|| Malformed("an extension with no value".to_owned()))?
            .contents;

        if oid == OID_SUBJECT_ALT_NAME {
            // `security-model.md`'s own words: an authority is not a server, and a name on one
            // invites something to accept it as a leaf.
            return Err(Malformed(
                "an authority carrying a subject alternative name, which is a server's field"
                    .to_owned(),
            ));
        } else if oid == OID_BASIC_CONSTRAINTS {
            if value != BASIC_CONSTRAINTS_CA_PATHLEN_0 {
                return Err(Malformed(
                    "basic constraints that are not exactly CA:TRUE with pathlen:0".to_owned(),
                ));
            }
            seen_basic_constraints = true;
        } else if oid == OID_KEY_USAGE {
            if value != KEY_USAGE_CERT_SIGN_AND_CRL_SIGN {
                return Err(Malformed(
                    "key usage that is not exactly keyCertSign and cRLSign".to_owned(),
                ));
            }
            seen_key_usage = true;
        }
    }

    if !seen_basic_constraints {
        return Err(Malformed(
            "no basic constraints, so nothing in this says it is an authority".to_owned(),
        ));
    }
    if !seen_key_usage {
        return Err(Malformed(
            "no key usage, so this authority is unconstrained".to_owned(),
        ));
    }

    Ok(())
}

/// The common name out of a `Name`, which is a SEQUENCE OF SET OF SEQUENCE { OID, value }.
fn common_name(subject: Element<'_>) -> Result<String, Refused> {
    for rdn in der::children(subject.expect(der::SEQUENCE, "the subject")?)? {
        for pair in der::children(rdn.expect(der::SET, "a name component")?)? {
            let parts = der::children(pair.expect(der::SEQUENCE, "a name attribute")?)?;
            let (Some(kind), Some(value)) = (parts.first(), parts.get(1)) else {
                continue;
            };

            if kind.expect(der::OID, "a name attribute's type")? != OID_COMMON_NAME {
                continue;
            }

            if value.tag != der::UTF8_STRING && value.tag != der::PRINTABLE_STRING {
                return Err(Malformed(
                    "a common name that is not a text string".to_owned(),
                ));
            }

            return String::from_utf8(value.contents.to_vec())
                .map_err(|_| Malformed("a common name that is not UTF-8".to_owned()));
        }
    }

    Err(Malformed("a subject with no common name".to_owned()))
}

/// The eight characters after the prefix, **as a shape and never recomputed from the key**.
///
/// The T49a design's D4 records why. Recomputing the identifier — SHA-256 of the
/// SubjectPublicKeyInfo, truncated — would need a hash in `mixengine-elevate`, measured at 8 crates
/// with none of them already there, and it would refuse nothing: whoever generates a certificate
/// sets its common name to their own key's identifier and passes. What is worth checking is that the
/// name belongs to a family small enough for uninstall to enumerate, which is this.
fn key_id(common_name: &str) -> Result<String, Refused> {
    let id = common_name.strip_prefix(SUBJECT_PREFIX).ok_or_else(|| {
        Malformed(format!(
            "a common name that is not a MixEngine authority: {common_name}"
        ))
    })?;

    if is_key_id(id) {
        Ok(id.to_owned())
    } else {
        Err(Malformed(format!(
            "a MixEngine authority whose identifier is not {KEY_ID_LENGTH} lowercase hex \
             characters: {id}"
        )))
    }
}

/// At most [`MAX_YEARS`] between the two, **compared as years rather than as dates**.
///
/// Full date arithmetic would mean a UTCTime parser, a GeneralizedTime parser and a civil calendar
/// inside the audited binary, and would refuse nothing this does not: the rule exists to reject a
/// hundred-year root, and a hundred years is visible in the year alone.
fn within_years(validity: Element<'_>) -> Result<(), Refused> {
    let times = der::children(validity.expect(der::SEQUENCE, "the validity")?)?;
    let (Some(before), Some(after)) = (times.first(), times.get(1)) else {
        return Err(Malformed("a validity without two times in it".to_owned()));
    };

    let span = year(*after)? - year(*before)?;

    if (0..=MAX_YEARS).contains(&span) {
        Ok(())
    } else {
        Err(Malformed(format!(
            "an authority valid for {span} years, and MixEngine issues itself {MAX_YEARS} at most"
        )))
    }
}

/// The four-digit year of one time.
fn year(time: Element<'_>) -> Result<i32, Refused> {
    let text = std::str::from_utf8(time.contents)
        .map_err(|_| Malformed("a time that is not text".to_owned()))?;

    // `YYMMDDHHMMSSZ`, where 50 and above is the twentieth century — RFC 5280, 4.1.2.5.1 — against
    // `YYYYMMDDHHMMSSZ`, which says so itself.
    let (digits, four_digit) = match time.tag {
        der::UTC_TIME => (text.get(..2), false),
        der::GENERALIZED_TIME => (text.get(..4), true),
        _ => {
            return Err(Malformed("a validity field that is not a time".to_owned()));
        }
    };

    let digits = digits.ok_or_else(|| Malformed("a time too short to hold a year".to_owned()))?;
    let value: i32 = digits
        .parse()
        .map_err(|_| Malformed(format!("a year that is not a number: {digits}")))?;

    Ok(if four_digit {
        value
    } else if value >= 50 {
        1900 + value
    } else {
        2000 + value
    })
}

#[cfg(test)]
mod tests {
    use rcgen::{BasicConstraints, CertificateParams, DnType, IsCa, KeyPair, KeyUsagePurpose};

    use super::*;

    const A_KEY_ID: &str = "0123abcd";

    /// A certificate shaped the way `mixengine_core::certs::ca` shapes one, with one thing changed.
    ///
    /// **Built here rather than by calling that module**, deliberately: a test that asked the code
    /// under test to generate its own input could not notice both of them being wrong the same way.
    fn an_authority(common_name: &str, edit: impl FnOnce(&mut CertificateParams)) -> Vec<u8> {
        let key = KeyPair::generate().expect("a key pair");
        let mut params = CertificateParams::default();

        params.is_ca = IsCa::Ca(BasicConstraints::Constrained(0));
        params.key_usages = vec![KeyUsagePurpose::KeyCertSign, KeyUsagePurpose::CrlSign];
        // **rcgen's defaults are 1975 to 4096**, which `within_years` refuses and should: 2121
        // years is exactly the hundred-year root that rule exists for. T48 sets both, so a fixture
        // that left them alone would be testing a certificate T48 never makes.
        params.not_before = rcgen::date_time_ymd(2026, 1, 1);
        params.not_after = rcgen::date_time_ymd(2036, 1, 1);
        params
            .distinguished_name
            .push(DnType::CommonName, common_name.to_owned());
        edit(&mut params);

        params
            .self_signed(&key)
            .expect("a certificate")
            .der()
            .to_vec()
    }

    fn ours_shaped(edit: impl FnOnce(&mut CertificateParams)) -> Vec<u8> {
        an_authority(&subject_of(A_KEY_ID), edit)
    }

    #[test]
    fn a_certificate_shaped_like_the_one_t48_makes_is_accepted() {
        let der = ours_shaped(|_| {});

        let authority = ours(&der).expect("the shape T48 generates");

        assert_eq!(authority.key_id, A_KEY_ID);
        assert_eq!(authority.subject, subject_of(A_KEY_ID));
    }

    /// D13: the hand-written reader and a real parser must agree, or one of them is reading a
    /// different document. This is the half of that arrangement neither can do alone.
    #[test]
    fn the_hand_written_reader_agrees_with_a_real_parser() {
        let der = ours_shaped(|_| {});

        let mine = ours(&der).expect("accepted");
        let (_, theirs) = x509_parser::parse_x509_certificate(&der).expect("a certificate");

        assert!(
            theirs.subject().to_string().contains(&mine.subject),
            "{} does not carry {}",
            theirs.subject(),
            mine.subject
        );
        assert!(theirs.is_ca());
        assert_eq!(theirs.issuer(), theirs.subject());
    }

    /// The two byte constants above are what `rcgen` **actually emits**, not what a specification
    /// says it should. A version that changed either fails here rather than quietly widening what
    /// `ours` accepts.
    #[test]
    fn the_expected_extension_bytes_are_the_bytes_rcgen_writes() {
        let der = ours_shaped(|_| {});
        let (_, parsed) = x509_parser::parse_x509_certificate(&der).expect("a certificate");

        let mut basic = false;
        let mut usage = false;
        for extension in parsed.extensions() {
            if extension.oid.as_bytes() == OID_BASIC_CONSTRAINTS {
                assert_eq!(extension.value, BASIC_CONSTRAINTS_CA_PATHLEN_0);
                basic = true;
            } else if extension.oid.as_bytes() == OID_KEY_USAGE {
                assert_eq!(extension.value, KEY_USAGE_CERT_SIGN_AND_CRL_SIGN);
                usage = true;
            }
        }

        assert!(basic && usage, "T48's own shape changed");
    }

    /// **An authority is not a server**, and a name on one invites something to accept it as a leaf.
    #[test]
    fn an_authority_with_a_subject_alternative_name_is_refused() {
        let der = ours_shaped(|params| {
            params.subject_alt_names = vec![rcgen::SanType::DnsName(
                "blog.test".try_into().expect("a name"),
            )];
        });

        assert!(ours(&der).is_err());
    }

    /// A root that could also sign a handshake is not this root.
    #[test]
    fn an_authority_that_can_also_sign_is_refused() {
        let der = ours_shaped(|params| {
            params.key_usages.push(KeyUsagePurpose::DigitalSignature);
        });

        assert!(ours(&der).is_err());
    }

    /// `pathlen:0` is what stops this authority issuing another authority.
    #[test]
    fn an_authority_that_may_issue_authorities_is_refused() {
        let der = ours_shaped(|params| {
            params.is_ca = IsCa::Ca(BasicConstraints::Unconstrained);
        });

        assert!(ours(&der).is_err());
    }

    /// A leaf is not an authority, whatever it is called.
    #[test]
    fn something_that_is_not_an_authority_at_all_is_refused() {
        let der = ours_shaped(|params| {
            params.is_ca = IsCa::NoCa;
            params.key_usages.clear();
        });

        assert!(ours(&der).is_err());
    }

    /// D4: the family of names an install could ever have created is what uninstall enumerates.
    #[test]
    fn a_name_outside_the_family_is_refused() {
        for name in [
            "DigiCert Global Root CA",
            "MixEngine Local CA",
            "MixEngine Local CA 0123abc",
            "MixEngine Local CA 0123ABCD",
            "MixEngine Local CA 0123abcde",
            "MixEngine Local CA 0123abcz",
            "MixEngine Local CA 0123abcd and more",
            "mixengine local ca 0123abcd",
        ] {
            let der = an_authority(name, |_| {});

            assert!(ours(&der).is_err(), "accepted {name}");
        }
    }

    /// D4: a hundred-year root is refused, and by comparing years rather than dates.
    #[test]
    fn an_authority_that_outlives_its_own_specification_is_refused() {
        let der = ours_shaped(|params| {
            params.not_before = rcgen::date_time_ymd(2000, 1, 1);
            params.not_after = rcgen::date_time_ymd(2100, 1, 1);
        });

        assert!(ours(&der).is_err());
    }

    /// And exactly ten years, which is what T48 asks for, is not.
    #[test]
    fn the_lifetime_t48_asks_for_is_accepted() {
        let der = ours_shaped(|params| {
            params.not_before = rcgen::date_time_ymd(2026, 1, 1);
            params.not_after = rcgen::date_time_ymd(2036, 1, 1);
        });

        assert!(ours(&der).is_ok());
    }

    /// D3: the field is attacker-controlled in the model this check exists for.
    #[test]
    fn something_far_too_large_is_refused_before_it_is_read() {
        assert!(ours(&vec![0x30; MAX_DER + 1]).is_err());
    }

    /// Not a certificate at all, and not a panic either.
    #[test]
    fn rubbish_is_refused_rather_than_unwound() {
        assert!(ours(b"not a certificate").is_err());
        assert!(ours(&[]).is_err());
        assert!(ours(&[0x30]).is_err());
    }

    /// Every truncation of a real certificate, which is the input a reader is likeliest to meet
    /// when something went wrong rather than when something was aimed at it.
    #[test]
    fn no_truncation_of_a_real_certificate_can_make_this_panic() {
        let der = ours_shaped(|_| {});

        for cut in 0..der.len() {
            let truncated = der.get(..cut).expect("a prefix of its own length");

            assert!(ours(truncated).is_err(), "accepted {cut} bytes of one");
        }
    }

    /// The value a removal names is checked before any store is opened, and it is checked as a
    /// shape — the T49a design, D5.
    #[test]
    fn only_eight_lowercase_hex_characters_name_an_authority() {
        assert!(is_key_id("0123abcd"));
        assert!(is_key_id("00000000"));

        for wrong in [
            "0123ABCD",
            "0123abc",
            "0123abcde",
            "0123abcg",
            "",
            "../../../etc",
            "DigiCert",
            "0123 bcd",
        ] {
            assert!(!is_key_id(wrong), "accepted {wrong:?}");
        }
    }
}
