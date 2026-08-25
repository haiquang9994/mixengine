//! What port access may be granted, decided by the binary that will grant it.
//!
//! **The helper validates the request itself rather than trusting the daemon** —
//! `.claude/architecture/security-model.md`, and the T42 design, D5. If the daemon is compromised it
//! *is* the attacker, so nothing the request asserts can be believed: not the binary, not the ports,
//! not the numbers a redirect names. This module is the whole of that decision, next door to
//! `hosts.rs`, which is the same shape for the same reason.
//!
//! **Ownership by the caller is the strongest assertion available.** The helper cannot be told where
//! `MIXENGINE_HOME` is, because the daemon is what would be telling it and the daemon is the thing
//! being guarded against — but the filesystem already knows who wrote the file, and the request's own
//! identity is established exactly that way.
//!
//! **The rule text is rendered here and never accepted.** A redirect carries two numbers; what goes
//! into a file root owns is built from them by `mixengine_platform::port_access`, so there is no
//! string in the request that could become a packet-filter rule of somebody else's choosing.
//!
//! What this cannot stop is a compromised daemon pointing the grant at a binary of its own choosing:
//! nothing here can tell that binary from a real front end. The control is T64 — the whole path is
//! printed before the user approves it, which is what that task was built for. See D11.

use std::path::Path;

use mixengine_platform::elevated::{self, Owner};
use mixengine_platform::port_access::{self, Change};
use mixengine_proto::privileged::{OpOutcome, PortAccessPlan, PortAccessTarget};

/// The only ports this helper will ever make reachable — the recorded allowlist the security model
/// requires. Not "anything below 1024": nothing MixEngine does needs 22.
const PERMITTED: [u16; 2] = [80, 443];

/// The first port an ordinary account may bind. A redirect whose target is itself reserved grants
/// nothing and moves the problem.
const FIRST_UNRESERVED: u16 = 1024;

/// Validate, apply, and say what happened.
pub(crate) fn grant(plan: &PortAccessPlan, caller: &Owner) -> OpOutcome {
    if let Some(reason) = refusal_to_grant(plan, caller) {
        return OpOutcome::Refused { reason };
    }

    outcome(port_access::apply(plan))
}

/// Validate, remove, and say what happened.
pub(crate) fn revoke(target: &PortAccessTarget, caller: &Owner) -> OpOutcome {
    if let Some(reason) = refusal_to_revoke(target, caller) {
        return OpOutcome::Refused { reason };
    }

    outcome(port_access::revoke(target))
}

/// One vocabulary for both directions.
fn outcome(result: mixengine_platform::Result<Change>) -> OpOutcome {
    match result {
        Ok(Change::Unchanged) => OpOutcome::AlreadyDone,
        Ok(Change::Written { detail }) => OpOutcome::Applied { detail },

        // What is wrong is on the machine or in the request, and a person has to look at it, so the
        // same request will be refused again — which is exactly what `Refused` says.
        Err(
            error @ (mixengine_platform::Error::UnsupportedPlatform { .. }
            | mixengine_platform::Error::MalformedBlock { .. }),
        ) => OpOutcome::Refused {
            reason: error.to_string(),
        },

        // A held lock, or an OS that said no. Nothing about the request is wrong.
        // **`flatten` and not `to_string`.** `Error::Os` renders as "cannot <action>" and keeps the
        // operating system's own words as its `#[source]`, so `to_string` alone hands back a
        // sentence with the cause cut off — which is the half a person needs. `mix` already
        // flattens the same errors at its own boundary; this is that boundary for the helper.
        Err(error) => OpOutcome::Failed {
            message: mixengine_proto::flatten(&error),
        },
    }
}

