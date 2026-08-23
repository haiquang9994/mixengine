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
pub(crate) fn apply(op: &PrivilegedOp, elevated: bool, caller: &Owner) -> OpOutcome {
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

        // Roadmap task T45, landing in two commits: the vocabulary first so that the queue, the
        // grant screen and the wire contract can be tested against it, and the validation next
        // door immediately after. Until then the helper answers what an older build would answer
        // for an operation it has never heard of, which is the honest reply and is what
        // `supported_ops` exists to let a daemon find out without spending a prompt.
        PrivilegedOp::ResolverApply { .. } | PrivilegedOp::ResolverRevoke { .. } => {
            OpOutcome::Unsupported {
                reason: "this build cannot wire a resolver yet".to_owned(),
            }
        }
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
        let (_directory, _binary, caller) = a_caller();

        // D5's payoff: the operation whose job includes reporting whether the token is elevated has
        // to work when it is not, or it could never report `false`.
        assert!(matches!(
            apply(&PrivilegedOp::Probe {}, false, &caller),
            OpOutcome::Applied { .. }
        ));
        assert!(matches!(
            apply(&PrivilegedOp::Probe {}, true, &caller),
            OpOutcome::Applied { .. }
        ));
    }

    #[test]
    fn an_operation_this_build_has_never_heard_of_is_unsupported() {
        let value = serde_json::json!({ "op": "trust-ca-install", "der": [1, 2, 3] });

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
        let (_directory, _binary, caller) = a_caller();

        let op = PrivilegedOp::hosts_apply([mixengine_proto::privileged::HostEntry {
            address: "127.0.0.1".parse().unwrap(),
            domain: "blog.test".to_owned(),
        }]);

        let outcome = apply(&op, false, &caller);

        assert!(
            matches!(&outcome, OpOutcome::Refused { reason } if reason.contains("hosts-apply")),
            "{outcome:?}"
        );
    }

    /// The gate, from the newest side: an operation that needs a token and does not have one is
    /// refused before it can touch a file, and both directions of port access need one.
    #[test]
    fn port_access_under_an_ordinary_token_is_refused_before_it_writes() {
        use mixengine_proto::privileged::{PortAccessPlan, PortAccessTarget};

        let (_directory, binary, caller) = a_caller();

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
            let outcome = apply(&op, false, &caller);

            assert!(
                matches!(&outcome, OpOutcome::Refused { reason } if reason.contains(named)),
                "{outcome:?}"
            );
        }
    }
}
