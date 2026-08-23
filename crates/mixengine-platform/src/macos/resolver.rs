//! macOS: one marked file per TLD under `/etc/resolver`.
//!
//! The simplest of the three mechanisms and the only one that needed no correction from the
//! measurement: a file naming `127.0.0.1` and a port routes that TLD **immediately**, with no
//! `dscacheutil -flushcache` and no `killall -HUP mDNSResponder` — a name nothing had ever asked
//! for resolved the moment the file existed. Every other name on the machine is untouched, and the
//! wiring reads back to an ordinary user because the file is world-readable.
//!
//! **A file that is not ours is never replaced** — the T45 design, D5. `/etc/resolver/test` may
//! already belong to somebody's VPN or corporate configuration, and replacing it silently is the
//! failure T41's marker block exists to prevent. The refusal is the same shape: say which file, and
//! stop.
//!
//! One thing the measurement turned up that is worth knowing before reading a log: macOS also asks
//! this server for `_dns.resolver.arpa` type 64 (SVCB), which is Discovery of Designated Resolvers.
//! That name is outside every managed TLD, so T44's server answers `REFUSED`, no encrypted
//! transport is discovered, and nothing else happens.

#[cfg(feature = "host")]
use std::path::Path;

use crate::resolver::directory;

#[cfg(feature = "host")]
use crate::{ResolverConfig, ResolverMethod, ResolverState, Result};

/// This system's answer.
#[cfg(feature = "host")]
#[derive(Debug, Default)]
pub(crate) struct Resolver;

#[cfg(feature = "host")]
impl ResolverConfig for Resolver {
    /// Always a resolver directory. `/etc/resolver` is a documented part of macOS —
    /// `man 5 resolver` — and needs nothing installed or running for it to be read.
    fn method(&self) -> Result<ResolverMethod> {
        Ok(ResolverMethod::ResolverDirectory)
    }

    fn probe(&self, tlds: &[&str], port: u16) -> Result<ResolverState> {
        let wired = wired_under(Path::new(directory::DIRECTORY), tlds, port);

        let missing = (wired.len() < tlds.len()).then(|| {
            format!(
                "{} holds no MixEngine file sending these names to 127.0.0.1:{port}",
                directory::DIRECTORY
            )
        });

        Ok(ResolverState {
            method: ResolverMethod::ResolverDirectory,
            wired,
            missing,
        })
    }
}

/// Which of `tlds` `root` already routes to this home's server on `port`.
///
/// **Ours *and* this port.** A file we wrote for another home names another port, and counting it
/// would report a home as wired whose names all fail to resolve.
#[cfg(feature = "host")]
fn wired_under(root: &Path, tlds: &[&str], port: u16) -> Vec<String> {
    let wanted = directory::file_for(port);

    tlds.iter()
        .filter(|tld| {
            std::fs::read_to_string(root.join(tld))
                .is_ok_and(|contents| directory::is_ours(&contents) && contents == wanted)
        })
        .map(|tld| (*tld).to_owned())
        .collect()
}

/// Write one marked file per TLD in the plan, and remove the marked ones it does not name.
///
/// **Whole state** — the T45 design, D4 — so a plan that drops a TLD takes its file away, and a
/// plan identical to what is on disk is [`Change::Unchanged`](crate::resolver::Change::Unchanged).
///
/// # Errors
///
/// [`Error::UnsupportedPlatform`](crate::Error::UnsupportedPlatform) for another system's plan,
/// [`Error::MalformedBlock`](crate::Error::MalformedBlock) for a resolver file MixEngine did not
/// write, and [`Error::Io`](crate::Error::Io) when a file cannot be read, written or removed.
#[cfg(feature = "elevated")]
pub(crate) fn apply(
    plan: &mixengine_proto::privileged::ResolverPlan,
) -> crate::Result<crate::resolver::Change> {
    use mixengine_proto::privileged::ResolverPlan;

    let ResolverPlan::ResolverDirectory { tlds, port } = plan else {
        return Err(unsupported(
            "macOS routes a TLD with a file under /etc/resolver; this plan is another system's \
             mechanism",
        ));
    };

    // After the mechanism check and never before it — see `crate::resolver::held`.
    let _held = crate::resolver::held()?;

    let root = std::path::Path::new(directory::DIRECTORY);

    if !root.exists() {
        std::fs::create_dir_all(root).map_err(|source| crate::Error::Io {
            action: "create",
            path: root.to_path_buf(),
            source,
        })?;
    }

    let wanted = directory::file_for(*port);
    let mut changed = Vec::new();

    for tld in tlds {
        let path = directory::path_for(tld);

        match std::fs::read_to_string(&path) {
            // Already exactly this. Nothing to write, and nothing to report.
            Ok(present) if present == wanted => {}

            // D5: somebody else's configuration for a TLD we manage. Refused rather than replaced,
            // and the path is in the sentence because `MalformedBlock` carries only a reason.
            Ok(present) if !directory::is_ours(&present) => {
                return Err(crate::Error::MalformedBlock {
                    reason: format!(
                        "{} was not written by MixEngine; remove it by hand if this TLD should be \
                         routed to MixEngine's DNS server",
                        path.display()
                    ),
                });
            }

            // Ours, and wrong — another port, or an older format. Replaced.
            Ok(_) => {
                crate::sys::replace::atomically(&path, &wanted)?;
                changed.push(tld.clone());
            }

            // Not there at all, which is every machine before the first grant.
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
                crate::sys::replace::atomically(&path, &wanted)?;
                changed.push(tld.clone());
            }

            Err(source) => {
                return Err(crate::Error::Io {
                    action: "read",
                    path,
                    source,
                });
            }
        }
    }

    // Whole state's other half: a TLD this home no longer routes loses its file. Only ours are
    // touched, so a file somebody else put there for a TLD we stopped managing stays.
    for removed in sweep(root, tlds)? {
        changed.push(removed);
    }

    Ok(change(&changed, "wrote"))
}

