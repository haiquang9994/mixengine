//! Deciding one operation: is it one this build knows, may it run under this token, and what
//! happened.
//!
//! **The elevation gate is here and nowhere else.** One `if`, applied to every operation, answered by
//! the operation itself — which is what keeps it findable by somebody auditing this binary. The
//! alternative frame, refusing to do anything at all when the process is not elevated, is what
//! `Probe` rules out.

use mixengine_platform::elevated::Owner;
use mixengine_proto::privileged::{OpOutcome, PrivilegedOp};

/// Decode one element of the batch, or say why it could not be.
///
/// One at a time, and never the whole `Vec`: a `Vec<PrivilegedOp>` fails as a whole when it meets one
/// variant this build has never heard of, which — this binary being excluded from auto-update — is a
/// routine event rather than a corruption. Element by element, an unknown operation becomes an
/// outcome **at its own index** and its neighbours are applied.
pub(crate) fn decode(value: &serde_json::Value) -> Result<PrivilegedOp, OpOutcome> {
    serde_json::from_value::<PrivilegedOp>(value.clone()).map_err(|error| OpOutcome::Unsupported {
        reason: error.to_string(),
    })
}

/// Carry out one decoded operation.
///
/// `caller` is whoever wrote the request file, established by the filesystem and not by the
/// document — see `crate::request`. Two operations need it: both directions of port access check
/// that the binary they are handed belongs to the same account.
///
/// `home` is `MIXENGINE_HOME` as the filesystem spells it, already established by `crate::request`
/// to be the caller's own directory and to contain the request. One operation needs it:
/// [`PrivilegedOp::HelperReplace`] composes the candidate's path from it and a compiled-in name,
/// which is how that operation carries no field for a caller to aim.
pub(crate) fn apply(
    op: &PrivilegedOp,
    elevated: bool,
    caller: &Owner,
    home: &std::path::Path,
) -> OpOutcome {
    if op.requires_elevation() && !elevated {
        // The first operation to reach this branch arrives with T41; `Probe` never does, by design.
        return OpOutcome::Refused {
            reason: format!(
                "{} needs an administrative token and this process does not have one",
                op.name()
            ),
        };
    }

    match op {
        // Applies nothing. What it reports travels in the response's header, on every answer — see
        // `mixengine_proto::privileged::PrivilegedResponse`.
        PrivilegedOp::Probe {} => OpOutcome::Applied {
            detail: "reported this build, its token and its audit log".to_owned(),
        },

        // The first operation with an effect. What it may write is decided next door, in forty lines
        // with nothing else in them — the T41 design, D3.
        PrivilegedOp::HostsApply { entries } => crate::hosts::apply(entries),

        // Roadmap task T42. What may be granted is decided next door, in one module with nothing
        // else in it — the T42 design, D5, on `hosts.rs`' pattern.
        PrivilegedOp::PortAccessGrant { plan } => crate::port_access::grant(plan, caller),
        PrivilegedOp::PortAccessRevoke { target } => crate::port_access::revoke(target, caller),

        // Roadmap task T45. What may be routed where is decided next door, in one module with
        // nothing else in it — the T45 design, D5, on `hosts.rs`' pattern.
        PrivilegedOp::ResolverApply { plan } => crate::resolver::wire(plan),
        PrivilegedOp::ResolverRevoke { target } => crate::resolver::unwire(target),

        // Roadmap task T49a. What may be installed, and what may be removed, is decided next door —
        // the T49a design, D4 and D5, on `hosts.rs`' pattern.
        PrivilegedOp::TrustCaInstall { plan } => crate::trust::install(plan),
        PrivilegedOp::TrustCaRemove { target } => crate::trust::remove(target),

        // Roadmap task T74. What ports may be opened is decided next door, on `hosts.rs`' pattern
        // — and with the one difference that shapes that module: this is the first operation whose
        // central question cannot be answered from a table compiled in here, so it refuses what is
        // provably not a web port instead of accepting what is provably one.
        PrivilegedOp::FirewallApply { plan } => crate::firewall::apply(plan),

        // Roadmap task T85, and the only operation whose source and destination are both this
        // binary's own business — see `crate::helper` for why it carries no field to aim.
        PrivilegedOp::HelperInstall {} => crate::helper::install(),

        // Roadmap task T88a, and the only operation whose decision is a signature rather than a
        // shape — see `crate::candidate` for why that check is made here and not in the daemon.
        PrivilegedOp::HelperReplace {} => crate::helper::replace(home),

        // Roadmap task T87, and the two operations whose target is this binary's own business —
        // see `crate::helper` and `crate::audit` for why neither carries a field to aim.
        PrivilegedOp::HelperRemove {} => crate::helper::remove(),
        PrivilegedOp::AuditLogRemove {} => match crate::audit::path() {
            Ok(log) => crate::audit::remove(&log),
            // The same refusal `main` makes of an unreadable audit path, at the granularity of one
            // operation: a machine that will not name the directory has said nothing about whether
            // a log is there, and guessing a path in a process running as root is not a trade this
            // binary makes anywhere.
            Err(why) => OpOutcome::Failed {
                message: format!("this machine will not name a place for the audit log: {why}"),
            },
        },
    }
}

