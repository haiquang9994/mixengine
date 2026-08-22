//! The file protocol between `mixengined` and `mixengine-elevate`.
//!
//! The daemon writes a [`PrivilegedRequest`] into a fresh single-use directory, raises the OS
//! elevation prompt on the helper with that file's path as its one argument, and reads the
//! [`PrivilegedResponse`] the helper leaves beside it. See
//! `.claude/decisions/0005-on-demand-elevation.md` and the T40 design for why it is files and not a
//! socket: the helper has no listener, no idle state, and exists for seconds.
//!
//! **The response file is the protocol.** When it is there, it is the answer and the exit code says
//! nothing; the exit code matters only when there is no file. That is not a preference — the macOS
//! launcher raises an AppleScript error instead of handing back a status, so an outcome encoded as a
//! number is an outcome one of the three systems has to reconstruct from an error string.
//!
//! **What is denied and what is tolerated runs in opposite directions on purpose.** The request and
//! every operation in it use `deny_unknown_fields`: a helper that silently ignored a field inside an
//! operation it thought it understood would apply a weaker version of that operation and tell nobody.
//! The response does not: the helper is excluded from auto-update, so a helper newer than the daemon
//! reading it is routine, and a field it added must not make its answer unreadable.

use std::path::PathBuf;

use serde::{Deserialize, Serialize};

use crate::ProtocolVersion;

/// The name the response takes, beside the request it answers.
///
/// Not passed as an argument: one fewer argument is one fewer thing the elevated process has to
/// validate, and the daemon already knows where it will be. Its **existence** is also the whole of
/// the anti-replay check — a request with an answer beside it has been processed and is refused.
pub const RESPONSE_FILE_NAME: &str = "response.json";

/// One batch of privileged operations, covered by one prompt.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields, rename_all = "kebab-case")]
pub struct PrivilegedRequest {
    /// The protocol this daemon speaks. A helper that does not know it refuses the whole request.
    pub version: ProtocolVersion,

    /// `MIXENGINE_HOME`. Every path in every operation is canonicalised and must resolve inside it,
    /// and the helper checks that this directory belongs to whoever owns the request file — without
    /// that, `--home C:\Windows\System32` is an escalation for every operation that takes a path.
    pub home: PathBuf,

    /// Echoed into the response, so a daemon cannot read the answer to an earlier request as the
    /// answer to this one. It is **not** the anti-replay check; [`RESPONSE_FILE_NAME`] is.
    pub nonce: String,

    /// The operations, left undecoded.
    ///
    /// A `Vec<PrivilegedOp>` would fail as a whole on one variant this build has never heard of,
    /// which — the helper being excluded from auto-update — is a routine event and not a corruption.
    /// Decoded one element at a time, an unknown operation becomes
    /// [`OpOutcome::Unsupported`] at its own index and its neighbours are applied. The daemon builds
    /// a `Vec<PrivilegedOp>` and serialises it into this field; the asymmetry is confined here.
    pub ops: Vec<serde_json::Value>,
}

/// The closed list of things that cross into the elevated process.
///
/// See `.claude/architecture/platform-abstraction.md`: the list is closed against operations **with
/// effects**, and adding one of those requires an ADR. [`PrivilegedOp::Probe`] has none.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "op", rename_all = "kebab-case", deny_unknown_fields)]
pub enum PrivilegedOp {
    /// Report this build: nothing is read, nothing is written, nothing is changed.
    ///
    /// What it reports arrives in the [`PrivilegedResponse`] header rather than in its own outcome,
    /// because every answer carries it — see that type.
    ///
    /// It is `Probe {}` and not `Probe` because serde deserialises a *unit* variant of an
    /// internally tagged enum through `deserialize_any`, which reads the map and drops every key
    /// but the tag — `deny_unknown_fields` never gets a chance to fire. An empty struct variant is
    /// deserialised as a struct, where it does. The rule above is only worth having if it holds for
    /// the operation that carries no fields as well as the ones that do.
    Probe {},
    // HostsApply arrives with T41; the resolver, the trust store, port access and the firewall with
    // T42, T44, T45 and Phase 5.
}

