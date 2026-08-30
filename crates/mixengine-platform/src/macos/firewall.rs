//! macOS' half of T74, which is to do nothing and say so.
//!
//! The application firewall filters applications rather than ports, and in its default
//! configuration — the one nearly every machine is in — a socket that is already listening accepts
//! connections from the local network with no rule at all. There is nothing to write, so nothing is
//! written, and the answer says which of the two it is: not success, which would claim a change
//! that never happened, and not failure, which would stop a share that already works.
//!
//! **The packet filter is not the answer here either.** MixEngine does drive `pf` on macOS — that
//! is how 80 and 443 reach a front end bound to 8080 and 8443 (T42) — but that is a redirect within
//! the machine, not a permission for anyone outside it. Adding a `pass` rule would be adding one to
//! a firewall that is not blocking, and one more root-owned artifact to remove later.

use mixengine_proto::privileged::FirewallPlan;

use crate::firewall::{Applied, unix_tools};

/// Report that this machine needs no rule, whatever the plan asked for.
///
/// The plan is read for nothing, which is deliberate: a validated request that this system does not
/// act on is still a request the helper answers honestly, and the shape stays identical to the
/// other two so a caller has one path.
pub(crate) fn apply(_plan: &FirewallPlan) -> crate::Result<Applied> {
    let (reason, manual) = unix_tools::macos_unmanaged();

    Ok(Applied::Unmanaged { reason, manual })
}
