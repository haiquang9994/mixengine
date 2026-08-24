//! Linux: an anchor file, and a command that folds it into the bundle everything else reads.
//!
//! **Two families, and neither is the platform** — the T49a design, D7. Debian keeps machine-added
//! anchors in `/usr/local/share/ca-certificates` and refreshes with `update-ca-certificates`; Red
//! Hat keeps them in `/etc/pki/ca-trust/source/anchors` and refreshes with `update-ca-trust`. A
//! machine with neither directory has no system store MixEngine knows how to write, which is
//! [`TrustStoreMethod::None`] and a supported mode rather than a failure — browsers here read NSS
//! anyway, which is T49b's.
//!
//! **Detected by probing for the directories, never by parsing `/etc/os-release`.**
//! `.claude/features/tls.md` asks for that in a sentence of its own, and a version string is a thing
//! distributions change.
//!
//! **Being in the anchors directory is not the same as being trusted**, which is why the probe
//! checks two things. A file that was written and never folded in — the refresh command failed, or
//! an image was built with one and not the other — is a certificate `curl` does not accept, and
//! reporting it as installed would report a home as working when its HTTPS does not.

#[cfg(feature = "host")]
use crate::{Result, TrustState, TrustStore, TrustStoreMethod};

/// Debian and its derivatives.
#[cfg(feature = "host")]
pub(crate) const DEBIAN_ANCHORS: &str = "/usr/local/share/ca-certificates";

/// Red Hat and its derivatives.
#[cfg(feature = "host")]
pub(crate) const REDHAT_ANCHORS: &str = "/etc/pki/ca-trust/source/anchors";

/// What MixEngine's anchor is called, in whichever directory it lands.
///
/// A constant here and never a value from a request: the helper deletes this name, so a request that
/// could choose it would be a request that could delete another package's anchor.
#[cfg(feature = "host")]
pub(crate) const ANCHOR_FILE: &str = "mixengine.crt";

/// Where the refresh command writes what it generated.
///
/// Debian's `update-ca-certificates` writes this one; Red Hat's `update-ca-trust` writes
/// `/etc/pki/ca-trust/extracted/pem/tls-ca-bundle.pem` and symlinks this path at it on most builds.
/// Both are read by OpenSSL, `curl` and everything that links them.
#[cfg(feature = "host")]
const DEBIAN_BUNDLE: &str = "/etc/ssl/certs/ca-certificates.crt";

/// Red Hat's own.
#[cfg(feature = "host")]
const REDHAT_BUNDLE: &str = "/etc/pki/ca-trust/extracted/pem/tls-ca-bundle.pem";

/// This system's answer.
#[cfg(feature = "host")]
#[derive(Debug, Default)]
pub(crate) struct Trust;

#[cfg(feature = "host")]
impl TrustStore for Trust {
    fn method(&self) -> Result<TrustStoreMethod> {
        Ok(method())
    }

    fn probe(&self, der: &[u8]) -> Result<TrustState> {
        let method = method();

        let (anchors, bundle) = match method {
            TrustStoreMethod::CaCertificates => (DEBIAN_ANCHORS, DEBIAN_BUNDLE),
            TrustStoreMethod::CaTrustAnchors => (REDHAT_ANCHORS, REDHAT_BUNDLE),
            _ => {
                return Ok(TrustState {
                    method: TrustStoreMethod::None,
                    installed: false,
                    missing: Some(
                        "this machine has neither anchors directory, so it has no system trust \
                         store MixEngine knows how to write"
                            .to_owned(),
                    ),
                });
            }
        };

        let anchor = std::path::Path::new(anchors).join(ANCHOR_FILE);

        // Compared as exact bytes, through the PEM envelope the anchor is written in — D6. A subject
        // match would claim another home's authority as this one's.
        let ours = std::fs::read(&anchor)
            .ok()
            .and_then(|text| crate::trust::pem::decode(&text))
            .is_some_and(|found| found == der);

        if !ours {
            return Ok(TrustState {
                method,
                installed: false,
                missing: Some(format!("{} does not hold this authority", anchor.display())),
            });
        }

        // The second question, and the one a file on disk cannot answer: did the refresh command
        // ever run? An anchor that never reached the bundle is trusted by nothing.
        let folded = std::fs::read(bundle)
            .map(|text| contains(&text, der))
            .unwrap_or(false);

        Ok(TrustState {
            method,
            installed: folded,
            missing: (!folded).then(|| {
                format!(
                    "{} holds this authority but {bundle} does not, so nothing on this machine \
                     trusts it yet",
                    anchor.display()
                )
            }),
        })
    }
}

/// Which family this machine is, by looking for the directory rather than by reading a name.
#[cfg(feature = "host")]
fn method() -> TrustStoreMethod {
    if std::path::Path::new(DEBIAN_ANCHORS).is_dir() {
        TrustStoreMethod::CaCertificates
    } else if std::path::Path::new(REDHAT_ANCHORS).is_dir() {
        TrustStoreMethod::CaTrustAnchors
    } else {
        TrustStoreMethod::None
    }
}

/// Does this bundle carry `der` as one of its certificates?
#[cfg(feature = "host")]
fn contains(bundle: &[u8], der: &[u8]) -> bool {
    crate::trust::pem::decode_all(bundle)
        .into_iter()
        .any(|found| found == der)
}