/// Whatever the request called this operation, for the log line that records it.
///
/// Reads the raw tag rather than the decoded operation, because the line that most needs writing is
/// the one for an operation that would not decode.
pub(crate) fn named(value: &serde_json::Value) -> &str {
    value
        .get("op")
        .and_then(serde_json::Value::as_str)
        .unwrap_or("unrecognised")
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The identity the filesystem gives a file this test wrote, which is the only kind of
    /// `Owner` there is: the type has no public constructor, deliberately — see `crate::request`.
    fn a_caller() -> (tempfile::TempDir, std::path::PathBuf, Owner) {
        let directory = tempfile::tempdir().expect("a temporary directory");
        let binary = directory.path().join("front-end");
        std::fs::write(&binary, b"not really a program").expect("the file");
        let caller = mixengine_platform::elevated::owner_of(&binary).expect("its owner");

        (directory, binary, caller)
    }

    #[test]
    fn probe_applies_under_any_token() {
        let (directory, _binary, caller) = a_caller();

        // D5's payoff: the operation whose job includes reporting whether the token is elevated has
        // to work when it is not, or it could never report `false`.
        assert!(matches!(
            apply(&PrivilegedOp::Probe {}, false, &caller, directory.path()),
            OpOutcome::Applied { .. }
        ));
        assert!(matches!(
            apply(&PrivilegedOp::Probe {}, true, &caller, directory.path()),
            OpOutcome::Applied { .. }
        ));
    }

    /// The gate, on the newest operation with an effect: nothing is copied anywhere by a process
    /// that does not hold an administrative token.
    #[test]
    fn installing_the_helper_needs_an_administrative_token() {
        let (directory, _binary, caller) = a_caller();

        assert!(matches!(
            apply(
                &PrivilegedOp::HelperInstall {},
                false,
                &caller,
                directory.path()
            ),
            OpOutcome::Refused { .. }
        ));
    }

    #[test]
    fn an_operation_this_build_has_never_heard_of_is_unsupported() {
        // **Was `trust-ca-install` until T49a made it real.** The firewall is the next operation
        // this binary does not have, and a name that stays unknown is the whole point of the test.
        let value = serde_json::json!({ "op": "firewall-allow", "ports": [1, 2, 3] });

        let outcome = decode(&value).unwrap_err();

        assert!(matches!(outcome, OpOutcome::Unsupported { .. }));
    }

    /// D3's intolerant half. An older helper must not silently ignore a field inside an operation it
    /// thinks it understands: that is how a weaker version of an operation gets applied and nobody
    /// finds out.
    #[test]
    fn a_known_operation_with_a_field_this_build_does_not_know_is_unsupported() {
        let value = serde_json::json!({ "op": "probe", "only-if": "something new" });

        let outcome = decode(&value).unwrap_err();

        assert!(
            matches!(outcome, OpOutcome::Unsupported { .. }),
            "{outcome:?}"
        );
    }

    /// The log has to say which operation a line is about even when the operation could not be
    /// decoded, and the raw tag is the only thing there is to say.
    #[test]
    fn an_undecodable_operation_is_still_named_in_the_log() {
        assert_eq!(
            named(&serde_json::json!({ "op": "hosts-apply" })),
            "hosts-apply"
        );
        assert_eq!(
            named(&serde_json::json!({ "nothing": true })),
            "unrecognised"
        );
        assert_eq!(named(&serde_json::json!(7)), "unrecognised");
    }

    /// The gate, from the side T41 added: an operation that needs a token and does not have one is
    /// refused before it can touch anything.
    #[test]
    fn a_hosts_change_under_an_ordinary_token_is_refused_before_it_writes() {
        let (directory, _binary, caller) = a_caller();

        let op = PrivilegedOp::hosts_apply([mixengine_proto::privileged::HostEntry {
            address: "127.0.0.1".parse().unwrap(),
            domain: "blog.test".to_owned(),
        }]);

        let outcome = apply(&op, false, &caller, directory.path());

        assert!(
            matches!(&outcome, OpOutcome::Refused { reason } if reason.contains("hosts-apply")),
            "{outcome:?}"
        );
    }

    /// The gate, from T45's side: both directions of resolver wiring need a token, and neither
    /// touches a file or the registry without one.
    #[test]
    fn resolver_operations_under_an_ordinary_token_are_refused_before_they_write() {
        use mixengine_proto::privileged::{ResolverPlan, ResolverTarget};

        let (directory, _binary, caller) = a_caller();

        for (op, named) in [
            (
                PrivilegedOp::ResolverApply {
                    plan: ResolverPlan::Nrpt {
                        tlds: vec!["test".to_owned()],
                    },
                },
                "resolver-apply",
            ),
            (
                PrivilegedOp::ResolverRevoke {
                    target: ResolverTarget::Nrpt {},
                },
                "resolver-revoke",
            ),
        ] {
            let outcome = apply(&op, false, &caller, directory.path());

            assert!(
                matches!(&outcome, OpOutcome::Refused { reason } if reason.contains(named)),
                "{outcome:?}"
            );
        }
    }

    /// The gate, from T74's side. **Including the plan that asks for nothing**, which is the one a
    /// careless reading would exempt: an empty plan removes rules, removing a rule needs a token,
    /// and a helper that skipped the check for it would be deciding policy from the request's own
    /// contents.
    #[test]
    fn a_firewall_change_under_an_ordinary_token_is_refused_before_it_writes() {
        use mixengine_proto::privileged::{FIREWALL_LABEL, FirewallPlan};

        let (directory, _binary, caller) = a_caller();

        for ports in [vec![80, 443], Vec::new()] {
            let op = PrivilegedOp::FirewallApply {
                plan: FirewallPlan {
                    ports,
                    label: format!("{FIREWALL_LABEL}blog"),
                },
            };

            let outcome = apply(&op, false, &caller, directory.path());

            assert!(
                matches!(&outcome, OpOutcome::Refused { reason } if reason.contains("firewall-apply")),
                "{outcome:?}"
            );
        }
    }

    /// The gate, from T87's side: both removals need a token, and neither touches a file without
    /// one. **Including the audit log's**, which is the one a careless reading would exempt — the
    /// log is world-readable, and the directory it sits in is not world-writable, which is the whole
    /// reason the log is out there.
    #[test]
    fn the_uninstall_removals_under_an_ordinary_token_are_refused_before_they_write() {
        let (directory, _binary, caller) = a_caller();

        for (op, named) in [
            (PrivilegedOp::HelperRemove {}, "helper-remove"),
            (PrivilegedOp::AuditLogRemove {}, "audit-log-remove"),
        ] {
            let outcome = apply(&op, false, &caller, directory.path());

            assert!(
                matches!(&outcome, OpOutcome::Refused { reason } if reason.contains(named)),
                "{outcome:?}"
            );
        }
    }

    /// The gate, from the newest side: an operation that needs a token and does not have one is
    /// refused before it can touch a file, and both directions of port access need one.
    #[test]
    fn port_access_under_an_ordinary_token_is_refused_before_it_writes() {
        use mixengine_proto::privileged::{PortAccessPlan, PortAccessTarget};

        let (directory, binary, caller) = a_caller();

        for (op, named) in [
            (
                PrivilegedOp::PortAccessGrant {
                    plan: PortAccessPlan::Capability {
                        binary: binary.clone(),
                        ports: vec![80],
                    },
                },
                "port-access-grant",
            ),
            (
                PrivilegedOp::PortAccessRevoke {
                    target: PortAccessTarget::Redirect {},
                },
                "port-access-revoke",
            ),
        ] {
            let outcome = apply(&op, false, &caller, directory.path());

            assert!(
                matches!(&outcome, OpOutcome::Refused { reason } if reason.contains(named)),
                "{outcome:?}"
            );
        }
    }

    /// The gate, from T88a's side: nothing is written into a root-owned directory by a process that
    /// does not hold an administrative token — including the operation whose other checks are a
    /// signature's, which a careless reading would exempt because "the signature decides".
    #[test]
    fn replacing_the_helper_needs_an_administrative_token() {
        let (directory, _binary, caller) = a_caller();

        let outcome = apply(
            &PrivilegedOp::HelperReplace {},
            false,
            &caller,
            directory.path(),
        );

        assert!(
            matches!(&outcome, OpOutcome::Refused { reason } if reason.contains("helper-replace")),
            "{outcome:?}"
        );
    }
}
