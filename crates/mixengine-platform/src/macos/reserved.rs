//! macOS: no such concept — roadmap task **T47a**.

use crate::{Error, PortRange, ReservedPorts, Result};

/// This system's answer, which is that the question does not apply here.
#[derive(Debug, Default)]
pub(crate) struct Reserved;

impl ReservedPorts for Reserved {
    fn reserved(&self) -> Result<Vec<PortRange>> {
        Err(Error::UnsupportedPlatform {
            capability: "ReservedPorts",
            reason: "macOS reserves no port ranges; a bind here fails because something holds the \
                     port or because the port is privileged, and both of those are asked elsewhere"
                .to_owned(),
        })
    }
}
