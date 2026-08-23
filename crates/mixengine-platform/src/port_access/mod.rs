//! Being allowed to answer on a port the operating system reserves — roadmap task **T42**.
//!
//! **Not [`crate::PortOwner`]**, which is about who got to a port first. This is about whether an
//! unprivileged program may bind one at all, and the three systems answer it three different ways:
//! Windows reserves nothing, Linux puts a capability on the binary, macOS redirects the packet
//! through its packet filter. See the T42 design, D2.
//!
//! Compiled under **both** `host` and `elevated`, for `crate::hosts`' reason: the daemon reads the
//! state and the helper writes it, and neither is worth a second implementation.

pub(crate) mod capability;
pub(crate) mod pf;

/// The lock that keeps two homes on one machine from interleaving their grants — as `hosts` does,
/// and in the same root-owned directory, because the artifacts are machine-wide and the state they
/// are generated from is per-home.
#[cfg(feature = "elevated")]
#[allow(
    dead_code,
    reason = "Windows refuses both directions before it would ever need a lock, so on that \n              system this is compiled and not called — the same shape as the tables in \n              `crate::prompt`"
)]
const LOCK: &str = "port-access.lock";

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

/// Grant what `plan` asks for, under the machine-wide lock.
///
/// **Whole state** — the T42 design, D4 — so a second call with the same plan is
/// [`Change::Unchanged`] and a superseded request is a replaced row rather than a second prompt.
///
/// # Errors
///
/// [`Error::UnsupportedPlatform`](crate::Error::UnsupportedPlatform) when the plan is not this
/// system's mechanism, [`Error::Io`](crate::Error::Io) when a file cannot be written, and
/// [`Error::Os`](crate::Error::Os) when the machine-wide lock is held by another helper.
#[cfg(feature = "elevated")]
pub fn apply(plan: &mixengine_proto::privileged::PortAccessPlan) -> crate::Result<Change> {
    crate::sys::port_access::apply(plan)
}

/// Take it away again.
///
/// **Does not disable the packet filter on macOS** — D3. By then there is no way to know who else
/// has come to depend on pf being up, and pf enabled with none of our rules in it is not observably
/// different from pf disabled.
///
/// # Errors
///
/// As [`apply`].
#[cfg(feature = "elevated")]
pub fn revoke(target: &mixengine_proto::privileged::PortAccessTarget) -> crate::Result<Change> {
    crate::sys::port_access::revoke(target)
}

/// The machine-wide lock, held across the read *and* the write: two helpers that both read before
/// either wrote would each apply their own home's state over the other's.
///
/// **Taken by each system's own `apply`, after it has decided the plan is its mechanism** — never
/// here. The lock lives in a root-owned directory, so taking it first would turn "this system does
/// not do that" into a permission error on the two machines the plan was not written for: `Refused`
/// becomes `Failed`, and a request that will never work starts reading as one worth retrying.
#[cfg(feature = "elevated")]
#[allow(
    dead_code,
    reason = "Windows refuses both directions before it would ever need a lock, so on that \n              system this is compiled and not called — the same shape as the tables in \n              `crate::prompt`"
)]
pub(crate) fn held() -> crate::Result<crate::lock::Lock> {
    let path = crate::elevated::audit_directory()?.join(LOCK);

    match crate::lock::Lock::acquire(&path)? {
        crate::lock::Acquired::Held(held) => Ok(held),
        crate::lock::Acquired::Taken(holder) => Err(crate::Error::Os {
            action: "take the machine-wide port-access lock",
            source: std::io::Error::new(
                std::io::ErrorKind::WouldBlock,
                format!("{holder} is already changing this machine's port access"),
            ),
        }),
    }
}
