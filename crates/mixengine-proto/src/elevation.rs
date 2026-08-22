//! The queue of privileged operations, and what a client is told about it.
//!
//! The daemon batches everything that needs an administrative token and spends **one** prompt on the
//! whole of it — `.claude/decisions/0005-on-demand-elevation.md` calls elevating inside a loop a
//! defect. What is here is the vocabulary that makes the waiting visible: what is in the queue, what
//! each operation will change, whether this machine can raise a prompt at all, and what the last
//! grant did.
//!
//! **Nothing here decides anything.** A declined prompt is a normal outcome, so a client renders a
//! pending list rather than an error, and the daemon is what knows the list is not empty.

use crate::privileged::{ElevationOutcome, PrivilegedOp};
use crate::{JobId, Timestamp};

/// One row of the queue, by its rowid.
///
/// A newtype rather than a bare `i64` on `.claude/standards/rust.md`'s rule: the one method that
/// takes one is `elevation.drop`, and an integer there could be a job, an operation or a mistake.
#[derive(
    Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, serde::Serialize, serde::Deserialize,
)]
#[serde(transparent)]
pub struct PendingOpId(pub i64);

impl std::fmt::Display for PendingOpId {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.0)
    }
}

/// One operation waiting for somebody to allow it.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct PendingOp {
    /// Which row, so `elevation.drop` can name one.
    pub id: PendingOpId,

    /// The operation itself, decoded. A row this build cannot decode is deleted rather than carried
    /// — see the T40b design, D2 — so this is never a value nobody can act on.
    pub op: PrivilegedOp,

    /// [`PrivilegedOp::describe`], rendered here rather than by the client.
    ///
    /// `CLAUDE.md`: a client renders what the daemon returns. It matters more here than anywhere
    /// else — this is the sentence somebody reads before allowing a change to a file outside their
    /// home — and a second client composing its own wording would be a second chance to get it
    /// wrong.
    pub description: String,

    /// When it was first asked for.
    ///
    /// **First** and not last: the same operation enqueued again keeps this reading (D2), so
    /// "pending since" says how long the machine has been missing it rather than how recently
    /// somebody noticed.
    pub requested_at: Timestamp,
}

/// The three facts `daemon.status` carries about elevation — the T40b design, D6.
///
/// *Degraded* is not a field here and is not a column anywhere: it means
/// [`pending`](ElevationSummary::pending) is not zero, computed where it is asked for. A second
/// representation of one fact is a second thing that can be wrong, and this one would be wrong in
/// the worst direction — reporting a healthy machine that is missing its hosts entries.
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct ElevationSummary {
    /// Is the **daemon itself** running with an administrative token?
    ///
    /// Reported rather than refused (D10): CI's Windows third runs the daemon suites under a full
    /// token, and a hard refusal would turn one platform red for a reason that has nothing to do
    /// with the code under test. What is worth saying about it is that every supervised service
    /// inherits that token.
    pub elevated: bool,

    /// Could a prompt be raised on this machine? `Elevation::probe` and a helper that is there.
    pub can_prompt: bool,

    /// How many operations are waiting. Not zero means degraded.
    pub pending: usize,
}

/// `elevation.status`, and what `elevation.drop` leaves behind.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct ElevationStatus {
    /// As [`ElevationSummary::elevated`].
    pub elevated: bool,

    /// As [`ElevationSummary::can_prompt`].
    pub can_prompt: bool,

    /// Why not, when it cannot.
    ///
    /// On Linux this is the whole `pkexec` command to run by hand, which the platform layer built it
    /// to be — dropping it would leave a person with "unavailable" and nothing to type. Absent
    /// rather than `null` when there is nothing to say.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub reason: Option<String>,

    /// Where `mixengine-elevate` was found, when it was.
    ///
    /// A string and not a `PathBuf` for [`DaemonStatus`](crate::DaemonStatus)' reason: serde refuses
    /// a `PathBuf` that is not valid UTF-8, and an oddly named directory is a reason to see it
    /// spelled oddly rather than a reason for the call to fail.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub helper: Option<String>,

    /// Everything waiting, oldest first.
    pub pending: Vec<PendingOp>,

    /// What the most recent grant did, **for as long as this daemon has been up**.
    ///
    /// Deliberately not persisted: the durable fact is the queue, and a stored "you declined once"
    /// would outlive the reason it was true. A daemon that has just started answers [`None`], and
    /// the pending list already says everything a client needs.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub last: Option<GrantOutcome>,
}

/// What one grant did.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct GrantOutcome {
    /// The job that raised the prompt, so `job.status` can be asked about it afterwards.
    pub job: JobId,

    /// When it ended.
    pub at: Timestamp,

    /// What became of the prompt.
    ///
    /// `flatten`, on [`JobFinish`](crate::JobFinish)'s precedent: one object with one discriminator,
    /// rather than an `outcome` wrapper a client unwraps for this type and for nothing else. It also
    /// puts [`ElevationOutcome::Unavailable`]'s `reason` on the top level, where it reads as the
    /// sentence it is.
    #[serde(flatten)]
    pub outcome: ElevationOutcome,

    /// How many operations came back done — applied, or already so.
    pub applied: usize,

    /// How many are still in the queue afterwards.
    ///
    /// Not `pending.len()` minus `applied`: an operation the OS refused for a reason of its own is
    /// kept and one the helper refused is dropped, so the two numbers do not add up and pretending
    /// they do would make a client compute a third that is wrong.
    pub still_pending: usize,
}

