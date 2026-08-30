//! What interfaces this machine has, on all three systems at once.
//!
//! **One implementation and not three.** Every other capability in this crate has a per-OS module
//! because the three systems answer through different mechanisms — a registry key, a resolver
//! directory, a `systemd` link. Enumerating interfaces is the exception: `getifaddrs` on the two
//! Unixes and `GetAdaptersAddresses` on Windows already sit behind one crate, and splitting a
//! single call three ways would leave three copies of the same filter to keep in step.

use crate::{Error, Interface, NetworkInfo, Result};

/// This machine's own interfaces.
#[derive(Debug, Default)]
pub(crate) struct Network;

impl NetworkInfo for Network {
    /// **IPv4 only, and up only** — the T74 design, D4. `if-addrs` reports an interface once per
    /// address it holds, so one adapter with a v4 and a v6 address arrives twice and only the v4
    /// row survives; an adapter that is down carries no address and never appears at all.
    fn interfaces(&self) -> Result<Vec<Interface>> {
        let found = if_addrs::get_if_addrs().map_err(|source| Error::Os {
            action: "enumerate this machine's network interfaces",
            source,
        })?;

        Ok(found
            .into_iter()
            .filter_map(|found| match found.addr {
                if_addrs::IfAddr::V4(v4) => Some(Interface {
                    loopback: v4.ip.is_loopback(),
                    address: v4.ip,
                    name: found.name,
                }),
                if_addrs::IfAddr::V6(_) => None,
            })
            .collect())
    }
}
