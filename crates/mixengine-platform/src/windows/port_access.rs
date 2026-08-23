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
    /// Windows reserves nothing below 1024, so a program binds exactly what it answers on.
    fn bindings(&self, answering: &[u16]) -> Vec<PortBinding> {
        answering
            .iter()
            .map(|&answer| PortBinding {
                answer,
                bind: answer,
            })
            .collect()
    }

    fn probe(&self, _binary: &Path, answering: &[u16]) -> Result<PortAccessState> {
        Ok(PortAccessState {
            method: PortAccessMethod::Direct,
            bindings: self.bindings(answering),
            granted: true,
            missing: None,
        })
    }
}

/// There is nothing to grant on this system, and saying so is the whole implementation.
///
/// A caller reaching this is a daemon that read `PortAccessMethod::Direct` and asked anyway, or a
/// request document written on another machine. Either way, refusing by name is better than
/// succeeding at nothing.
///
/// # Errors
///
/// Always [`Error::UnsupportedPlatform`](crate::Error::UnsupportedPlatform).
#[cfg(feature = "elevated")]
pub(crate) fn apply(
    _plan: &mixengine_proto::privileged::PortAccessPlan,
) -> crate::Result<crate::port_access::Change> {
    Err(nothing_to_grant())
}

/// And nothing to take away.
///
/// # Errors
///
/// Always [`Error::UnsupportedPlatform`](crate::Error::UnsupportedPlatform).
#[cfg(feature = "elevated")]
pub(crate) fn revoke(
    _target: &mixengine_proto::privileged::PortAccessTarget,
) -> crate::Result<crate::port_access::Change> {
    Err(nothing_to_grant())
}

/// The one answer this system has.
#[cfg(feature = "elevated")]
fn nothing_to_grant() -> crate::Error {
    crate::Error::UnsupportedPlatform {
        capability: "PortAccess",
        reason: "Windows reserves no ports below 1024, so any process may bind 80 and 443 and \
                 there is nothing to grant; nothing was changed. A bind that fails here is an \
                 excluded port range (`netsh int ipv4 show excludedportrange`) rather than a \
                 permission"
            .to_owned(),
    }
}