/// `elevation.drop` — forget one operation, or all of them.
#[derive(Debug, Clone, Default, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ElevationDrop {
    /// Which one. [`None`] is every one of them.
    ///
    /// `deny_unknown_fields` above is what keeps that safe: a client that misspells this field would
    /// otherwise have asked to empty the queue and been obeyed.
    #[serde(default)]
    pub op: Option<PendingOpId>,
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::privileged::{ElevationOutcome, PrivilegedOp};
    use crate::{JobId, Timestamp};

    fn waiting() -> PendingOp {
        let op = PrivilegedOp::Probe {};

        PendingOp {
            id: PendingOpId(3),
            description: op.describe(),
            op,
            requested_at: Timestamp(1_760_000_000_000),
        }
    }

    /// A client renders the list without deciding what an operation means, which is what the
    /// pre-rendered `description` is for — `CLAUDE.md`'s "no business logic in clients", applied to
    /// the one screen where getting it wrong means somebody allows the wrong thing.
    #[test]
    fn a_pending_operation_carries_its_own_description() {
        let encoded = serde_json::to_value(waiting()).unwrap();

        assert_eq!(encoded["id"], 3);
        assert_eq!(encoded["op"]["op"], "probe");
        assert!(
            encoded["description"]
                .as_str()
                .is_some_and(|d| !d.is_empty())
        );
        assert_eq!(encoded["requested_at"], 1_760_000_000_000_i64);

        assert_eq!(
            serde_json::from_value::<PendingOp>(encoded).unwrap(),
            waiting()
        );
    }

    /// Flattened, on `JobFinish`'s precedent: one object with one discriminator, not an `outcome`
    /// wrapper a client has to unwrap for this type and for no other.
    #[test]
    fn a_grant_outcome_is_one_flat_object() {
        let grant = GrantOutcome {
            job: JobId(9),
            at: Timestamp(1_760_000_000_000),
            outcome: ElevationOutcome::Declined,
            applied: 0,
            still_pending: 3,
        };

        let encoded = serde_json::to_value(&grant).unwrap();
        assert_eq!(encoded["outcome"], "declined");
        assert_eq!(encoded["still_pending"], 3);
        assert!(encoded.get("reason").is_none());

        assert_eq!(
            serde_json::from_value::<GrantOutcome>(encoded).unwrap(),
            grant
        );
    }

    /// The Linux branch is the one this matters for: `reason` is the whole `pkexec` command a person
    /// is meant to type, and a flattening that lost it would leave them with "unavailable".
    #[test]
    fn an_unavailable_grant_keeps_the_command_to_run_by_hand() {
        let grant = GrantOutcome {
            job: JobId(9),
            at: Timestamp(1_760_000_000_000),
            outcome: ElevationOutcome::Unavailable {
                reason: "no polkit agent; run: pkexec /opt/mixengine/mixengine-elevate /…"
                    .to_owned(),
            },
            applied: 0,
            still_pending: 1,
        };

        let encoded = serde_json::to_value(&grant).unwrap();
        assert_eq!(encoded["outcome"], "unavailable");
        assert!(encoded["reason"].as_str().unwrap().contains("pkexec"));

        assert_eq!(
            serde_json::from_value::<GrantOutcome>(encoded).unwrap(),
            grant
        );
    }

    /// A status with nothing waiting is the ordinary machine, and it puts no nulls on the wire.
    #[test]
    fn a_machine_with_nothing_waiting_says_so_without_nulls() {
        let status = ElevationStatus {
            elevated: false,
            can_prompt: true,
            reason: None,
            helper: Some("/opt/mixengine/mixengine-elevate".to_owned()),
            pending: Vec::new(),
            last: None,
        };

        let encoded = serde_json::to_value(&status).unwrap();
        assert_eq!(encoded["pending"], serde_json::json!([]));
        assert!(encoded.get("reason").is_none(), "{encoded}");
        assert!(encoded.get("last").is_none(), "{encoded}");

        assert_eq!(
            serde_json::from_value::<ElevationStatus>(encoded).unwrap(),
            status
        );
    }

    /// `elevation.drop` with no parameters means "all of them", and that has to be reachable from a
    /// client that sends `{}` as well as from one that sends nothing at all.
    #[test]
    fn a_drop_with_nothing_named_means_all_of_them() {
        assert_eq!(
            serde_json::from_str::<ElevationDrop>("{}").unwrap(),
            ElevationDrop { op: None }
        );
        assert_eq!(
            serde_json::from_str::<ElevationDrop>(r#"{"op":7}"#).unwrap(),
            ElevationDrop {
                op: Some(PendingOpId(7))
            }
        );
        // A typo in the one field this type has must not be read as "drop everything".
        assert!(serde_json::from_str::<ElevationDrop>(r#"{"id":7}"#).is_err());
    }

    #[test]
    fn a_summary_is_three_facts_and_no_more() {
        let summary = ElevationSummary {
            elevated: false,
            can_prompt: true,
            pending: 3,
        };

        assert_eq!(
            serde_json::to_string(&summary).unwrap(),
            r#"{"elevated":false,"can_prompt":true,"pending":3}"#
        );
    }
}
