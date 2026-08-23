//! Windows: there is nothing to grant.
//!
//! Windows reserves no ports below 1024 — any process may bind 80 — so this system's whole answer is
//! that the question does not apply. It is written out rather than left to a default, because the
//! rule is that no branch quietly does nothing: a reader looking for what Windows does finds a file
//! saying so.
//!
//! What *can* stop a bind here is an excluded port range (`netsh int ipv4 show excludedportrange`),
//! which looks like a permission error and is not one. That is `mix doctor`'s (T47) and not a grant.

#[cfg(feature = "host")]
use std::path::Path;

#[cfg(feature = "host")]
use crate::{PortAccess, PortAccessMethod, PortAccessState, PortBinding, Result};

/// This system's answer.
#[cfg(feature = "host")]
#[derive(Debug, Default)]
pub(crate) struct Ports;

#[cfg(feature = "host")]
impl PortAccess for Ports {
    fn probe(&self, _binary: &Path, answering: &[u16]) -> Result<PortAccessState> {
        Ok(PortAccessState {
            method: PortAccessMethod::Direct,
            bindings: answering
                .iter()
                .map(|&answer| PortBinding {
                    answer,
                    bind: answer,
                })
                .collect(),
            granted: true,
            missing: None,
        })
    }
}
