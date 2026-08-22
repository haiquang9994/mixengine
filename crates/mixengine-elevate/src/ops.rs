//! Deciding one operation: is it one this build knows, may it run under this token, and what
//! happened.
//!
//! **The elevation gate is here and nowhere else.** One `if`, applied to every operation, answered by
//! the operation itself — which is what keeps it findable by somebody auditing this binary. The
//! alternative frame, refusing to do anything at all when the process is not elevated, is what
//! `Probe` rules out.

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
pub(crate) fn apply(op: &PrivilegedOp, elevated: bool) -> OpOutcome {
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

    #[test]
    fn probe_applies_under_any_token() {
        // D5's payoff: the operation whose job includes reporting whether the token is elevated has
        // to work when it is not, or it could never report `false`.
        assert!(matches!(
            apply(&PrivilegedOp::Probe {}, false),
            OpOutcome::Applied { .. }
        ));
        assert!(matches!(
            apply(&PrivilegedOp::Probe {}, true),
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
}