impl PrivilegedOp {
    /// Every operation this build knows, by wire name.
    ///
    /// Reported in [`PrivilegedResponse::supported_ops`] so a daemon can find out what the installed
    /// helper can do without spending a prompt to discover it by failure.
    pub const ALL: &'static [&'static str] = &["probe"];

    /// Does this operation need an administrative token to mean anything?
    ///
    /// **A property of the operation, not a gate on the process.** The obvious frame refuses to do
    /// anything at all when it is not elevated, and `Probe` is what shows that to be wrong: the
    /// operation whose job includes reporting whether the token is elevated could then never report
    /// `false`. The helper applies this at one place, which is what keeps it auditable.
    #[must_use]
    pub fn requires_elevation(&self) -> bool {
        match self {
            Self::Probe {} => false,
        }
    }

    /// The wire tag, which is also what the audit log records.
    #[must_use]
    pub fn name(&self) -> &'static str {
        match self {
            Self::Probe {} => "probe",
        }
    }
}

/// What the helper did, one entry per operation, plus what it is.
///
/// **The report is a property of the response and not the outcome of `Probe`.** Nesting it in
/// [`OpOutcome::Applied`]'s `detail` would put a JSON document inside a JSON string, and would mean
/// the daemon learns what the installed helper can do only on the round trips where it thought to
/// ask. Here it costs a few strings, arrives on every answer, and is read the same way whatever the
/// request contained.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub struct PrivilegedResponse {
    /// The protocol the helper speaks.
    pub version: ProtocolVersion,

    /// The helper binary's own version, which is not the protocol's: it is installed once and
    /// excluded from auto-update, so it drifts behind the daemon by design.
    pub elevate_version: String,

    /// Echoed from the request.
    pub nonce: String,

    /// Was this process actually running with an administrative token?
    pub elevated: bool,

    /// [`PrivilegedOp::ALL`] for the build that answered.
    pub supported_ops: Vec<String>,

    /// Where this helper records what it applied — reported whether or not anything was written to
    /// it, so `mix doctor` can find it on a machine where nothing has been applied yet.
    pub audit_log: PathBuf,

    /// One outcome per element of [`PrivilegedRequest::ops`], at the same index.
    pub results: Vec<OpOutcome>,
}

/// What became of one operation.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "outcome", rename_all = "kebab-case")]
pub enum OpOutcome {
    /// Done, and the machine changed.
    Applied {
        /// What changed, for a log line and for `mix doctor`.
        detail: String,
    },

    /// The machine was already in the state this asked for. Not a failure and not a change.
    AlreadyDone,

    /// Validation said no — the caller's fault, and the same request will be refused again.
    Refused {
        /// Which rule it broke.
        reason: String,
    },

    /// This build does not know this operation, or does not understand it as it was written.
    Unsupported {
        /// What could not be decoded.
        reason: String,
    },

    /// The operating system refused. Trying again may work; nothing about the request is wrong.
    Failed {
        /// The OS's own complaint.
        message: String,
    },
}

/// What became of one *attempt to elevate* — the outcome of raising the prompt, not of the batch.
///
/// Defined here and used by T40a, where the three launchers live. A declined prompt cannot be an exit
/// code of the helper's, because when the user clicks Cancel the helper never ran: `ERROR_CANCELLED`
/// (1223), osascript's `-128` and `pkexec`'s 126 all map onto [`ElevationOutcome::Declined`].
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "outcome", rename_all = "kebab-case")]
pub enum ElevationOutcome {
    /// The helper ran. Whether the batch succeeded is in the response file.
    Completed,

    /// The user said no. A normal outcome, and the daemon goes into degraded mode (T40b).
    Declined,

    /// There is no way to raise a prompt on this machine — no polkit agent, no session.
    Unavailable {
        /// What is missing, phrased for a user, with the manual command where one exists.
        reason: String,
    },
}

#[cfg(test)]
mod tests {
    use super::*;

    use crate::PROTOCOL_VERSION;

    /// The daemon writes this; the helper reads it. A change to either side that the other did not
    /// make shows up here first.
    #[test]
    fn a_request_round_trips() {
        let request = PrivilegedRequest {
            version: PROTOCOL_VERSION,
            home: PathBuf::from("/home/someone/.mixengine"),
            nonce: "b8f0…".to_owned(),
            ops: vec![serde_json::json!({ "op": "probe" })],
        };

        let encoded = serde_json::to_string(&request).unwrap();
        assert_eq!(
            serde_json::from_str::<PrivilegedRequest>(&encoded).unwrap(),
            request
        );
    }

