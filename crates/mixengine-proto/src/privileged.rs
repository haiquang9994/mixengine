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

use std::net::IpAddr;
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

/// One line of the managed block: a name, and the address it resolves to.
///
/// The address is an [`IpAddr`] and not a string because the helper refuses anything that is not
/// loopback, and a refusal that had to parse the field first would be a refusal with a second way
/// to be wrong. `serde` renders it the way a hosts file spells one.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields, rename_all = "kebab-case")]
pub struct HostEntry {
    /// Where the name points. Only `127.0.0.1` and `::1` are ever accepted — see the T41 design, D5.
    pub address: IpAddr,

    /// The name, lowercased and already checked by whoever built this.
    pub domain: String,
}

/// One port a site is reached on, and the ordinary port a program binds to answer it.
///
/// On macOS these differ — a packet-filter rule sends 80 to 8080 — and on the other two they do not.
/// The pair travels together because the layer that generates a front end's configuration needs both
/// numbers and may not ask which operating system it is on.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields, rename_all = "kebab-case")]
pub struct PortRedirect {
    /// What a browser asks for: 80 or 443.
    pub answer: u16,

    /// What a program actually binds, which an ordinary account may.
    pub bind: u16,
}

/// What granting port access means on the machine being asked — the T42 design, D2 and D4.
///
/// **Two variants rather than one struct holding both a binary and a redirect list**, because a
/// field the helper does not use is a field the helper cannot validate, and validating is that
/// binary's entire job. Every OS refuses the variant that is not its mechanism; no branch quietly
/// does nothing.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "method", rename_all = "kebab-case", deny_unknown_fields)]
pub enum PortAccessPlan {
    /// Linux: `cap_net_bind_service` on the front end's binary, which then binds 80 itself.
    Capability {
        /// The program the capability goes on. The helper checks that the caller owns it and that
        /// nobody else can write it — the T42 design, D5.
        binary: PathBuf,

        /// Which reserved ports it is being allowed. Only 80 and 443 are ever accepted.
        ports: Vec<u16>,
    },

    /// macOS: a packet-filter anchor, its declaration in `/etc/pf.conf`, and the boot job that
    /// enables pf — see ADR 0012. The program binds an ordinary port instead.
    Redirect {
        /// Every port that moves, and where to.
        redirects: Vec<PortRedirect>,
    },
}

/// What taking port access away means.
///
/// Mirrors [`PortAccessPlan`] with only the fields a removal reads: a capability is cleared whole,
/// so there is no port to name, and the three files a redirect leaves behind are constants in the
/// helper rather than anything a request may choose.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "method", rename_all = "kebab-case", deny_unknown_fields)]
pub enum PortAccessTarget {
    /// Clear `security.capability` from this binary.
    Capability {
        /// The program to clear it from, checked exactly as a grant's is.
        binary: PathBuf,
    },

    /// Remove the anchor, its block in `/etc/pf.conf` and the boot job.
    ///
    /// `Redirect {}` and not `Redirect`, for the reason [`PrivilegedOp::Probe`] is written that way:
    /// serde reads a unit variant of an internally tagged enum through `deserialize_any`, where
    /// `deny_unknown_fields` never gets a chance to fire.
    Redirect {},
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

    /// Set MixEngine's block in the hosts file to exactly `entries`.
    ///
    /// **The whole state, not a delta** — the T41 design, D1. A block that has drifted cannot be
    /// pulled back by "add this line", so a whole-state operation is idempotent, is its own repair,
    /// and makes "already done" a byte comparison rather than a judgement. An empty list removes
    /// the block.
    HostsApply {
        /// Sorted and deduplicated by [`PrivilegedOp::hosts_apply`], which is the only way one
        /// should be built.
        entries: Vec<HostEntry>,
    },

    /// Let this machine's front end answer on the ports the OS reserves — roadmap task **T42**.
    ///
    /// **Whole state, like [`HostsApply`](Self::HostsApply)**: the plan says what the machine should
    /// end up allowing, so a second request supersedes the first rather than queueing behind it, and
    /// "already done" is a comparison rather than a judgement.
    PortAccessGrant {
        /// What to grant, and how — one variant per OS mechanism.
        plan: PortAccessPlan,
    },

