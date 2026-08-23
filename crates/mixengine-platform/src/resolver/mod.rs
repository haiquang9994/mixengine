//! Routing a managed TLD to MixEngine's own DNS server — roadmap task **T45**.
//!
//! T44 built a server that answers `A 127.0.0.1` for every name under a managed TLD, and nothing
//! that sends it a query. This is the other half: three mechanisms, one per system, none of them
//! interchangeable — a file per TLD on macOS, a network link of our own on Linux, one registry rule
//! on Windows.
//!
//! **Each system's text or values are generated in a submodule here, pure and compiled
//! everywhere**, exactly as [`crate::port_access`] does it. That is what lets a developer on any one
//! of the three test all three; only the writes live in `crate::sys::resolver`.
//!
//! Compiled under **both** `host` and `elevated`, for [`crate::hosts`]' reason: the daemon reads the
//! state and the helper writes it, and neither is worth a second implementation.

// Every one of these has its consumer two commits away, in `crate::sys::resolver`, and until then
// only their own tests call them. `expect` rather than `allow` on purpose: the day the per-OS
// modules land, this attribute becomes an unfulfilled expectation and the compiler asks for it back
// — which `allow` would not. Under `cfg_attr(not(test))` because in a test build they *are* used,
// so the lint does not fire there and an unconditional `expect` would be unfulfilled at once.
#[cfg_attr(
    not(test),
    expect(
        dead_code,
        reason = "the per-OS writers that call these land in the next three commits"
    )
)]
pub(crate) mod directory;
#[cfg_attr(
    not(test),
    expect(
        dead_code,
        reason = "the per-OS writers that call these land in the next three commits"
    )
)]
pub(crate) mod networkd;
// No `expect` here any more: `crate::sys::resolver` on Windows is its consumer, and it landed with
// this module's own commit.
pub(crate) mod nrpt;

/// The lock that keeps two homes on one machine from interleaving their wiring — as `hosts` and
/// `port_access` do, and in the same root-owned directory, because the artifacts are machine-wide
/// while the state they are generated from is per-home.
#[cfg(feature = "elevated")]
const LOCK: &str = "resolver.lock";

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

/// Route what `plan` asks for, under the machine-wide lock.
///
/// **Whole state** — the T45 design, D4 — so a second call with the same plan is
/// [`Change::Unchanged`] and a superseded request is a replaced artifact rather than a second one.
///
/// # Errors
///
/// [`Error::UnsupportedPlatform`](crate::Error::UnsupportedPlatform) when the plan is not this
/// system's mechanism, [`Error::MalformedBlock`](crate::Error::MalformedBlock) when an artifact on
/// the machine was not written by MixEngine and would have to be overwritten,
/// [`Error::Io`](crate::Error::Io) when a file cannot be written, and
/// [`Error::Os`](crate::Error::Os) when the machine-wide lock is held by another helper.
#[cfg(feature = "elevated")]
pub fn apply(plan: &mixengine_proto::privileged::ResolverPlan) -> crate::Result<Change> {
    crate::sys::resolver::apply(plan)
}

/// Take it away again.
///
/// # Errors
///
/// As [`apply`].
#[cfg(feature = "elevated")]
pub fn revoke(target: &mixengine_proto::privileged::ResolverTarget) -> crate::Result<Change> {
    crate::sys::resolver::revoke(target)
}

/// The machine-wide lock, held across the read *and* the write: two helpers that both read before
/// either wrote would each apply their own home's state over the other's.
///
/// **Taken by each system's own `apply`, after it has decided the plan is its mechanism** — never
/// here. The lock lives in a root-owned directory, so taking it first would turn "this system does
/// not do that" into a permission error on the two machines the plan was not written for:
/// `Refused` becomes `Failed`, and a request that will never work starts reading as one worth
/// retrying. `port_access` established both the shape and the reason.
#[cfg(feature = "elevated")]
pub(crate) fn held() -> crate::Result<crate::lock::Lock> {
    let path = crate::elevated::audit_directory()?.join(LOCK);

    match crate::lock::Lock::acquire(&path)? {
        crate::lock::Acquired::Held(held) => Ok(held),
        crate::lock::Acquired::Taken(holder) => Err(crate::Error::Os {
            action: "take the machine-wide resolver lock",
            source: std::io::Error::new(
                std::io::ErrorKind::WouldBlock,
                format!("{holder} is already changing this machine's resolver"),
            ),
        }),
    }
}
