//! Whether this machine trusts MixEngine's own certificate authority — roadmap task **T49a**.
//!
//! T48 generated an authority and nothing that asks any operating system to believe it. This is the
//! other half: three mechanisms, one per system, none of them interchangeable — a certificate store
//! on Windows, the System keychain on macOS, and a file plus a refresh command on Linux, where which
//! file it is depends on the distribution family rather than on the platform.
//!
//! **The check is written here, pure and compiled everywhere**, exactly as [`crate::resolver`] and
//! [`crate::port_access`] are: that is what lets a developer on any one of the three test the check
//! for all three. Only the reads and the writes live in `crate::sys::trust`.
//!
//! Compiled under **both** `host` and `elevated`, for [`crate::hosts`]' reason: the daemon reads
//! whether the machine already trusts an authority and the helper is what makes it, and neither is
//! worth a second implementation.

mod check;
mod der;
// The envelope the two file-based stores are written in. Unix only, so a Windows build of the
// helper does not gain the crate — see the module header.
#[cfg(unix)]
pub(crate) mod pem;

pub use check::{Authority, MAX_DER, Refused, is_key_id, ours, subject_of};

/// The lock that keeps two homes on one machine from interleaving their installs — as `hosts`,
/// `port_access` and `resolver` do, and in the same root-owned directory, because the store is
/// machine-wide while the certificate is per-home.
#[cfg(feature = "elevated")]
const LOCK: &str = "trust.lock";

/// What one [`apply`] or [`revoke`] did.
#[cfg(feature = "elevated")]
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Change {
    /// The machine changed, and this is what changed.
    Written {
        /// For the audit line and for `mix doctor`.
        detail: String,
    },

    /// The machine already said exactly this. Not a failure and not a change.
    Unchanged,
}

/// Make this machine trust the certificate `plan` carries, under the machine-wide lock.
///
/// **The shape check is the caller's, not this function's** — `mixengine-elevate` runs [`ours`]
/// before a store is opened at all, so that a refusal costs no privilege and no lock. What is
/// checked here is only that the plan names this system's mechanism.
///
/// # Errors
///
/// [`Error::UnsupportedPlatform`](crate::Error::UnsupportedPlatform) when the plan is not this
/// system's mechanism, [`Error::Io`](crate::Error::Io) when a file cannot be written, and
/// [`Error::Os`](crate::Error::Os) when a store refuses or the machine-wide lock is held.
#[cfg(feature = "elevated")]
pub fn apply(plan: &mixengine_proto::privileged::TrustPlan) -> crate::Result<Change> {
    crate::sys::trust::apply(plan)
}

/// Take it back out again.
///
/// Removes every certificate under this authority's subject **that also passes [`ours`]** — D5's
/// second check, because a certificate sitting under a MixEngine-shaped name is not proof that
/// MixEngine put it there.
///
/// # Errors
///
/// As [`apply`].
#[cfg(feature = "elevated")]
pub fn revoke(target: &mixengine_proto::privileged::TrustTarget) -> crate::Result<Change> {
    crate::sys::trust::revoke(target)
}

/// The machine-wide lock, held across the read *and* the write.
///
/// **Taken by each system's own writer, after it has decided the plan is its mechanism** — never
/// here, on `resolver`'s stated reason: the lock lives in a root-owned directory, so taking it first
/// would turn "this system does not do that" into a permission error on the two machines the plan
/// was not written for, and a request that will never work would start reading as one worth
/// retrying.
#[cfg(feature = "elevated")]
pub(crate) fn held() -> crate::Result<crate::lock::Lock> {
    let path = crate::elevated::audit_directory()?.join(LOCK);

    match crate::lock::Lock::acquire(&path)? {
        crate::lock::Acquired::Held(held) => Ok(held),
        crate::lock::Acquired::Taken(holder) => Err(crate::Error::Os {
            action: "take the machine-wide trust store lock",
            source: std::io::Error::new(
                std::io::ErrorKind::WouldBlock,
                format!("{holder} is already changing this machine's trusted roots"),
            ),
        }),
    }
}

/// A plan or target that is not this system's mechanism.
#[cfg(feature = "elevated")]
pub(crate) fn unsupported(reason: &str) -> crate::Error {
    crate::Error::UnsupportedPlatform {
        capability: "TrustStore",
        reason: reason.to_owned(),
    }
}
