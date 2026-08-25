//! Windows: the `Root` store under `LocalMachine`, through the certificate-store API.
//!
//! **Through the API rather than `certutil.exe`** — the T49a design, D6. `.claude/features/tls.md`
//! names CryptoAPI first with `certutil` as a fallback; the fallback is not built. This crate
//! already reaches Windows through `windows-sys` for the resolver's registry work and the named
//! pipe's DACL, and a process spawned from a context holding an administrative token is a larger
//! surface than four API calls.
//!
//! **Opening the store to read needs no administrative token**, which is what makes it affordable
//! for the producer to ask on every daemon start and for `mix doctor` to ask whenever somebody does.
//! `tests/trust.rs` measures that in CI's ordinary `test` job rather than this comment asserting it.
//!
//! Features on `windows-sys` add modules and not crates, so `Win32_Security_Cryptography` does not
//! move `mixengine-elevate`'s dependency budget.

#[cfg(feature = "elevated")]
use crate::trust::Change;
#[cfg(feature = "host")]
use crate::{Result, TrustState, TrustStore, TrustStoreMethod};

/// This system's answer.
#[cfg(feature = "host")]
#[derive(Debug, Default)]
pub(crate) struct Trust;

#[cfg(feature = "host")]
impl TrustStore for Trust {
    fn method(&self) -> Result<TrustStoreMethod> {
        // A constant, unlike Linux: every Windows has this store — D7.
        Ok(TrustStoreMethod::SystemRoot)
    }

    fn probe(&self, der: &[u8]) -> Result<TrustState> {
        // Exact DER bytes — D6. The store offers a SHA-1 property to search by, which is a
        // different value from the SHA-256 `cert.ca_status` reports; carrying two hashes for one
        // identity is how they come apart, and a byte comparison needs neither.
        let installed = super::store::each_certificate(|found| found == der)?;

        Ok(TrustState {
            method: TrustStoreMethod::SystemRoot,
            installed,
            missing: (!installed).then(|| {
                "this machine's Trusted Root Certification Authorities do not hold MixEngine's \
                 certificate authority"
                    .to_owned()
            }),
        })
    }
}

/// Put MixEngine's authority into `LocalMachine\Root`.
#[cfg(feature = "elevated")]
pub(crate) fn apply(plan: &mixengine_proto::privileged::TrustPlan) -> crate::Result<Change> {
    use mixengine_proto::privileged::TrustPlan;

    let der = match plan {
        TrustPlan::SystemRoot { der } => der,
        TrustPlan::SystemKeychain { .. }
        | TrustPlan::CaCertificates { .. }
        | TrustPlan::CaTrustAnchors { .. } => {
            return Err(crate::trust::unsupported(
                "this is Windows, whose trust store is a certificate store rather than a macOS \
                 keychain or a Linux anchors directory",
            ));
        }
    };

    let _lock = crate::trust::held()?;

    if super::store::add(der)? {
        Ok(Change::Written {
            detail: "added MixEngine's certificate authority to this machine's trusted roots"
                .to_owned(),
        })
    } else {
        Ok(Change::Unchanged)
    }
}

/// Take it back out again.
#[cfg(feature = "elevated")]
pub(crate) fn revoke(target: &mixengine_proto::privileged::TrustTarget) -> crate::Result<Change> {
    use mixengine_proto::privileged::TrustTarget;

    let key_id = match target {
        TrustTarget::SystemRoot { key_id } => key_id,
        TrustTarget::SystemKeychain { .. }
        | TrustTarget::CaCertificates { .. }
        | TrustTarget::CaTrustAnchors { .. } => {
            return Err(crate::trust::unsupported(
                "this is Windows, whose trust store is a certificate store",
            ));
        }
    };

    let _lock = crate::trust::held()?;

    // **D5's second check**, run against every certificate the walk finds rather than against
    // anything the request said: a certificate carrying a MixEngine-shaped name is not proof that
    // MixEngine put it there, and nothing that fails this is removed.
    let removed = super::store::remove(|found| {
        crate::trust::ours(found).is_ok_and(|authority| &authority.key_id == key_id)
    })?;

    if removed == 0 {
        Ok(Change::Unchanged)
    } else {
        Ok(Change::Written {
            detail: format!(
                "removed MixEngine's certificate authority {key_id} from this machine's trusted \
                 roots"
            ),
        })
    }
}
