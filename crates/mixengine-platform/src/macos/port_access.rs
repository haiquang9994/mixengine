//! macOS: three files, compared against what the grant would write.
//!
//! **`/dev/pf` belongs to root**, so the daemon — which runs as the user — cannot ask the packet
//! filter whether it is enabled or what it has loaded. What it can read is the anchor, the block in
//! `/etc/pf.conf` and the boot job's plist, all of them world-readable. The probe is a byte
//! comparison against what a grant would put there — the T42 design, D9.
//!
//! **That proves the configuration is in place; it does not prove pf is running right now.** The
//! plist is what makes that true at every boot, and the plist's presence is what is checked here.
//! The honest end-to-end check is a request to `127.0.0.1:80` reaching this home's front end, which
//! needs a front end that serves something — T43's, and `mix doctor`'s (T47).

#[cfg(feature = "host")]
use std::path::Path;

#[cfg(feature = "host")]
use mixengine_proto::privileged::PortRedirect;

#[cfg(feature = "host")]
use crate::port_access::pf;
#[cfg(feature = "host")]
use crate::{PortAccess, PortAccessMethod, PortAccessState, PortBinding, Result};

/// This system's answer.
#[cfg(feature = "host")]
#[derive(Debug, Default)]
pub(crate) struct Ports;

#[cfg(feature = "host")]
impl PortAccess for Ports {
    fn probe(&self, _binary: &Path, answering: &[u16]) -> Result<PortAccessState> {
        let bindings: Vec<PortBinding> = answering
            .iter()
            .map(|&answer| PortBinding {
                answer,
                bind: target(answer),
            })
            .collect();

        let redirects: Vec<PortRedirect> = bindings
            .iter()
            .filter(|binding| binding.answer != binding.bind)
            .map(|binding| PortRedirect {
                answer: binding.answer,
                bind: binding.bind,
            })
            .collect();

        if redirects.is_empty() {
            return Ok(PortAccessState {
                method: PortAccessMethod::Redirect,
                bindings,
                granted: true,
                missing: None,
            });
        }

        let mut absent = Vec::new();

        if text(pf::ANCHOR_FILE).as_deref() != Some(pf::anchor(&redirects).as_str()) {
            absent.push(pf::ANCHOR_FILE);
        }

        if !text(pf::CONF_FILE).map_or(Ok(false), |conf| pf::is_declared(&conf))? {
            absent.push(pf::CONF_FILE);
        }

        if text(pf::PLIST_FILE).as_deref() != Some(pf::plist().as_str()) {
            absent.push(pf::PLIST_FILE);
        }

        Ok(PortAccessState {
            method: PortAccessMethod::Redirect,
            bindings,
            granted: absent.is_empty(),
            missing: (!absent.is_empty()).then(|| {
                format!(
                    "{} {} not what MixEngine's packet-filter redirect needs",
                    absent.join(", "),
                    if absent.len() == 1 { "is" } else { "are" }
                )
            }),
        })
    }
}

/// The ordinary port a program binds to answer `answer` — D2's table, fixed.
///
/// A port that is not reserved answers itself: nothing has to move, so nothing does.
#[cfg(feature = "host")]
fn target(answer: u16) -> u16 {
    match answer {
        80 => 8080,
        443 => 8443,
        other => other,
    }
}

/// The contents of a file that may not be there. Absent and unreadable are the same answer here:
/// the grant is not in place.
#[cfg(feature = "host")]
fn text(path: &str) -> Option<String> {
    std::fs::read_to_string(Path::new(path)).ok()
}
