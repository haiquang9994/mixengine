//! Linux: a `systemd-networkd` link of MixEngine's own.
//!
//! **This is the mechanism left after the measurement removed every one the feature spec and the
//! roadmap named** — the T45 design, D10:
//!
//! - a `/etc/systemd/resolved.conf.d/` drop-in with `DNS=127.0.0.1:53535` and `Domains=~test`
//!   redirects the **whole machine**: after it, `getent hosts github.com` answered `127.0.0.1`. A
//!   global routing domain does not scope the global DNS servers;
//! - `resolvectl dns lo …` is refused by name — *"Link lo is loopback device"* — and so is
//!   `resolvectl domain lo` and `resolvectl revert lo`;
//! - a real link would have its own servers **replaced** rather than added to, which is what
//!   `resolvectl dns <link>` does and what the roadmap already warned about;
//! - and a dummy link with **no address** is accepted, reports its servers back through
//!   `resolvectl status`, and never gets a DNS scope at all — so nothing is ever sent to it. That
//!   is the worst of the four, because everything about it reads as applied.
//!
//! So: a dummy link carrying a link-local `/32`, declared in two files so it survives a reboot with
//! no standing process of ours.
//!
//! **And unlike the other two systems, the mechanism here is a question about the machine rather
//! than about the platform** — D2. A Linux with no `systemd-resolved` and no `systemd-networkd` has
//! no scoped mechanism, which is [`ResolverMethod::None`] and a mode rather than a failure.

use crate::resolver::networkd;

#[cfg(feature = "host")]
use crate::{ResolverConfig, ResolverMethod, ResolverState, Result};

/// Where `systemd-resolved` puts its runtime state. Its presence is how this module asks whether
/// the service is running without starting a process to find out.
#[cfg(feature = "host")]
const RESOLVED_RUNTIME: &str = "/run/systemd/resolve";

/// The same question for `systemd-networkd`.
#[cfg(feature = "host")]
const NETWORKD_RUNTIME: &str = "/run/systemd/netif";

/// This system's answer.
#[cfg(feature = "host")]
#[derive(Debug, Default)]
pub(crate) struct Resolver;

#[cfg(feature = "host")]
impl ResolverConfig for Resolver {
    fn method(&self) -> Result<ResolverMethod> {
        Ok(mechanism(
            std::path::Path::new(RESOLVED_RUNTIME).exists(),
            std::path::Path::new(NETWORKD_RUNTIME).exists(),
        ))
    }

    fn probe(&self, tlds: &[&str], port: u16) -> Result<ResolverState> {
        let method = self.method()?;

        if method == ResolverMethod::None {
            return Ok(ResolverState {
                method,
                wired: Vec::new(),
                missing: Some(
                    "this machine runs neither systemd-resolved nor systemd-networkd, so there is \
                     no way to send one TLD to a nameserver without changing every name it \
                     resolves"
                        .to_owned(),
                ),
            });
        }

        let declared = std::fs::read_to_string(networkd::NETWORK_PATH).unwrap_or_default();
        let wired = wired_in(&declared, tlds, port);

        let missing = (wired.len() < tlds.len()).then(|| {
            format!(
                "{} does not send these names to 127.0.0.1:{port}",
                networkd::NETWORK_PATH
            )
        });

        Ok(ResolverState {
            method,
            wired,
            missing,
        })
    }
}

/// Which mechanism a machine with these two services has.
///
/// **Both, or neither.** `systemd-resolved` is what routes a domain to a nameserver and
/// `systemd-networkd` is what brings up the link the routing is attached to; a machine with one of
/// them has no way to do this that does not change every name it resolves.
#[cfg(feature = "host")]
fn mechanism(resolved: bool, networkd_running: bool) -> ResolverMethod {
    if resolved && networkd_running {
        ResolverMethod::SystemdLink
    } else {
        ResolverMethod::None
    }
}

/// Which of `tlds` a `.network` file routes to this home's server on `port`.
///
/// **The port has to match.** A file another home wrote names another port, and counting it would
/// report this home as wired while none of its names resolve.
#[cfg(feature = "host")]
fn wired_in(contents: &str, tlds: &[&str], port: u16) -> Vec<String> {
    let (domains, declared) = networkd::declared(contents);

    if declared != Some(port) {
        return Vec::new();
    }

    tlds.iter()
        .filter(|tld| domains.iter().any(|domain| domain == *tld))
        .map(|tld| (*tld).to_owned())
        .collect()
}

/// Declare the link and its routing domains, then ask `systemd-networkd` to bring it up.
///
/// # Errors
///
/// [`Error::UnsupportedPlatform`](crate::Error::UnsupportedPlatform) for another system's plan,
/// [`Error::Io`](crate::Error::Io) when a file cannot be written, and
/// [`Error::Os`](crate::Error::Os) when the reload could not be run or refused.
#[cfg(feature = "elevated")]
pub(crate) fn apply(
    plan: &mixengine_proto::privileged::ResolverPlan,
) -> crate::Result<crate::resolver::Change> {
    use mixengine_proto::privileged::ResolverPlan;

    let ResolverPlan::SystemdLink { tlds, port } = plan else {
        return Err(unsupported(
            "Linux routes a TLD with a systemd-networkd link of MixEngine's own; this plan is \
             another system's mechanism",
        ));
    };

    // After the mechanism check and never before it — see `crate::resolver::held`.
    let _held = crate::resolver::held()?;

    let netdev = networkd::netdev();
    let network = networkd::network(tlds, *port);

    let mut changed = false;
    changed |= put(networkd::NETDEV_PATH, &netdev)?;
    changed |= put(networkd::NETWORK_PATH, &network)?;

    if !changed {
        return Ok(crate::resolver::Change::Unchanged);
    }

    reload()?;

    Ok(crate::resolver::Change::Written {
        detail: format!(
            "declared the {} link in {} and {}, sending {} to 127.0.0.1:{port}",
            networkd::LINK,
            networkd::NETDEV_PATH,
            networkd::NETWORK_PATH,
            tlds.join(", ")
        ),
    })
}