/// Why this grant will not be applied, or [`None`].
fn refusal_to_grant(plan: &PortAccessPlan, caller: &Owner) -> Option<String> {
    match plan {
        PortAccessPlan::Capability { binary, ports } => {
            if ports.is_empty() {
                return Some("a capability that names no port grants nothing".to_owned());
            }

            for port in ports {
                if !PERMITTED.contains(port) {
                    return Some(format!(
                        "port {port} is outside the ports this helper will grant (80, 443)"
                    ));
                }
            }

            refusal_about(binary, caller)
        }

        PortAccessPlan::Redirect { redirects } => {
            if redirects.is_empty() {
                return Some("a redirect that redirects nothing changes nothing".to_owned());
            }

            let mut seen: Vec<u16> = Vec::new();

            for redirect in redirects {
                if !PERMITTED.contains(&redirect.answer) {
                    return Some(format!(
                        "port {} is outside the ports this helper will redirect (80, 443)",
                        redirect.answer
                    ));
                }

                if redirect.bind < FIRST_UNRESERVED {
                    return Some(format!(
                        "{} is itself a reserved port; redirecting one reserved port to another \
                         grants nothing",
                        redirect.bind
                    ));
                }

                if seen.contains(&redirect.answer) {
                    return Some(format!("port {} is redirected twice", redirect.answer));
                }

                seen.push(redirect.answer);
            }

            None
        }
    }
}

/// Why this removal will not be applied, or [`None`].
///
/// There is no port to bound: clearing the attribute clears all of it. And a redirect's three paths
/// are constants in this binary rather than anything the request may choose, so `Redirect {}` has
/// nothing to check — which is the point of it carrying nothing.
fn refusal_to_revoke(target: &PortAccessTarget, caller: &Owner) -> Option<String> {
    match target {
        PortAccessTarget::Capability { binary } => refusal_about(binary, caller),
        PortAccessTarget::Redirect {} => None,
    }
}

