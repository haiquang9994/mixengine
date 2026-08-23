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

#[cfg(feature = "elevated")]
use mixengine_proto::privileged::{PortAccessPlan, PortAccessTarget};

#[cfg(any(feature = "host", feature = "elevated"))]
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

/// Write the three artifacts a redirect is made of — the T42 design, D3, and ADR 0012.
///
/// **The third is a boot job**, and it is what makes the other two mean anything: pf is disabled on
/// every boot and `pfctl -e` needs root, so a redirect that is only installed works until the first
/// reboot and then silently stops — leaving a front end answering on 8080 that nothing reaches on
/// 80.
///
/// # Errors
///
/// [`Error::UnsupportedPlatform`](crate::Error::UnsupportedPlatform) for a capability plan, which is
/// not this system's mechanism, [`Error::MalformedBlock`](crate::Error::MalformedBlock) for a
/// `/etc/pf.conf` somebody has half-edited, and [`Error::Io`](crate::Error::Io) when a file cannot
/// be read or replaced.
#[cfg(feature = "elevated")]
pub(crate) fn apply(plan: &PortAccessPlan) -> crate::Result<crate::port_access::Change> {
    let PortAccessPlan::Redirect { redirects } = plan else {
        return Err(unsupported(
            "macOS reserves ports below 1024 and has no per-file capability; a redirect through \
             the packet filter is what this system grants",
        ));
    };

    let _held = crate::port_access::held()?;
    let mut changed = Vec::new();

    // Before the declaration that loads it: a `load anchor` naming a file that is not there is a
    // `/etc/pf.conf` `pfctl` refuses, and this order means no moment exists where that is true.
    if put(pf::ANCHOR_FILE, &pf::anchor(redirects))? {
        changed.push(pf::ANCHOR_FILE);
    }

    let conf = whole(pf::CONF_FILE);
    let declared = pf::declared(&conf)?;

    if declared != conf {
        crate::sys::replace::atomically(std::path::Path::new(pf::CONF_FILE), &declared)?;
        changed.push(pf::CONF_FILE);
    }

    if put(pf::PLIST_FILE, &pf::plist())? {
        changed.push(pf::PLIST_FILE);
    }

    Ok(change(changed, "wrote"))
}

/// Remove all three.
///
/// **`pfctl -d` is deliberately not run.** By then there is no way to know who else has come to
/// depend on pf being up, and pf enabled with none of our rules in it is not observably different
/// from pf disabled.
///
/// # Errors
///
/// As [`apply`].
#[cfg(feature = "elevated")]
pub(crate) fn revoke(target: &PortAccessTarget) -> crate::Result<crate::port_access::Change> {
    let PortAccessTarget::Redirect {} = target else {
        return Err(unsupported(
            "macOS has no per-file capability to take back; what it grants is a packet-filter \
             redirect",
        ));
    };

    let _held = crate::port_access::held()?;
    let mut changed = Vec::new();

    // The declaration first, this time: the reverse order, for the same reason.
    let conf = whole(pf::CONF_FILE);
    let undeclared = pf::undeclared(&conf)?;

    if undeclared != conf {
        crate::sys::replace::atomically(std::path::Path::new(pf::CONF_FILE), &undeclared)?;
        changed.push(pf::CONF_FILE);
    }

    for path in [pf::ANCHOR_FILE, pf::PLIST_FILE] {
        if remove(path)? {
            changed.push(path);
        }
    }

    Ok(change(changed, "removed"))
}

/// Replace `path` with `contents` when it does not already say that. Answers whether it wrote.
#[cfg(feature = "elevated")]
fn put(path: &str, contents: &str) -> crate::Result<bool> {
    if whole(path) == contents {
        return Ok(false);
    }

    crate::sys::replace::atomically(std::path::Path::new(path), contents)?;

    Ok(true)
}

/// Delete `path` if it is there. Answers whether it removed anything.
#[cfg(feature = "elevated")]
fn remove(path: &str) -> crate::Result<bool> {
    match std::fs::remove_file(path) {
        Ok(()) => Ok(true),
        Err(source) if source.kind() == std::io::ErrorKind::NotFound => Ok(false),
        Err(source) => Err(crate::Error::Io {
            action: "remove",
            path: std::path::PathBuf::from(path),
            source,
        }),
    }
}

/// The contents of a file that may not be there, as the empty string when it is not.
///
/// A machine with no `/etc/pf.conf` is not one macOS ships, and is reachable — somebody who has
/// cleaned it up. It is created rather than refused, exactly as the hosts file is.
#[cfg(feature = "elevated")]
fn whole(path: &str) -> String {
    std::fs::read_to_string(path).unwrap_or_default()
}

/// [`Change`](crate::port_access::Change) from the list of paths that moved.
#[cfg(feature = "elevated")]
fn change(changed: Vec<&str>, verb: &str) -> crate::port_access::Change {
    if changed.is_empty() {
        return crate::port_access::Change::Unchanged;
    }

    crate::port_access::Change::Written {
        detail: format!("{verb} {}", changed.join(", ")),
    }
}

/// The refusal this system gives a plan that is not its mechanism.
#[cfg(feature = "elevated")]
fn unsupported(reason: &str) -> crate::Error {
    crate::Error::UnsupportedPlatform {
        capability: "PortAccess",
        reason: format!("{reason}; nothing was changed"),
    }
}
