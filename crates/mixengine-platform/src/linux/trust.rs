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

#[cfg(feature = "elevated")]
use crate::trust::Change;
#[cfg(feature = "host")]
use crate::{Result, TrustState, TrustStore, TrustStoreMethod};

/// Debian and its derivatives.
#[cfg(any(feature = "host", feature = "elevated"))]
pub(crate) const DEBIAN_ANCHORS: &str = "/usr/local/share/ca-certificates";

/// Red Hat and its derivatives.
#[cfg(any(feature = "host", feature = "elevated"))]
pub(crate) const REDHAT_ANCHORS: &str = "/etc/pki/ca-trust/source/anchors";

/// What MixEngine's anchor is called, in whichever directory it lands.
///
/// A constant here and never a value from a request: the helper deletes this name, so a request that
/// could choose it would be a request that could delete another package's anchor.
#[cfg(any(feature = "host", feature = "elevated"))]
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

/// Where the refresh command lives, and its one fixed argument vector.
///
/// **One fixed command with no argument from the request** — the shape T42 established with `pfctl`
/// and T45 kept with `systemctl`. This binary never runs an *arbitrary* command; a constant argument
/// vector is not one, and there is no API for this that does not go through the distribution's own
/// tool.
#[cfg(feature = "elevated")]
const DEBIAN_REFRESH: [&str; 1] = ["update-ca-certificates"];

/// Red Hat's.
#[cfg(feature = "elevated")]
const REDHAT_REFRESH: [&str; 2] = ["update-ca-trust", "extract"];

/// Write the anchor and fold it in.
#[cfg(feature = "elevated")]
pub(crate) fn apply(plan: &mixengine_proto::privileged::TrustPlan) -> crate::Result<Change> {
    use mixengine_proto::privileged::TrustPlan;

    let (der, anchors, refresh) = match plan {
        TrustPlan::CaCertificates { der } => (der, DEBIAN_ANCHORS, &DEBIAN_REFRESH[..]),
        TrustPlan::CaTrustAnchors { der } => (der, REDHAT_ANCHORS, &REDHAT_REFRESH[..]),
        TrustPlan::SystemRoot { .. } | TrustPlan::SystemKeychain { .. } => {
            return Err(crate::trust::unsupported(
                "this is Linux, whose trust store is an anchors directory rather than a Windows \
                 certificate store or a macOS keychain",
            ));
        }
    };

    if !std::path::Path::new(anchors).is_dir() {
        return Err(crate::trust::unsupported(&format!(
            "{anchors} is not a directory on this machine"
        )));
    }

    let _lock = crate::trust::held()?;
    let anchor = std::path::Path::new(anchors).join(ANCHOR_FILE);
    let wanted = crate::trust::pem::encode(der);

    // Read before writing, under the lock: an anchor that already holds exactly this is the answer
    // `Unchanged`, and rewriting it would run the refresh command for nothing.
    if std::fs::read(&anchor).is_ok_and(|found| found == wanted.as_bytes()) {
        return Ok(Change::Unchanged);
    }

    std::fs::write(&anchor, &wanted).map_err(|source| crate::Error::Io {
        action: "write MixEngine's certificate authority into this machine's anchors",
        path: anchor.clone(),
        source,
    })?;

    run(refresh)?;

    Ok(Change::Written {
        detail: format!(
            "wrote {} and refreshed this machine's trusted roots",
            anchor.display()
        ),
    })
}

/// Remove the anchor, if what is in it is ours, and fold the removal in.
#[cfg(feature = "elevated")]
pub(crate) fn revoke(target: &mixengine_proto::privileged::TrustTarget) -> crate::Result<Change> {
    use mixengine_proto::privileged::TrustTarget;

    let (key_id, anchors, refresh) = match target {
        TrustTarget::CaCertificates { key_id } => (key_id, DEBIAN_ANCHORS, &DEBIAN_REFRESH[..]),
        TrustTarget::CaTrustAnchors { key_id } => (key_id, REDHAT_ANCHORS, &REDHAT_REFRESH[..]),
        TrustTarget::SystemRoot { .. } | TrustTarget::SystemKeychain { .. } => {
            return Err(crate::trust::unsupported(
                "this is Linux, whose trust store is an anchors directory",
            ));
        }
    };

    let _lock = crate::trust::held()?;
    let anchor = std::path::Path::new(anchors).join(ANCHOR_FILE);

    let Ok(found) = std::fs::read(&anchor) else {
        return Ok(Change::Unchanged);
    };

    // **D5's second check.** A file sitting at MixEngine's name is not proof that MixEngine wrote
    // it, so what is there has to be shaped like one of ours *and* be the authority that was named
    // before it is unlinked.
    let ours = crate::trust::pem::decode(&found)
        .and_then(|der| crate::trust::ours(&der).ok())
        .is_some_and(|authority| &authority.key_id == key_id);

    if !ours {
        return Ok(Change::Unchanged);
    }

    std::fs::remove_file(&anchor).map_err(|source| crate::Error::Io {
        action: "remove MixEngine's certificate authority from this machine's anchors",
        path: anchor.clone(),
        source,
    })?;

    run(refresh)?;

    Ok(Change::Written {
        detail: format!(
            "removed {} and refreshed this machine's trusted roots",
            anchor.display()
        ),
    })
}

/// Run one of the two fixed refresh commands.
#[cfg(feature = "elevated")]
fn run(command: &[&str]) -> crate::Result<()> {
    let Some((program, arguments)) = command.split_first() else {
        return Ok(());
    };

    let output = std::process::Command::new(program)
        .args(arguments)
        .output()
        .map_err(|source| crate::Error::Os {
            action: "run this machine's certificate refresh command",
            source,
        })?;

    if output.status.success() {
        return Ok(());
    }

    Err(crate::Error::Os {
        action: "refresh this machine's trusted roots",
        source: std::io::Error::other(String::from_utf8_lossy(&output.stderr).trim().to_owned()),
    })
}