    /// Take it away again.
    ///
    /// **Nothing in T42 enqueues one** — the T42 design, D12. The producer asks in one direction
    /// only, deliberately: on Linux the question needs the front end's binary, which is exactly what
    /// a home with no front end cannot supply. Uninstall (T87) is the producer that can. It ships
    /// built, validated and tested, which is the shape T20, T21 and T22 landed in.
    PortAccessRevoke {
        /// What to take away.
        target: PortAccessTarget,
    },
    // The resolver, the trust store and the firewall arrive with T44, T45 and Phase 5.
}

impl PrivilegedOp {
    /// Every operation this build knows, by wire name.
    ///
    /// Reported in [`PrivilegedResponse::supported_ops`] so a daemon can find out what the installed
    /// helper can do without spending a prompt to discover it by failure.
    pub const ALL: &'static [&'static str] = &[
        "probe",
        "hosts-apply",
        "port-access-grant",
        "port-access-revoke",
    ];

    /// A hosts change from whatever order its caller happened to have.
    ///
    /// Sorted and deduplicated, so two orderings of one change are one operation: the queue
    /// deduplicates on identity (see [`dedupe_key`](Self::dedupe_key)) and the *equality* below it
    /// is what decides whether anything is announced.
    #[must_use]
    pub fn hosts_apply(entries: impl IntoIterator<Item = HostEntry>) -> Self {
        let mut entries: Vec<HostEntry> = entries.into_iter().collect();

        // By name first: this order is what the block is rendered in and what `describe` reads out,
        // and a person scanning a dialog is scanning names.
        entries.sort_by(|left, right| {
            (left.domain.as_str(), left.address).cmp(&(right.domain.as_str(), right.address))
        });
        entries.dedup();

        Self::HostsApply { entries }
    }

    /// The identity a queue deduplicates on — the T41 design, D2.
    ///
    /// For an operation that carries no state this is its serialisation, so two identical requests
    /// are one row. For a **whole-state** operation it is the bare kind: two `hosts-apply` rows
    /// disagreeing about what the file should hold would both be valid and both be rendered on the
    /// one screen whose job is to say what is about to happen, so the newer state supersedes the
    /// older one instead.
    #[must_use]
    pub fn dedupe_key(&self) -> String {
        match self {
            // Falling back to the tag cannot happen — this type holds nothing serde refuses — and
            // is written rather than unwrapped because nothing in this crate panics.
            Self::Probe {} => {
                serde_json::to_string(self).unwrap_or_else(|_| self.name().to_owned())
            }
            Self::HostsApply { .. } => self.name().to_owned(),
            // D12: two values of one question — what port access should this machine have? — so a
            // revoke enqueued behind a pending grant replaces it rather than queueing after it.
            Self::PortAccessGrant { .. } | Self::PortAccessRevoke { .. } => {
                "port-access".to_owned()
            }
        }
    }

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
            Self::HostsApply { .. } => true,
            Self::PortAccessGrant { .. } | Self::PortAccessRevoke { .. } => true,
        }
    }

    /// The wire tag, which is also what the audit log records.
    #[must_use]
    pub fn name(&self) -> &'static str {
        match self {
            Self::Probe {} => "probe",
            Self::HostsApply { .. } => "hosts-apply",
            Self::PortAccessGrant { .. } => "port-access-grant",
            Self::PortAccessRevoke { .. } => "port-access-revoke",
        }
    }

    /// What this operation will literally change, for a person about to allow it.
    ///
    /// **Derived from the operation rather than stored beside it** — the T40b design, D7. The
    /// alternative is a `summary` written by whoever enqueued the operation and kept in its row,
    /// which is a description that can disagree with what will be applied and would preserve that
    /// disagreement across a restart, on the one screen whose whole job is to tell the truth before
    /// somebody clicks Allow.
    ///
    /// [`String`] and not `&'static str`: the operations that matter carry data — `HostsApply`'s
    /// description is its domains (T41) — and a constant would be a shape the next operation has to
    /// break immediately.
    #[must_use]
    pub fn describe(&self) -> String {
        match self {
            Self::Probe {} => "report the installed helper's version, whether it holds an \
                               administrative token, and where it writes its audit log"
                .to_owned(),
            Self::HostsApply { entries } => describe_hosts(entries),
            Self::PortAccessGrant { plan } => describe_grant(plan),
            Self::PortAccessRevoke { target } => describe_revoke(target),
        }
    }
}

