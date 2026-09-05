//! Putting a replacement `mixengine-elevate` where the elevation prompt will find it — roadmap task
//! **T88a**.
//!
//! **Nothing here installs anything**, and the module is written so that reading it makes that
//! obvious: it fetches two files, checks one against the other, and writes them into
//! `<home>/run/helper/`. What turns them into the file this machine runs as root is
//! [`PrivilegedOp::HelperReplace`](mixengine_proto::privileged::PrivilegedOp::HelperReplace),
//! applied by the *installed* helper behind an explicit prompt, which checks the same signature
//! again against a key of its own.
//!
//! **So why check it twice?** The second check is the security boundary — this process runs as the
//! user and is, if it has been compromised, the attacker. The first one is the user interface:
//! `.claude/features/updates.md` asks that a tampered artifact be refused *with the reason shown*,
//! and a reason that only appears after somebody has clicked Allow is not shown. See the T88a
//! design, D13.
//!
//! **And why a detached signature rather than the SHA-256 inside the signed feed**, which is how
//! every other artifact this product installs is bound? Because the process that has to be
//! convinced never fetched the feed and must not trust the daemon that did. `.claude/features/`
//! `updates.md` states that exception in the same paragraph that states the rule.

use std::path::Path;

use mixengine_proto::privileged::HelperStamp;

use super::feed::HelperArtifact;
use crate::{Error, Result};

/// The suffix minisign gives a detached signature, and `packaging/sign.sh` writes.
const SIGNATURE_SUFFIX: &str = ".minisig";

/// How long a fetch of one small file may take.
///
/// Every path that touches the network has one, per `.claude/standards/rust.md`. Shorter than the
/// index's is not warranted and longer is not either: the two files together are about a megabyte,
/// there is no cache to fall back to, and somebody typed `mix elevation upgrade` and is watching.
const FETCH_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(30);

/// Check `bytes` against `signature`, and read what the signature says they are.
///
/// `public_key` is a parameter on [`crate::blueprints::trust::verify`]'s precedent and for its
/// reason: a compiled-in key cannot answer *does this refuse everything else*, because no test can
/// produce a signature under it.
///
/// # Errors
///
/// [`Error::HelperSignature`] when it is not ours, [`Error::HelperStampUnreadable`] when the signed
/// trusted comment is not the grammar this build reads, and [`Error::HelperNotForThisMachine`] when
/// it is a correctly signed build for somewhere else.
pub fn verify(bytes: &[u8], signature: &str, public_key: &str) -> Result<HelperStamp> {
    let key = minisign_verify::PublicKey::from_base64(public_key).map_err(|source| {
        Error::HelperSignature {
            what: "this build's update key".to_owned(),
            source: Box::new(source),
        }
    })?;

    let decoded =
        minisign_verify::Signature::decode(signature).map_err(|source| Error::HelperSignature {
            what: "the signature published beside it".to_owned(),
            source: Box::new(source),
        })?;

    // `false` refuses minisign's legacy algorithm, as every other verifier in this product does:
    // everything MixEngine publishes is the modern pre-hashed form. The call covers the trusted
    // comment as well as the bytes, which is what makes the stamp below a fact rather than a claim.
    key.verify(bytes, &decoded, false)
        .map_err(|source| Error::HelperSignature {
            what: "the privileged helper this release publishes".to_owned(),
            source: Box::new(source),
        })?;

    let stamp = HelperStamp::parse(decoded.trusted_comment()).ok_or_else(|| {
        Error::HelperStampUnreadable {
            comment: decoded.trusted_comment().to_owned(),
        }
    })?;

    if !stamp.is_for_host() {
        return Err(Error::HelperNotForThisMachine {
            os: stamp.os,
            arch: stamp.arch,
        });
    }

    Ok(stamp)
}

/// Download a helper and its signature, check them, and leave both in `into`.
///
/// The directory is emptied first: what is in one left by an attempt that was killed is half of
/// somebody else's download, which is [`crate::updates::apply::stage`]'s rule one artifact along.
///
/// **Written after the check and not before**, so a payload that fails verification never appears
/// under the name the elevated process reads.
///
/// # Errors
///
/// [`Error::ArtifactTransport`] for either fetch, [`Error::Io`] for the directory and the two
/// files, and whatever [`verify`] refused.
pub async fn stage(
    http: &reqwest::Client,
    artifact: &HelperArtifact,
    public_key: &str,
    into: &Path,
) -> Result<HelperStamp> {
    let bytes = fetch(http, &artifact.url).await?;
    let signature = fetch(http, &format!("{}{SIGNATURE_SUFFIX}", artifact.url)).await?;
    let signature = String::from_utf8(signature).map_err(|_| Error::HelperStampUnreadable {
        comment: "the file published beside it is not a minisign signature".to_owned(),
    })?;

    let stamp = verify(&bytes, &signature, public_key)?;

    clear(into).await?;
    crate::paths::create_dir(into)?;

    let name = name();
    write(&into.join(&name), &bytes)?;
    write(
        &into.join(format!("{name}{SIGNATURE_SUFFIX}")),
        signature.as_bytes(),
    )?;

    Ok(stamp)
}