/// Remove every resolver file MixEngine marked, and leave every other one.
///
/// # Errors
///
/// As [`apply`].
#[cfg(feature = "elevated")]
pub(crate) fn revoke(
    target: &mixengine_proto::privileged::ResolverTarget,
) -> crate::Result<crate::resolver::Change> {
    use mixengine_proto::privileged::ResolverTarget;

    let ResolverTarget::ResolverDirectory {} = target else {
        return Err(unsupported(
            "macOS routes a TLD with a file under /etc/resolver; this target is another system's \
             mechanism",
        ));
    };

    let _held = crate::resolver::held()?;

    let removed = sweep(std::path::Path::new(directory::DIRECTORY), &[])?;

    Ok(change(&removed, "removed"))
}

/// Remove every file under `root` that MixEngine marked and that `keep` does not name.
///
/// Reads the directory rather than the TLD table, so a file left by a build that managed a TLD this
/// one does not still goes — which is what makes uninstall complete rather than nearly complete.
#[cfg(feature = "elevated")]
fn sweep(root: &std::path::Path, keep: &[String]) -> crate::Result<Vec<String>> {
    let entries = match std::fs::read_dir(root) {
        Ok(entries) => entries,
        // No directory is nothing of ours to remove, which is the ordinary state of a machine that
        // has never run MixEngine.
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(Vec::new()),
        Err(source) => {
            return Err(crate::Error::Io {
                action: "read",
                path: root.to_path_buf(),
                source,
            });
        }
    };

    let mut removed = Vec::new();

    for entry in entries {
        let entry = entry.map_err(|source| crate::Error::Io {
            action: "read",
            path: root.to_path_buf(),
            source,
        })?;

        let path = entry.path();
        let Some(name) = path.file_name().and_then(|name| name.to_str()) else {
            continue;
        };

        if keep.iter().any(|tld| tld == name) {
            continue;
        }

        if !std::fs::read_to_string(&path).is_ok_and(|contents| directory::is_ours(&contents)) {
            continue;
        }

        std::fs::remove_file(&path).map_err(|source| crate::Error::Io {
            action: "remove",
            path: path.clone(),
            source,
        })?;

        removed.push(name.to_owned());
    }

    Ok(removed)
}

/// What a run of [`apply`] or [`revoke`] did, in one sentence.
#[cfg(feature = "elevated")]
fn change(tlds: &[String], verb: &str) -> crate::resolver::Change {
    if tlds.is_empty() {
        return crate::resolver::Change::Unchanged;
    }

    crate::resolver::Change::Written {
        detail: format!(
            "{verb} {} under {}",
            tlds.iter()
                .map(|tld| format!("/etc/resolver/{tld}"))
                .collect::<Vec<_>>()
                .join(", "),
            directory::DIRECTORY
        ),
    }
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

    #[test]
    fn a_directory_with_our_files_reports_them_wired() {
        let temp = tempfile::tempdir().expect("a temporary directory");
        std::fs::write(temp.path().join("test"), directory::file_for(53_535)).expect("the file");

        let wired = wired_under(temp.path(), &["test", "internal"], 53_535);

        assert_eq!(wired, vec!["test".to_owned()]);
    }

    /// A file for the right TLD on the wrong port belongs to another home's server, and counting it
    /// would report this home as wired while none of its names resolve.
    #[test]
    fn a_file_naming_another_port_is_not_wired() {
        let temp = tempfile::tempdir().expect("a temporary directory");
        std::fs::write(temp.path().join("test"), directory::file_for(60_000)).expect("the file");

        assert!(wired_under(temp.path(), &["test"], 53_535).is_empty());
    }

    /// D5. Somebody else's resolver file for a TLD we manage is not ours, is not counted, and — in
    /// `apply` — is never replaced.
    #[test]
    fn somebody_elses_file_is_not_ours() {
        let temp = tempfile::tempdir().expect("a temporary directory");
        std::fs::write(temp.path().join("test"), "nameserver 192.168.1.1\n").expect("the file");

        assert!(wired_under(temp.path(), &["test"], 53_535).is_empty());
    }

    /// A machine that has never run MixEngine has no directory at all, and that is not an error.
    #[test]
    fn a_directory_that_is_not_there_reports_nothing_wired() {
        let temp = tempfile::tempdir().expect("a temporary directory");

        assert!(wired_under(&temp.path().join("absent"), &["test"], 53_535).is_empty());
    }

    /// This system's mechanism is a property of the system, not of the machine.
    #[test]
    fn macos_always_routes_with_a_resolver_directory() {
        assert_eq!(
            Resolver.method().expect("a method"),
            ResolverMethod::ResolverDirectory
        );
    }
}