/// What a hosts change will literally do, for a person about to allow it.
///
/// The addresses are named when they differ, because the helper permits `::1` as well as
/// `127.0.0.1` (D5) and a description that hid the difference would be describing something else.
fn describe_hosts(entries: &[HostEntry]) -> String {
    let Some(first) = entries.first() else {
        return "remove MixEngine's block from the hosts file".to_owned();
    };

    let uniform = entries.iter().all(|entry| entry.address == first.address);
    let plural = if entries.len() == 1 { "" } else { "s" };

    let names: Vec<String> = entries
        .iter()
        .map(|entry| {
            if uniform {
                entry.domain.clone()
            } else {
                format!("{} ({})", entry.domain, entry.address)
            }
        })
        .collect();

    let at = if uniform {
        first.address.to_string()
    } else {
        "loopback".to_owned()
    };

    format!(
        "point {} name{plural} at {at} in the hosts file: {}",
        entries.len(),
        names.join(", ")
    )
}

/// What a port-access grant will literally do, for a person about to allow it.
///
/// The binary's whole path, never its file name: T42's D11 leaves exactly one control against a
/// compromised daemon pointing the grant at a program of its own choosing, and this is it.
fn describe_grant(plan: &PortAccessPlan) -> String {
    match plan {
        PortAccessPlan::Capability { binary, ports } => format!(
            "let {} bind port{} {} without an administrator, by giving that file the \
             cap_net_bind_service capability",
            binary.display(),
            if ports.len() == 1 { "" } else { "s" },
            list(ports)
        ),
        PortAccessPlan::Redirect { redirects } => format!(
            "send {} on 127.0.0.1 to a port an ordinary program may bind, through a packet-filter \
             anchor, a block in /etc/pf.conf and a boot-time job that enables the packet filter: {}",
            if redirects.len() == 1 {
                "one port"
            } else {
                "two ports"
            },
            redirects
                .iter()
                .map(|redirect| format!("{} to {}", redirect.answer, redirect.bind))
                .collect::<Vec<_>>()
                .join(", ")
        ),
    }
}

/// What taking it away will literally do.
fn describe_revoke(target: &PortAccessTarget) -> String {
    match target {
        PortAccessTarget::Capability { binary } => format!(
            "take the cap_net_bind_service capability back off {}",
            binary.display()
        ),
        PortAccessTarget::Redirect {} => "remove MixEngine's packet-filter anchor, its block in \
                                          /etc/pf.conf and its boot-time job"
            .to_owned(),
    }
}

