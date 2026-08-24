//! The PEM envelope, on the two systems whose trust stores are made of text files.
//!
//! **Unix only, and that is why the dependency is declared per target.** Windows carries DER through
//! the certificate-store API from end to end and never sees an envelope, so its build of
//! `mixengine-elevate` does not gain the crate at all.
//!
//! **Taken rather than hand-written, and the measurement is why the answer differs from
//! `super::der`'s.** The T49a design's D11 refused `x509-parser` at 22 net crates and `sha2` at 8,
//! both into a binary that runs as root. `pem` is **two** — itself and `base64` — and what it
//! replaces is base64 with its padding rules, which is the kind of thing that is wrong in a way
//! nobody notices until a certificate with the wrong length shows up. The rule that file states is
//! that adding a line is a decision somebody has to argue for, not that the number may never go up.
//!
//! It is also the version `rcgen` pins, so the workspace holds one copy and a certificate written by
//! `mixengine_core::certs::ca` is read back through the same code that wrote it.

/// What a certificate is labelled in a PEM file.
const CERTIFICATE: &str = "CERTIFICATE";

/// The DER inside a one-certificate PEM document, or [`None`] when it is not one.
///
/// **Linux's, in both directions, and nowhere else** — the mirror of `decode_all`'s note. Linux
/// reads its own anchor as a single document and writes one; macOS only ever reads the keychain's
/// whole list. Compiled for the tests everywhere so the envelope both systems depend on is still
/// exercised on the machine that runs them.
///
/// Named rather than linked, in both notes, because the two are gated onto **different systems** —
/// a link from either to the other is a link rustdoc cannot resolve on the OS where only one of
/// them is compiled, which is a broken-link failure in the `test` job of the OS in question.
#[cfg(any(target_os = "linux", test))]
pub(crate) fn decode(text: &[u8]) -> Option<Vec<u8>> {
    let parsed = pem::parse(text).ok()?;

    (parsed.tag() == CERTIFICATE).then(|| parsed.contents().to_vec())
}

/// Every certificate in a bundle, which is what a system trust file is.
///
/// Anything that is not a certificate is skipped rather than refused: a real bundle carries comments
/// and, on some distributions, other labels between the blocks.
///
/// **Both probes read a bundle, and only macOS writes against one**: Linux's writer reads its single
/// anchor with `decode` and never a list, so an `elevated`-only build there has no caller for this.
#[cfg(any(feature = "host", target_os = "macos"))]
pub(crate) fn decode_all(text: &[u8]) -> Vec<Vec<u8>> {
    pem::parse_many(text)
        .unwrap_or_default()
        .into_iter()
        .filter(|block| block.tag() == CERTIFICATE)
        .map(|block| block.contents().to_vec())
        .collect()
}

/// One certificate as a PEM document, which is what an anchors directory holds and what
/// `security add-trusted-cert` reads.
#[cfg(feature = "elevated")]
pub(crate) fn encode(der: &[u8]) -> String {
    pem::encode(&pem::Pem::new(CERTIFICATE, der.to_vec()))
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A certificate in an envelope, the way both stores hold one.
    fn enveloped(der: &[u8]) -> String {
        pem::encode(&pem::Pem::new("CERTIFICATE", der.to_vec()))
    }

    /// What a store holds comes back as the bytes that were put in it, which is the only property
    /// either probe depends on.
    #[test]
    fn a_certificate_survives_the_envelope() {
        let der = vec![0x30, 0x82, 0x01, 0x00];

        assert_eq!(decode(enveloped(&der).as_bytes()), Some(der));
    }

    /// A bundle is many, and the count is what the Linux probe turns on.
    #[test]
    fn every_certificate_in_a_bundle_comes_back() {
        let first = vec![1, 2, 3];
        let second = vec![4, 5, 6];
        let bundle = format!(
            "# a comment a real bundle carries\n{}{}",
            enveloped(&first),
            enveloped(&second)
        );

        assert_eq!(decode_all(bundle.as_bytes()), vec![first, second]);
    }

    /// Not a PEM document at all is [`None`], never a panic and never an empty certificate.
    #[test]
    fn something_that_is_not_an_envelope_is_not_a_certificate() {
        assert_eq!(decode(b"not a certificate"), None);
        assert_eq!(decode(b""), None);
        assert!(decode_all(b"not a bundle").is_empty());
    }

    /// A private key is a PEM document and is **not** a certificate. Reading one as if it were is
    /// how a store ends up holding something nobody meant to publish.
    #[test]
    fn a_key_in_an_envelope_is_not_read_as_a_certificate() {
        let key = pem::encode(&pem::Pem::new("PRIVATE KEY", vec![1, 2, 3]));

        assert_eq!(decode(key.as_bytes()), None);
        assert!(decode_all(key.as_bytes()).is_empty());
    }
}
