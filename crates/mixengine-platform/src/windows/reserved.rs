//! Windows: `netsh`, parsed — roadmap task **T47a**.
//!
//! **A command and not an API**, deliberately. Windows exposes these ranges through `netsh`'s output
//! and through the registry's `ReservedPorts` value, and the two do not agree: the registry holds
//! what an administrator asked for, while `netsh` holds what the system has actually taken —
//! including the dynamic ranges Hyper-V and `winnat` claim at boot. The second is the one a failing
//! bind is about.

use crate::{Error, PortRange, ReservedPorts, Result};

/// This system's answer.
#[derive(Debug, Default)]
pub(crate) struct Reserved;

impl ReservedPorts for Reserved {
    fn reserved(&self) -> Result<Vec<PortRange>> {
        let output = std::process::Command::new("netsh")
            .args(["int", "ipv4", "show", "excludedportrange", "protocol=tcp"])
            .output()
            .map_err(|source| Error::Os {
                action: "ask netsh which port ranges this system has reserved",
                source,
            })?;

        Ok(crate::reserved::parse(&String::from_utf8_lossy(
            &output.stdout,
        )))
    }
}