/// The three checks a binary gets, in both directions.
///
/// The same two `request.rs` already applies to the request document, through the same two
/// functions, plus the symlink refusal it makes for the same reason: a symlink is somebody else
/// choosing which file root touches, after root has decided to trust the name.
fn refusal_about(binary: &Path, caller: &Owner) -> Option<String> {
    let metadata = match std::fs::symlink_metadata(binary) {
        Ok(metadata) => metadata,
        Err(source) => return Some(format!("cannot read {}: {source}", binary.display())),
    };

    if metadata.file_type().is_symlink() {
        return Some(format!("{} is a symlink", binary.display()));
    }

    if !metadata.is_file() {
        return Some(format!("{} is not a regular file", binary.display()));
    }

    match elevated::owner_of(binary) {
        Ok(owner) if &owner != caller => {
            return Some(format!(
                "{} belongs to {owner} and not to {caller}, who is asking",
                binary.display()
            ));
        }
        Ok(_) => {}
        Err(error) => return Some(error.to_string()),
    }

    match elevated::others_can_write(binary) {
        Ok(true) => Some(format!(
            "{} can be written by somebody other than {caller}",
            binary.display()
        )),
        Ok(false) => None,
        Err(error) => Some(error.to_string()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    use std::path::PathBuf;

    use mixengine_proto::privileged::PortRedirect;

    /// A file this test owns, and the identity the filesystem gives it — which is the only identity
    /// this binary ever believes.
    fn a_binary() -> (tempfile::TempDir, PathBuf, Owner) {
        let directory = tempfile::tempdir().expect("a temporary directory");
        let binary = directory.path().join("front-end");
        std::fs::write(&binary, b"not really a program").expect("the file");

        let owner = mixengine_platform::elevated::owner_of(&binary).expect("its owner");

        (directory, binary, owner)
    }

    /// The whole of the policy for a capability, row by row. Each line is something a compromised
    /// daemon would ask for and this binary is the last thing between it and the kernel.
    #[test]
    fn what_this_helper_will_not_grant_a_capability_for() {
        let (directory, binary, caller) = a_binary();

        let plan = |ports: Vec<u16>| PortAccessPlan::Capability {
            binary: binary.clone(),
            ports,
        };

        for (bad, what) in [
            (plan(vec![]), "no port"),
            (plan(vec![22]), "22"),
            (plan(vec![80, 8080]), "8080"),
            (plan(vec![0]), "0"),
        ] {
            let reason =
                refusal_to_grant(&bad, &caller).unwrap_or_else(|| panic!("{bad:?} was accepted"));

            assert!(reason.contains(what), "{reason}");
        }

        let missing = PortAccessPlan::Capability {
            binary: directory.path().join("not-there"),
            ports: vec![80],
        };
        assert!(refusal_to_grant(&missing, &caller).is_some());
    }

    /// A symlink is somebody else choosing which file root writes to, after root has decided to
    /// trust the name. The same refusal `request.rs` already makes about the request document.
    #[cfg(unix)]
    #[test]
    fn a_binary_that_is_a_symlink_or_a_directory_is_refused() {
        let (directory, binary, caller) = a_binary();

        let link = directory.path().join("link");
        std::os::unix::fs::symlink(&binary, &link).expect("a symlink");

        let plan = |path: PathBuf| PortAccessPlan::Capability {
            binary: path,
            ports: vec![80],
        };

        assert!(
            refusal_to_grant(&plan(link), &caller)
                .expect("refused")
                .contains("symlink")
        );
        assert!(
            refusal_to_grant(&plan(directory.path().to_path_buf()), &caller)
                .expect("refused")
                .contains("regular file")
        );
    }

    /// A file anybody may rewrite is a file whose bytes root approved and somebody else replaced —
    /// except that the kernel clears the capability on any write, which is what bounds D11 and why
    /// this is a refusal rather than the whole story.
    #[cfg(unix)]
    #[test]
    fn a_binary_anybody_can_rewrite_is_refused() {
        use std::os::unix::fs::PermissionsExt as _;

        let (_directory, binary, caller) = a_binary();
        std::fs::set_permissions(&binary, std::fs::Permissions::from_mode(0o666)).unwrap();

        let plan = PortAccessPlan::Capability {
            binary: binary.clone(),
            ports: vec![80],
        };

        assert!(
            refusal_to_grant(&plan, &caller)
                .expect("refused")
                .contains("written by somebody other than")
        );
    }

    /// The other plan's policy. The helper renders the rule text itself from these numbers and
    /// **never accepts text** — D5 — so these three are the whole of what a redirect may say.
    #[test]
    fn what_this_helper_will_not_redirect() {
        let (_directory, _binary, caller) = a_binary();

        let plan = |redirects: Vec<PortRedirect>| PortAccessPlan::Redirect { redirects };
        let one = |answer, bind| PortRedirect { answer, bind };

        for (bad, what) in [
            (plan(vec![]), "nothing"),
            (plan(vec![one(8080, 9090)]), "8080"),
            (plan(vec![one(80, 443)]), "443"),
            (plan(vec![one(80, 8080), one(80, 8081)]), "twice"),
        ] {
            let reason =
                refusal_to_grant(&bad, &caller).unwrap_or_else(|| panic!("{bad:?} was accepted"));

            assert!(reason.contains(what), "{reason}");
        }
    }

    /// A refusal test that never says yes proves only that the code refuses.
    #[test]
    fn what_this_helper_will_grant() {
        let (_directory, binary, caller) = a_binary();

        assert_eq!(
            refusal_to_grant(
                &PortAccessPlan::Capability {
                    binary: binary.clone(),
                    ports: vec![80, 443]
                },
                &caller
            ),
            None
        );

        assert_eq!(
            refusal_to_grant(
                &PortAccessPlan::Redirect {
                    redirects: vec![
                        PortRedirect {
                            answer: 80,
                            bind: 8080
                        },
                        PortRedirect {
                            answer: 443,
                            bind: 8443
                        },
                    ],
                },
                &caller
            ),
            None
        );

        assert_eq!(
            refusal_to_revoke(
                &PortAccessTarget::Capability {
                    binary: binary.clone()
                },
                &caller
            ),
            None
        );
        assert_eq!(
            refusal_to_revoke(&PortAccessTarget::Redirect {}, &caller),
            None
        );
    }

    /// Revoking runs the same three checks on the binary and no port check, because clearing the
    /// attribute clears all of it — there is nothing to bound.
    #[cfg(unix)]
    #[test]
    fn revoking_checks_the_binary_the_same_way() {
        use std::os::unix::fs::PermissionsExt as _;

        let (_directory, binary, caller) = a_binary();
        std::fs::set_permissions(&binary, std::fs::Permissions::from_mode(0o666)).unwrap();

        assert!(
            refusal_to_revoke(&PortAccessTarget::Capability { binary }, &caller).is_some(),
            "a file anybody can rewrite is not one to take a capability off blindly"
        );
    }
}