    /// D3, the reading half: an operation this build has never heard of survives as an undecoded
    /// value and does not take its neighbours down with it.
    #[test]
    fn an_unknown_operation_does_not_fail_the_envelope() {
        let text = r#"{
            "version": 1,
            "home": "/home/someone/.mixengine",
            "nonce": "n",
            "ops": [{ "op": "probe" }, { "op": "trust-ca-install", "der": [1, 2, 3] }]
        }"#;

        let request: PrivilegedRequest = serde_json::from_str(text).unwrap();

        assert_eq!(request.ops.len(), 2);
        assert!(serde_json::from_value::<PrivilegedOp>(request.ops[0].clone()).is_ok());
        assert!(serde_json::from_value::<PrivilegedOp>(request.ops[1].clone()).is_err());
    }

    /// D3, the intolerant half: a field inside an operation this build *does* know is fatal for that
    /// operation. Silently ignoring it is how a weaker version of an operation gets applied.
    #[test]
    fn an_unknown_field_inside_a_known_operation_is_fatal() {
        let value = serde_json::json!({ "op": "probe", "and-also": "something new" });

        assert!(serde_json::from_value::<PrivilegedOp>(value).is_err());
    }

    /// A field the envelope does not know is a daemon speaking a protocol this build does not have.
    #[test]
    fn an_unknown_field_in_the_envelope_is_fatal() {
        let text = r#"{
            "version": 1, "home": "/h", "nonce": "n", "ops": [], "deadline": 5
        }"#;

        assert!(serde_json::from_str::<PrivilegedRequest>(text).is_err());
    }

    /// D5: the one operation that changes nothing is the one that does not need a token.
    #[test]
    fn probe_is_the_operation_that_needs_no_privilege() {
        assert!(!PrivilegedOp::Probe {}.requires_elevation());
    }

    /// `name()` is what goes in the audit log and in `supported_ops`; the tag is what goes on the
    /// wire. Two spellings of one operation would make the log unreadable against the protocol.
    #[test]
    fn the_name_of_an_operation_is_its_wire_tag() {
        let encoded = serde_json::to_value(PrivilegedOp::Probe {}).unwrap();

        assert_eq!(encoded["op"], PrivilegedOp::Probe {}.name());
        assert!(PrivilegedOp::ALL.contains(&PrivilegedOp::Probe {}.name()));
        assert_eq!(PrivilegedOp::ALL.len(), 1, "ALL and the enum have drifted");
    }

    /// The response is read by a daemon that may be older than the helper that wrote it, so an
    /// added field must not make it unreadable. The opposite rule to the request, deliberately.
    #[test]
    fn a_response_tolerates_a_field_the_reader_does_not_know() {
        let text = r#"{
            "version": 1, "elevate-version": "0.1.0", "nonce": "n", "elevated": true,
            "supported-ops": ["probe"], "audit-log": "/var/log/mixengine/elevate.log",
            "results": [{ "outcome": "applied", "detail": "…" }],
            "duration-ms": 4
        }"#;

        let response: PrivilegedResponse = serde_json::from_str(text).unwrap();

        assert_eq!(response.results.len(), 1);
        assert!(response.elevated);
    }

    #[test]
    fn every_outcome_round_trips() {
        let outcomes = vec![
            OpOutcome::Applied {
                detail: "d".to_owned(),
            },
            OpOutcome::AlreadyDone,
            OpOutcome::Refused {
                reason: "r".to_owned(),
            },
            OpOutcome::Unsupported {
                reason: "u".to_owned(),
            },
            OpOutcome::Failed {
                message: "m".to_owned(),
            },
        ];

        let encoded = serde_json::to_string(&outcomes).unwrap();
        assert_eq!(
            serde_json::from_str::<Vec<OpOutcome>>(&encoded).unwrap(),
            outcomes
        );
    }

    /// T40a's vocabulary, defined here so that task has a word to use — D11.
    #[test]
    fn a_declined_prompt_is_a_word_rather_than_a_number() {
        let encoded = serde_json::to_string(&ElevationOutcome::Declined).unwrap();

        assert_eq!(encoded, r#"{"outcome":"declined"}"#);
    }
}