/// Take the staged candidate away again.
///
/// Called after a replacement has been applied, and before staging a new one. Removing a directory
/// that is not there is not an error — the caller wanted it gone and it is.
///
/// # Errors
///
/// [`Error::Io`] naming the directory.
pub async fn clear(into: &Path) -> Result<()> {
    crate::paths::remove_dir(into).await
}

/// What the candidate is called, which is what
/// [`mixengine_proto::privileged::helper_candidate`] composes on the other side.
fn name() -> String {
    format!("mixengine-elevate{}", std::env::consts::EXE_SUFFIX)
}

/// One file, whole, with a timeout.
async fn fetch(http: &reqwest::Client, url: &str) -> Result<Vec<u8>> {
    let response = http
        .get(url)
        .timeout(FETCH_TIMEOUT)
        .send()
        .await
        .and_then(reqwest::Response::error_for_status)
        .map_err(|source| Error::ArtifactTransport {
            url: url.to_owned(),
            source: Box::new(source),
        })?;

    response
        .bytes()
        .await
        .map(|bytes| bytes.to_vec())
        .map_err(|source| Error::ArtifactTransport {
            url: url.to_owned(),
            source: Box::new(source),
        })
}

/// One file, written where the elevated process will look for it.
fn write(path: &Path, bytes: &[u8]) -> Result<()> {
    std::fs::write(path, bytes).map_err(|source| Error::Io {
        action: "write the staged privileged helper",
        path: path.to_path_buf(),
        source,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The grammar, for this machine, at whatever version the caller wants to talk about.
    fn comment(version: &str) -> String {
        format!(
            "{} {version} {} {}",
            HelperStamp::LABEL,
            HelperStamp::host_os(),
            HelperStamp::host_arch()
        )
    }

    #[test]
    fn a_candidate_this_key_signed_reads_back_its_stamp() {
        let signer = mixengine_testkit::Signer::new();
        let bytes = b"a helper, allegedly";
        let signature = signer.sign_with_comment(bytes, &comment("0.2.0"));

        let stamp = verify(bytes, &signature, &signer.public_key()).expect("our own signature");

        assert_eq!(stamp.version, "0.2.0");
        assert!(stamp.is_for_host());
    }

    /// The daemon's check is the user interface and the helper's is the boundary — but a mirror
    /// answering with rubbish must cost a sentence rather than an elevation prompt, and this test
    /// is that acceptance criterion.
    #[test]
    fn a_candidate_somebody_else_signed_is_refused_with_a_reason() {
        let signer = mixengine_testkit::Signer::new();
        let stranger = mixengine_testkit::Signer::new();
        let bytes = b"a helper, allegedly";
        let signature = stranger.sign_with_comment(bytes, &comment("0.2.0"));

        let error =
            verify(bytes, &signature, &signer.public_key()).expect_err("somebody else's signature");

        assert!(matches!(error, Error::HelperSignature { .. }), "{error:?}");
    }

    #[test]
    fn a_candidate_edited_after_it_was_signed_is_refused() {
        let signer = mixengine_testkit::Signer::new();
        let signature = signer.sign_with_comment(b"a helper", &comment("0.2.0"));

        let error = verify(b"a helper, tampered!", &signature, &signer.public_key())
            .expect_err("bytes that are not the ones signed");

        assert!(matches!(error, Error::HelperSignature { .. }), "{error:?}");
    }

    #[test]
    fn a_candidate_for_another_machine_is_refused() {
        let signer = mixengine_testkit::Signer::new();
        let bytes = b"somebody else's helper";
        let signature =
            signer.sign_with_comment(bytes, &format!("{} 9.9.9 plan9 s390x", HelperStamp::LABEL));

        let error =
            verify(bytes, &signature, &signer.public_key()).expect_err("another machine's build");

        assert!(
            matches!(&error, Error::HelperNotForThisMachine { os, .. } if os == "plan9"),
            "{error:?}"
        );
    }

    #[test]
    fn a_trusted_comment_this_build_cannot_read_is_refused() {
        let signer = mixengine_testkit::Signer::new();
        let bytes = b"a helper";
        let signature = signer.sign_with_comment(bytes, "something else entirely");

        let error = verify(bytes, &signature, &signer.public_key())
            .expect_err("a comment nothing can read");

        assert!(
            matches!(error, Error::HelperStampUnreadable { .. }),
            "{error:?}"
        );
    }

    /// A key that is not a key is this build's fault and not the release's, and the two must not
    /// read the same to whoever is looking at the message.
    #[test]
    fn a_public_key_that_is_not_one_names_itself() {
        let signer = mixengine_testkit::Signer::new();
        let signature = signer.sign_with_comment(b"a helper", &comment("0.2.0"));

        let error = verify(b"a helper", &signature, "not a key at all")
            .expect_err("a key that will not parse");

        assert!(
            matches!(&error, Error::HelperSignature { what, .. } if what.contains("update key")),
            "{error:?}"
        );
    }
}
