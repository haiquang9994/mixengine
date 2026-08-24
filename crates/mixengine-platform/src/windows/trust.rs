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
