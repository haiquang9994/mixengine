//! The mock's interfaces — whatever a test said, or one ordinary Wi-Fi.

use crate::{Interface, NetworkInfo, Result};

/// What this mock will answer.
#[derive(Debug)]
pub(crate) struct Network {
    /// The interfaces this machine pretends to have, loopback included.
    pub(crate) interfaces: Vec<Interface>,
}

impl Default for Network {
    /// **One shareable interface, not none.** The overwhelming majority of tests that touch sharing
    /// want a machine a site *can* be shared on, and a default of nothing would make every one of
    /// them arrange the same fixture. The two interesting machines — none, and more than one — are
    /// what [`Host::with_interfaces`](super::Host::with_interfaces) exists to arrange.
    fn default() -> Self {
        Self {
            interfaces: vec![
                Interface {
                    name: "lo".to_owned(),
                    address: std::net::Ipv4Addr::LOCALHOST,
                    loopback: true,
                },
                Interface {
                    name: "Wi-Fi".to_owned(),
                    address: std::net::Ipv4Addr::new(192, 168, 1, 10),
                    loopback: false,
                },
            ],
        }
    }
}

impl NetworkInfo for Network {
    /// **Never fails.** A machine that cannot be asked about its own interfaces is an OS error the
    /// real implementation reports; a mock that refused would leave the empty case — a laptop with
    /// the Wi-Fi off — untestable, and that is the case sharing has to refuse well.
    fn interfaces(&self) -> Result<Vec<Interface>> {
        Ok(self.interfaces.clone())
    }
}