/// Take both files away and bring the link down with them.
///
/// # Errors
///
/// As [`apply`].
#[cfg(feature = "elevated")]
pub(crate) fn revoke(
    target: &mixengine_proto::privileged::ResolverTarget,
) -> crate::Result<crate::resolver::Change> {
    use mixengine_proto::privileged::ResolverTarget;

    let ResolverTarget::SystemdLink {} = target else {
        return Err(unsupported(
            "Linux routes a TLD with a systemd-networkd link of MixEngine's own; this target is \
             another system's mechanism",
        ));
    };

    let _held = crate::resolver::held()?;

    let mut changed = false;
    changed |= remove(networkd::NETDEV_PATH)?;
    changed |= remove(networkd::NETWORK_PATH)?;

    if !changed {
        return Ok(crate::resolver::Change::Unchanged);
    }

    reload()?;

    Ok(crate::resolver::Change::Written {
        detail: format!(
            "removed the {} link and its two files under /etc/systemd/network",
            networkd::LINK
        ),
    })
}

/// Write `contents` to `path` unless it already says exactly that. Answers whether it wrote.
#[cfg(feature = "elevated")]
fn put(path: &str, contents: &str) -> crate::Result<bool> {
    let path = std::path::Path::new(path);

    if std::fs::read_to_string(path).ok().as_deref() == Some(contents) {
        return Ok(false);
    }

    crate::sys::replace::atomically(path, contents)?;

    Ok(true)
}

/// Remove `path` if it is there. Answers whether it removed anything.
#[cfg(feature = "elevated")]
fn remove(path: &str) -> crate::Result<bool> {
    let path = std::path::Path::new(path);

    match std::fs::remove_file(path) {
        Ok(()) => Ok(true),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(false),
        Err(source) => Err(crate::Error::Io {
            action: "remove",
            path: path.to_path_buf(),
            source,
        }),
    }
}

/// Ask `systemd-networkd` to read the files that were just written.
///
/// **One fixed command with no argument from the request** — the shape T42 established with
/// `pfctl`. The rule this binary keeps is that it never runs an *arbitrary* command; a constant
/// argument vector is not one, and there is no API for this that does not go through systemd's own
/// tool.
///
/// `networkctl reload` would be lighter and **was not measured**, so it is not used: what the
/// probe demonstrated bringing the link up was a restart, and a mechanism nobody has watched work
/// is not one to ship on the strength of its documentation.
#[cfg(feature = "elevated")]
fn reload() -> crate::Result<()> {
    const RELOAD: [&str; 3] = ["systemctl", "restart", "systemd-networkd"];

    let output = std::process::Command::new(RELOAD[0])
        .args(&RELOAD[1..])
        .output()
        .map_err(|source| crate::Error::Os {
            action: "run systemctl to reload systemd-networkd",
            source,
        })?;

    if !output.status.success() {
        return Err(crate::Error::Os {
            action: "reload systemd-networkd",
            source: std::io::Error::other(
                String::from_utf8_lossy(&output.stderr).trim().to_owned(),
            ),
        });
    }

    Ok(())
}

/// A plan or target that is not this system's mechanism.
#[cfg(feature = "elevated")]
fn unsupported(reason: &str) -> crate::Error {
    crate::Error::UnsupportedPlatform {
        capability: "ResolverConfig",
        reason: reason.to_owned(),
    }
}

#[cfg(all(test, feature = "host"))]
mod tests {
    use super::*;

    /// D2, and the whole reason [`ResolverConfig::method`] returns a `Result` here where the other
    /// two systems return a constant: on this system the mechanism depends on what is running, and
    /// a machine with neither service is a mode rather than a failure.
    #[test]
    fn a_machine_without_both_services_has_no_mechanism() {
        assert_eq!(mechanism(false, false), ResolverMethod::None);
        assert_eq!(mechanism(true, false), ResolverMethod::None);
        assert_eq!(mechanism(false, true), ResolverMethod::None);
        assert_eq!(mechanism(true, true), ResolverMethod::SystemdLink);
    }

    /// The probe reads the file back, so a wiring that drifted is not reported as present.
    #[test]
    fn a_file_that_matches_reports_its_tlds_wired() {
        let contents = networkd::network(&["test".to_owned()], 53_535);

        assert_eq!(
            wired_in(&contents, &["test", "internal"], 53_535),
            vec!["test".to_owned()]
        );
    }

    /// A file naming another port belongs to another home's server.
    #[test]
    fn a_file_naming_another_port_is_not_wired() {
        let contents = networkd::network(&["test".to_owned()], 60_000);

        assert!(wired_in(&contents, &["test"], 53_535).is_empty());
    }

    /// No file at all is every machine before the first grant, and is not an error.
    #[test]
    fn nothing_declared_is_nothing_wired() {
        assert!(wired_in("", &["test"], 53_535).is_empty());
    }
}