/// `80 and 443`, the way a sentence names a short list.
fn list(ports: &[u16]) -> String {
    let rendered: Vec<String> = ports.iter().map(u16::to_string).collect();

    match rendered.split_last() {
        None => String::new(),
        Some((last, [])) => last.clone(),
        Some((last, rest)) => format!("{} and {last}", rest.join(", ")),
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
        assert_eq!(PrivilegedOp::ALL.len(), 4, "ALL and the enum have drifted");
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

    /// D7: a description is derived from the operation every time it is rendered, so it cannot
    /// disagree with what will actually be applied. What is asserted here is that it says something
    /// a person could act on — the wire tag repeated back is not that.
    #[test]
    fn every_operation_says_what_it_will_change() {
        for op in [
            PrivilegedOp::Probe {},
            PrivilegedOp::hosts_apply([entry("127.0.0.1", "blog.test")]),
            PrivilegedOp::hosts_apply([]),
        ] {
            let described = op.describe();

            assert!(!described.is_empty());
            assert_ne!(described, op.name(), "a tag is not a description");
            assert!(
                described.chars().next().is_some_and(char::is_lowercase),
                "descriptions are rendered in a list and start mid-sentence: {described}"
            );
        }
    }

    /// D1: the operation carries the whole managed block, so two orderings of one change are one
    /// operation and not two rows on the screen that asks a person to allow them.
    #[test]
    fn a_hosts_change_is_a_set_and_not_a_sequence() {
        let one = PrivilegedOp::hosts_apply([
            entry("127.0.0.1", "api.blog.test"),
            entry("127.0.0.1", "blog.test"),
        ]);
        let other = PrivilegedOp::hosts_apply([
            entry("127.0.0.1", "blog.test"),
            entry("127.0.0.1", "api.blog.test"),
            entry("127.0.0.1", "blog.test"),
        ]);

        assert_eq!(
            one, other,
            "order and repetition are not part of the request"
        );
    }

    /// D2: a whole-state operation deduplicates on its *kind*, so a newer state supersedes an older
    /// one rather than queueing beside it. `Probe`'s key is unchanged, which is what makes the
    /// column need no migration.
    #[test]
    fn a_whole_state_operation_deduplicates_on_its_kind() {
        let one = PrivilegedOp::hosts_apply([entry("127.0.0.1", "blog.test")]);
        let other = PrivilegedOp::hosts_apply([entry("127.0.0.1", "shop.test")]);

        assert_eq!(one.dedupe_key(), other.dedupe_key());
        assert_eq!(one.dedupe_key(), "hosts-apply");
        assert_ne!(
            one, other,
            "the same key, and deliberately not the same operation"
        );

        assert_eq!(
            PrivilegedOp::Probe {}.dedupe_key(),
            serde_json::to_string(&PrivilegedOp::Probe {}).unwrap(),
            "Probe's key is still its serialisation, so no row in an existing home moves"
        );
    }

    /// It needs a token, unlike `Probe`, and the helper's one gate is what reads this.
    #[test]
    fn writing_the_hosts_file_needs_an_administrative_token() {
        assert!(PrivilegedOp::hosts_apply([entry("127.0.0.1", "blog.test")]).requires_elevation());
        assert_eq!(
            PrivilegedOp::hosts_apply([]).name(),
            "hosts-apply",
            "the tag is the audit log's word for it"
        );
    }

    /// The screen T64 renders exists to be read before somebody clicks Allow, so the description is
    /// the domains themselves and not a count.
    #[test]
    fn a_hosts_change_describes_itself_by_naming_every_domain() {
        let described = PrivilegedOp::hosts_apply([
            entry("127.0.0.1", "blog.test"),
            entry("127.0.0.1", "api.blog.test"),
        ])
        .describe();

        assert!(described.contains("blog.test"), "{described}");
        assert!(described.contains("api.blog.test"), "{described}");
        assert!(described.contains("127.0.0.1"), "{described}");

        assert_eq!(
            PrivilegedOp::hosts_apply([]).describe(),
            "remove MixEngine's block from the hosts file"
        );
    }

    /// The request is intolerant, and an operation that carries data is where that matters.
    #[test]
    fn a_hosts_entry_with_a_field_this_build_does_not_know_is_fatal() {
        let value = serde_json::json!({
            "op": "hosts-apply",
            "entries": [{ "address": "127.0.0.1", "domain": "blog.test", "comment": "hi" }]
        });

        assert!(serde_json::from_value::<PrivilegedOp>(value).is_err());
    }

    #[test]
    fn a_hosts_change_round_trips() {
        let op = PrivilegedOp::hosts_apply([entry("::1", "blog.test")]);

        let encoded = serde_json::to_string(&op).unwrap();
        assert_eq!(serde_json::from_str::<PrivilegedOp>(&encoded).unwrap(), op);
        assert!(encoded.contains(r#""op":"hosts-apply""#), "{encoded}");
    }

    /// A `HostEntry` for a test, from the two strings a reader recognises.
    fn entry(address: &str, domain: &str) -> HostEntry {
        HostEntry {
            address: address.parse().expect("a literal address"),
            domain: domain.to_owned(),
        }
    }

    /// D12: they are two values of one question — *what port access should this machine have?* — so
    /// the guarded upsert supersedes rather than queues, and execution order never has to be
    /// reasoned about.
    #[test]
    fn granting_and_revoking_port_access_are_one_row() {
        let grant = PrivilegedOp::PortAccessGrant {
            plan: PortAccessPlan::Capability {
                binary: PathBuf::from("/home/someone/.mixengine/packages/caddy/caddy"),
                ports: vec![80, 443],
            },
        };
        let revoke = PrivilegedOp::PortAccessRevoke {
            target: PortAccessTarget::Redirect {},
        };

        assert_eq!(grant.dedupe_key(), "port-access");
        assert_eq!(revoke.dedupe_key(), "port-access");
        assert_ne!(
            grant, revoke,
            "the same key, and deliberately not the same operation"
        );
    }

    /// Both write outside the home, so both need the token — and the helper's one gate reads this.
    #[test]
    fn port_access_needs_an_administrative_token_in_both_directions() {
        assert!(
            PrivilegedOp::PortAccessGrant {
                plan: PortAccessPlan::Redirect {
                    redirects: vec![PortRedirect {
                        answer: 80,
                        bind: 8080
                    }],
                },
            }
            .requires_elevation()
        );
        assert!(
            PrivilegedOp::PortAccessRevoke {
                target: PortAccessTarget::Capability {
                    binary: PathBuf::from("/x/caddy")
                },
            }
            .requires_elevation()
        );
    }

    /// The screen T64 renders is read before somebody clicks Allow. D11 leaves exactly one control
    /// against a compromised daemon pointing a grant at a binary of its own choosing, and it is that
    /// the whole path is printed here.
    #[test]
    fn a_port_access_change_describes_what_it_will_do_to_the_machine() {
        let capability = PrivilegedOp::PortAccessGrant {
            plan: PortAccessPlan::Capability {
                binary: PathBuf::from("/home/someone/.mixengine/packages/caddy/caddy"),
                ports: vec![80, 443],
            },
        }
        .describe();

        assert!(capability.contains("/home/someone"), "{capability}");
        assert!(capability.contains("80"), "{capability}");
        assert!(capability.contains("443"), "{capability}");

        let redirect = PrivilegedOp::PortAccessGrant {
            plan: PortAccessPlan::Redirect {
                redirects: vec![PortRedirect {
                    answer: 80,
                    bind: 8080,
                }],
            },
        }
        .describe();

        assert!(redirect.contains("8080"), "{redirect}");

        let taken = PrivilegedOp::PortAccessRevoke {
            target: PortAccessTarget::Capability {
                binary: PathBuf::from("/x/caddy"),
            },
        }
        .describe();

        assert!(taken.contains("/x/caddy"), "{taken}");
    }

    #[test]
    fn both_port_access_operations_round_trip() {
        for op in [
            PrivilegedOp::PortAccessGrant {
                plan: PortAccessPlan::Capability {
                    binary: PathBuf::from("/x/caddy"),
                    ports: vec![80],
                },
            },
            PrivilegedOp::PortAccessRevoke {
                target: PortAccessTarget::Redirect {},
            },
        ] {
            let encoded = serde_json::to_string(&op).unwrap();

            assert_eq!(serde_json::from_str::<PrivilegedOp>(&encoded).unwrap(), op);
            assert!(PrivilegedOp::ALL.contains(&op.name()), "{}", op.name());
        }
    }

    /// D3's intolerant half, on the operation that carries the most data: a field this build does
    /// not know, inside one it thinks it understands, is fatal — or a weaker grant gets applied and
    /// nobody finds out.
    #[test]
    fn a_port_access_plan_with_a_field_this_build_does_not_know_is_fatal() {
        let value = serde_json::json!({
            "op": "port-access-grant",
            "plan": { "method": "capability", "binary": "/x/caddy", "ports": [80], "force": true }
        });

        assert!(serde_json::from_value::<PrivilegedOp>(value).is_err());
    }
}
