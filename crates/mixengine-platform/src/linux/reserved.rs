//! Linux: no such concept — roadmap task **T47a**.

use crate::{Error, PortRange, ReservedPorts, Result};

/// This system's answer, which is that the question does not apply here.
#[derive(Debug, Default)]
pub(crate) struct Reserved;

impl ReservedPorts for Reserved {
    fn reserved(&self) -> Result<Vec<PortRange>> {
        Err(Error::UnsupportedPlatform {
            capability: "ReservedPorts",
            reason: "Linux reserves nothing outside net.ipv4.ip_local_reserved_ports, which is \
                     empty on every ordinary machine and is not what a failing bind here is about"
                .to_owned(),
        })
    }
}
