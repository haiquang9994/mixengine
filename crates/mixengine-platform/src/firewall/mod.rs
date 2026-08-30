//! Letting the local network reach a shared site's ports — roadmap task **T74**.
//!
//! Three systems, and only one of them writes a rule in the ordinary case. Windows has a firewall
//! that blocks an inbound connection by default and a precise, removable way to say otherwise;
//! Linux has one only where `ufw` or `firewalld` is running; macOS' application firewall asks about
//! *applications* rather than ports and, for a socket that is already listening, needs nothing at
//! all. Where there is nothing to write this answers [`Applied::Unmanaged`] with the command a
//! person would run — never success, which would tell a user their phone can reach a site the
//! machine is still refusing.
//!
//! **The argument lists are built in submodules here, pure and compiled everywhere**, exactly as
//! [`crate::resolver`] does it: that is what lets a developer on any one system test all three.
//! Only the writes live in `crate::sys::firewall`.
//!
//! Compiled under **both** `host` and `elevated` for [`crate::hosts`]' reason — except that here
//! only the helper ever calls it. The daemon never reads the rule set back; sharing state lives in
//! its own database, and a machine's rules are not a thing two homes could agree about.

#[allow(
    dead_code,
    reason = "called by Windows' writer only; compiled on all three so its tests run on all three"
)]
pub(crate) mod netsh;
#[allow(
    dead_code,
    reason = "called by Linux' writer only; compiled on all three so its tests run on all three"
)]
pub(crate) mod unix_tools;

/// What became of a firewall plan.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Applied {
    /// The machine changed, and this is what changed.
    Written {
        /// For the audit line and for the answer the CLI renders.
        detail: String,
    },

    /// The machine already allowed exactly this. Not a failure and not a change.
    Unchanged,

    /// This machine has no mechanism to apply it to.
    ///
    /// **An outcome, not an error** — the T74 design, D8. macOS in its ordinary configuration and a
    /// Linux running neither `ufw` nor `firewalld` both arrive here, and on both of them the site
    /// may well already be reachable. What a person needs is the sentence and the command, not a
    /// failed share.
    Unmanaged {
        /// Why nothing was done, phrased for a user.
        reason: String,

        /// What to run by hand if the port does turn out to be blocked.
        manual: String,
    },
}

/// Open exactly the ports `plan` names, under this system's own mechanism.
///
/// **Whole state** — the T74 design, D6 — so a second call with the same plan is
/// [`Applied::Unchanged`], and a plan naming no ports removes the rules rather than doing nothing.
///
/// # Errors
///
/// [`Error::Command`](crate::Error::Command) when the tool ran and refused, and
/// [`Error::Io`](crate::Error::Io) when it could not be run at all.
#[cfg(feature = "elevated")]
pub fn apply(plan: &mixengine_proto::privileged::FirewallPlan) -> crate::Result<Applied> {
    crate::sys::firewall::apply(plan)
}
