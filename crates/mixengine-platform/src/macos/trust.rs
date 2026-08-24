//! macOS: the System keychain, through `/usr/bin/security`.
//!
//! **Not through Security.framework, and that is a decision about the other binary** — the T49a
//! design, D6. `SecCertificateCreateWithData`, `SecItemAdd` and `SecTrustSettingsSetTrustSettings`
//! would mean a new unsafe FFI surface inside `mixengine-elevate`, whose whole design constraint is
//! that a person can audit it by reading it, for an operation that runs once per install. The rule
//! T42 set with `pfctl` and T45 kept with `systemctl` holds here: one fixed command, a constant
//! argument vector, and no argument taken from the request.
//!
//! Reading is `security find-certificate -a -p`, which lists every certificate in a keychain as PEM
//! and **needs no administrative token** — measured by `tests/trust.rs` in CI's ordinary `test` job
//! rather than asserted here.

#[cfg(feature = "host")]
use crate::{Result, TrustState, TrustStore, TrustStoreMethod};

/// The keychain a machine-wide root belongs in.
#[cfg(feature = "host")]
pub(crate) const SYSTEM_KEYCHAIN: &str = "/Library/Keychains/System.keychain";

/// Absolute, never resolved through `PATH`: this is invoked from a process holding an
/// administrative token, and a `PATH` entry is something another program can arrange.
#[cfg(feature = "host")]
pub(crate) const SECURITY: &str = "/usr/bin/security";

/// This system's answer.
#[cfg(feature = "host")]
#[derive(Debug, Default)]
pub(crate) struct Trust;

#[cfg(feature = "host")]
impl TrustStore for Trust {
    fn method(&self) -> Result<TrustStoreMethod> {
        // A constant, unlike Linux: every macOS has this keychain — D7.
        Ok(TrustStoreMethod::SystemKeychain)
    }

    fn probe(&self, der: &[u8]) -> Result<TrustState> {
        let listed = certificates()?;

        // Exact DER bytes — D6. `security` also offers `-Z`, which prints SHA-1; that is a
        // different value from the SHA-256 `cert.ca_status` reports, and carrying two hashes for one
        // identity is how they come apart.
        let installed = listed.iter().any(|found| found == der);

        Ok(TrustState {
            method: TrustStoreMethod::SystemKeychain,
            installed,
            missing: (!installed).then(|| {
                format!("{SYSTEM_KEYCHAIN} does not hold MixEngine's certificate authority")
            }),
        })
    }
}

/// Every certificate in the System keychain, as DER.
///
/// **An empty keychain is an empty list and not an error.** `security` exits non-zero when it finds
/// nothing, which is a true answer to the question this asks and must not become a failure that
/// stops a daemon start.
#[cfg(feature = "host")]
fn certificates() -> Result<Vec<Vec<u8>>> {
    let output = std::process::Command::new(SECURITY)
        .args(["find-certificate", "-a", "-p", SYSTEM_KEYCHAIN])
        .output()
        .map_err(|source| crate::Error::Os {
            action: "run security to read the System keychain",
            source,
        })?;

    Ok(crate::trust::pem::decode_all(&output.stdout))
}
