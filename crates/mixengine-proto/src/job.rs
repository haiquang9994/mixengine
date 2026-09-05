//! Long operations that outlive the call that asked for them.
//!
//! `.claude/architecture/daemon-and-ipc.md` states the rule this module exists to serve: *long
//! operations return a job*, because a download of eighty megabytes cannot be an RPC call a client
//! waits inside. What comes back from `runtime.install` is a [`JobId`]; what happens afterwards
//! arrives as [`crate::DaemonEvent::JobProgress`] and [`crate::DaemonEvent::JobFinished`], and
//! `job.wait` is there for a script that has nothing to do but wait.
//!
//! Three types and the split is the one [`crate::state`] draws for services: [`JobState`] is where a
//! job is, [`JobOutcome`] is what it ended up producing, and [`JobUpdate`] is a move travelling as
//! one value — persisted by `mixengine-core` and published by the daemon from that same value, so
//! the `jobs` row and the event cannot disagree about what happened.
//!
//! **What is deliberately absent is a list of job kinds.** [`JobKind`] is a validated string and not
//! an enum, for the reason `packages.name` is not `CHECK`ed in the first migration: the set grows
//! with every phase that has something long to do — and, from T80, with every extension that ships
//! one — so a closed vocabulary here would be a schema change per feature and a wire break per
//! extension. What keeps it honest instead is that a kind *is* the method that produced it.

use crate::{Error, Timestamp};

/// Which job. The rowid of its `jobs` row, and nothing more.
///
/// A number rather than the human-stable string a [`ServiceId`](crate::ServiceId) is, and the
/// difference is what the two identify: a service is named by whoever declared it and is typed by a
/// user for years, while a job is one run of one operation that nobody names and nobody types twice.
/// The database already mints exactly this — `jobs.id INTEGER PRIMARY KEY` is SQLite's rowid — so
/// generating an id of our own would be a second identity to keep in step with the first.
///
/// `i64` and not `u64` because that is what the column holds; SQLite has no unsigned integer, and a
/// type that could express a value the store cannot round-trip would be a type that lies.
#[derive(
    Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, serde::Serialize, serde::Deserialize,
)]
#[serde(transparent)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS), ts(export))]
pub struct JobId(pub i64);

impl std::fmt::Display for JobId {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "#{}", self.0)
    }
}

/// What kind of work a job is doing — `"runtime.install"`, `"cert.issue"`.
///
/// **A kind is the method that produced the job**, and that is a rule rather than a convention: it
/// makes the vocabulary self-maintaining, since a method is already a name this project has agreed
/// on and already has documentation a user can be sent to. The alternative — a second vocabulary of
/// job names beside the method names — is two lists to keep in step, and the day they disagree the
/// GUI shows a job kind no page in the manual mentions.
///
/// Validated rather than free text, because it is stored and shown: a kind with a newline in it is a
/// listing a person cannot read, and one built from user input is a value nothing else in the
/// database allows.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash, serde::Serialize)]
#[serde(transparent)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS), ts(export))]
pub struct JobKind(String);

impl JobKind {
    /// The longest a kind may be. Generous for `namespace.verb`, and short enough that a row cannot
    /// be used as storage for something that is not a name.
    const LIMIT: usize = 64;

    /// Read a kind, or refuse it.
    ///
    /// Accepts what a method name is made of — lower-case letters, digits, `.`, `_` and `-` — and
    /// nothing else. An `Option` rather than a `Result` for the reason
    /// [`ServiceState::parse`](crate::ServiceState::parse) returns one: the caller knows why it
    /// matters, and the same refusal is a corrupt row to the store and a peer speaking a newer
    /// protocol to a client.
    #[must_use]
    pub fn parse(value: &str) -> Option<Self> {
        let shaped = !value.is_empty()
            && value.len() <= Self::LIMIT
            && value.chars().all(|c| {
                c.is_ascii_lowercase() || c.is_ascii_digit() || matches!(c, '.' | '_' | '-')
            });

        shaped.then(|| Self(value.to_owned()))
    }

    /// The kind as it is stored and shown.
    #[must_use]
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl std::fmt::Display for JobKind {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.0)
    }
}

impl<'de> serde::Deserialize<'de> for JobKind {
    /// Refuses on the way in, so a kind that reached a client is one a client can render.
    fn deserialize<D: serde::Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        let value = String::deserialize(deserializer)?;

        Self::parse(&value).ok_or_else(|| {
            serde::de::Error::custom(format!(
                "`{value}` is not a job kind: lower-case letters, digits, `.`, `_` and `-`, \
                 at most {} characters",
                Self::LIMIT
            ))
        })
    }
}

/// Where a job is.
///
/// **Closed, like [`ServiceState`](crate::ServiceState) and for the same reason**: this is a state
/// machine, and one with room for a state nobody has enumerated is one nobody can reason about. The
/// wire form is the snake_case name and it is also exactly what `jobs.state` holds — one spelling,
/// written by [`JobState::as_str`] and read back by [`JobState::parse`], with the column's `CHECK`
/// carrying the same closed list.
///
/// **There is no `Cancelling`, and that is a decision rather than an omission.** Cancellation here
/// is cooperative: `job.cancel` cancels the token the work is watching, and the work ends when it
/// next looks. A state between the asking and the ending would have to be written by every producer
/// of a job — and this build has none yet, so the only thing that could justify it is a guess about
/// what T21's download will need. `Running` with a cancellation already requested is what a client
/// sees, which is the truth: the work is still going.
#[derive(
    Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, serde::Serialize, serde::Deserialize,
)]
#[serde(rename_all = "snake_case")]
#[cfg_attr(feature = "ts", derive(ts_rs::TS), ts(export))]
pub enum JobState {
    /// Started, and still going. The state a job is created in — there is no queue in front of it.
    Running,
    /// It did what it was asked to do.
    Succeeded,
    /// It could not. [`JobOutcome::Failed`] carries the error a client renders.
    Failed,
    /// Somebody asked it to stop, and it stopped.
    ///
    /// Distinct from [`JobState::Failed`] the way
    /// [`ServiceState::Stopped`](crate::ServiceState::Stopped) is distinct from
    /// [`ServiceState::Failed`](crate::ServiceState::Failed): intent. Nothing went wrong, so nothing
    /// should be reported as having gone wrong, and a script that cancelled a download should not be
    /// told its download broke.
    Cancelled,
}

impl JobState {
    /// Every state. Exists so a test can be exhaustive without restating the list — the migration's
    /// `CHECK` and the serde form are both checked against this.
    pub const ALL: [Self; 4] = [
        Self::Running,
        Self::Succeeded,
        Self::Failed,
        Self::Cancelled,
    ];

    /// The one spelling: the wire form, and the text in `jobs.state`.
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Running => "running",
            Self::Succeeded => "succeeded",
            Self::Failed => "failed",
            Self::Cancelled => "cancelled",
        }
    }

    /// Whether this job is over, whatever it ended as.
    ///
    /// The question `job.wait` answers and the one a client polls on, which is why it is here rather
    /// than written out as a `matches!` at each of those places: three states are terminal, and a
    /// fourth added later must not silently read as "still going" in a loop that waits forever.
    #[must_use]
    pub const fn is_finished(self) -> bool {
        !matches!(self, Self::Running)
    }

    /// Read back what [`JobState::as_str`] wrote, or `None`.
    #[must_use]
    pub fn parse(value: &str) -> Option<Self> {
        Self::ALL.into_iter().find(|state| state.as_str() == value)
    }

    /// Whether the machine may move from `self` to `next`.
    ///
    /// A job's machine is almost too small to be one: it starts running and it ends, once. What the
    /// function is for is the *once* — a finished job cannot be finished a second time, so a
    /// producer that reports success after a cancellation is a bug reported rather than a row
    /// rewritten, and a `job.cancel` arriving at a job that failed a moment ago changes nothing.
    #[must_use]
    pub const fn can_become(self, next: Self) -> bool {
        matches!(
            (self, next),
            (
                Self::Running,
                Self::Succeeded | Self::Failed | Self::Cancelled
            )
        )
    }
}

impl std::fmt::Display for JobState {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.as_str())
    }
}

/// What a finished job produced.
///
/// Stored in `jobs.result_json` and carried by [`crate::DaemonEvent::JobFinished`], which is why it
/// is one type rather than a pair of nullable columns: a job that succeeded has a result and no
/// error, one that failed has an error and no result, and two independent options can express a
/// third thing that never happens.
///
/// **The success payload is a [`serde_json::Value`]**, deliberately and unusually for this crate.
/// Every other wire type here is named, and this one cannot be: the shape of what a job produces
/// belongs to the method that produced it — an install answers with a version and a path, a
/// certificate issue with a fingerprint — and naming them all here would make `mixengine-proto`
/// depend on every feature in the roadmap. The typed half stays with the caller, which knows which
/// job it asked for and can deserialise accordingly.
/// The discriminator is `ending` rather than `outcome`, and that is not a spelling preference.
/// [`JobFinish`] flattens this type, so the tag lands beside `job` and `at` on the wire — where
/// `kind` would read as the job's kind (`"runtime.install"`) and `outcome` would nest as
/// `"outcome": {"outcome": …}` in [`JobSummary`](crate::JobSummary). One word that means the same
/// thing in both places is worth more than either.
#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
#[serde(tag = "ending", rename_all = "snake_case")]
#[non_exhaustive]
#[cfg_attr(feature = "ts", derive(ts_rs::TS), ts(export))]
pub enum JobOutcome {
    /// It worked, and this is what it produced.
    Succeeded {
        /// Whatever the method that owns this job documents. `null` for one that produces nothing
        /// but the fact of having finished.
        result: serde_json::Value,
    },

    /// It did not work.
    Failed {
        /// The same wire error the call would have answered with had it been short enough to answer
        /// inline — written once, by the layer that knows what went wrong, and rendered by a client
        /// exactly as any other failure is. A job is not a second error vocabulary.
        error: Error,
    },

    /// Somebody cancelled it. Carries nothing: there is nothing to say beyond the state.
    Cancelled,
}

impl JobOutcome {
    /// The state a job ends in when it ends this way.
    ///
    /// The two are written together and must agree — a row saying `succeeded` beside an outcome
    /// carrying an error is a row that describes two different events — so the state is *derived*
    /// here rather than passed alongside, and there is no way to write the pair inconsistently.
    #[must_use]
    pub const fn state(&self) -> JobState {
        match self {
            Self::Succeeded { .. } => JobState::Succeeded,
            Self::Failed { .. } => JobState::Failed,
            Self::Cancelled => JobState::Cancelled,
        }
    }
}

/// How far along a running job is.
///
/// Its own type because it is what [`crate::DaemonEvent::JobProgress`] carries and what `jobs`
/// stores in two columns, and the pair is meaningless split: `40` with no sentence is a number a
/// user cannot act on, and `"verifying the download"` with no number is a progress bar that does not
/// move.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS), ts(export))]
pub struct JobProgress {
    /// Which job.
    pub job: JobId,

    /// How far, as a percentage.
    ///
    /// A `u8` rather than a float, and clamped to 0–100 by the column's own `CHECK`: a progress bar
    /// is drawn to the nearest pixel and no caller has ever needed a fraction of a percent. A job
    /// that genuinely cannot say — a download whose server sent no `Content-Length` — reports `0`
    /// and says so in the message, which is honest where an invented number is not.
    pub percent: u8,

    /// What it is doing right now, in one short clause a client puts beside the bar.
    ///
    /// Written by the producer and shown verbatim, so it is the producer's job to make it a sentence
    /// rather than a debug line. Empty is allowed and means "no change to report".
    pub message: String,

    /// When this was read, by the producer that reported it.
    pub at: Timestamp,
}

/// A job ending: which one, how, and when.
///
/// The value `mixengine-core` returns from the transaction that wrote it, and the value
/// [`crate::DaemonEvent::JobFinished`] carries — one description used twice, on
/// [`ServiceTransition`](crate::ServiceTransition)'s precedent, so an ending that was not persisted
/// cannot be announced.
#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS), ts(export))]
pub struct JobFinish {
    /// Which job.
    pub job: JobId,

    /// What it produced. The state is [`JobOutcome::state`] and is not carried separately, because
    /// two fields that must agree are two fields that can disagree.
    #[serde(flatten)]
    pub outcome: JobOutcome,

    /// When it ended.
    pub at: Timestamp,
}

/// One move of a job: progress, or the end of it.
///
/// What a producer hands the daemon, so that the *writing* of a job's every move goes through one
/// door. A job that is over can no longer report progress, and this is the type that makes that
/// expressible rather than a rule each producer has to remember.
#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
#[serde(untagged)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS), ts(export))]
pub enum JobUpdate {
    /// It is still going, and here is how far.
    Progress(JobProgress),
    /// It is over.
    Finished(JobFinish),
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ErrorCode;

    /// The rule the module rests on: one spelling for the wire and for `jobs.state`. A
    /// `#[serde(rename)]` on a single variant would otherwise store a word nothing reads back.
    #[test]
    fn the_stored_spelling_is_the_wire_spelling() {
        for state in JobState::ALL {
            let encoded = serde_json::to_string(&state).unwrap();

            assert_eq!(
                encoded,
                format!(r#""{}""#, state.as_str()),
                "{state:?} is written differently by serde than by as_str"
            );
            assert_eq!(JobState::parse(state.as_str()), Some(state));
        }
    }

    #[test]
    fn a_word_that_is_not_a_job_state_is_not_guessed_at() {
        assert_eq!(JobState::parse("Running"), None, "lowercase only");
        assert_eq!(JobState::parse("done"), None);
        assert_eq!(JobState::parse(""), None);
    }

    /// Every terminal state is terminal, and the one running state is not. `job.wait` loops on this,
    /// so a state that answered wrongly here would be a script that never returns.
    #[test]
    fn a_job_is_finished_in_every_state_but_one() {
        let unfinished: Vec<_> = JobState::ALL
            .into_iter()
            .filter(|state| !state.is_finished())
            .collect();

        assert_eq!(unfinished, [JobState::Running]);
    }

    /// A job ends once. Nothing may leave a terminal state, or a cancelled download could be
    /// reported as having succeeded by work that had not noticed it was over.
    #[test]
    fn a_finished_job_cannot_move_again() {
        for state in JobState::ALL
            .into_iter()
            .filter(|state| state.is_finished())
        {
            for next in JobState::ALL {
                assert!(
                    !state.can_become(next),
                    "a {state} job could still become {next}"
                );
            }
        }
    }

    #[test]
    fn a_running_job_can_reach_every_ending_and_nothing_else() {
        let endings: Vec<_> = JobState::ALL
            .into_iter()
            .filter(|state| JobState::Running.can_become(*state))
            .collect();

        assert_eq!(
            endings,
            [JobState::Succeeded, JobState::Failed, JobState::Cancelled]
        );
    }

    /// The state is derived from the outcome and never written beside it, so the two cannot
    /// disagree. This is the test that says so.
    #[test]
    fn an_outcome_decides_the_state_it_ends_in() {
        assert_eq!(
            JobOutcome::Succeeded {
                result: serde_json::Value::Null
            }
            .state(),
            JobState::Succeeded
        );
        assert_eq!(
            JobOutcome::Failed {
                error: Error::new(ErrorCode::Io, "the disk filled up")
            }
            .state(),
            JobState::Failed
        );
        assert_eq!(JobOutcome::Cancelled.state(), JobState::Cancelled);
    }

    /// A finish is one flat object, like every other event payload: the GUI's single `onmessage`
    /// handler must not have to unwrap a nested object for this one variant.
    #[test]
    fn a_finish_arrives_as_one_flat_object() {
        let finish = JobFinish {
            job: JobId(7),
            outcome: JobOutcome::Succeeded {
                result: serde_json::json!({"version": "8.3.12"}),
            },
            at: Timestamp(1_760_000_000_000),
        };

        let encoded = serde_json::to_string(&finish).unwrap();
        assert_eq!(
            encoded,
            r#"{"job":7,"ending":"succeeded","result":{"version":"8.3.12"},"at":1760000000000}"#
        );
        assert_eq!(serde_json::from_str::<JobFinish>(&encoded).unwrap(), finish);
    }

    /// A failed job carries the same wire error any other failure does — a client renders it with
    /// the code and hint it already knows, rather than with a second vocabulary invented for jobs.
    #[test]
    fn a_failure_travels_as_the_wire_error() {
        let finish = JobFinish {
            job: JobId(3),
            outcome: JobOutcome::Failed {
                error: Error::new(ErrorCode::Io, "the download was cut short")
                    .with_hint("check the connection and install again"),
            },
            at: Timestamp(1_760_000_000_000),
        };

        let encoded = serde_json::to_value(&finish).unwrap();
        assert_eq!(encoded["ending"], "failed");
        assert_eq!(encoded["error"]["code"], "io");
        assert_eq!(
            serde_json::from_value::<JobFinish>(encoded).unwrap(),
            finish
        );
    }

    #[test]
    fn a_kind_is_a_method_name_and_refuses_anything_else() {
        assert_eq!(
            JobKind::parse("runtime.install").map(|kind| kind.as_str().to_owned()),
            Some("runtime.install".to_owned())
        );

        for refused in [
            "",
            "Runtime.Install",
            "runtime install",
            "runtime.install\n",
            "runtime/install",
        ] {
            assert_eq!(JobKind::parse(refused), None, "{refused:?} was accepted");
        }

        assert_eq!(
            JobKind::parse(&"a".repeat(JobKind::LIMIT + 1)),
            None,
            "a kind is a name, not storage"
        );
    }

    /// Refused on the way in as well as at `parse`, or a hand-written client could put a value in a
    /// listing that no client — including the GUI — can lay out.
    #[test]
    fn a_kind_that_is_not_one_does_not_decode() {
        serde_json::from_str::<JobKind>(r#""runtime install""#)
            .expect_err("a space is not part of a method name");
    }

    /// An update is untagged, so a producer's report reads as what it is on the wire rather than
    /// carrying a discriminator that repeats what `outcome` already says.
    #[test]
    fn an_update_reads_back_as_the_half_it_was_written_as() {
        let progress = JobUpdate::Progress(JobProgress {
            job: JobId(1),
            percent: 40,
            message: "verifying the download".to_owned(),
            at: Timestamp(1_760_000_000_000),
        });

        let encoded = serde_json::to_string(&progress).unwrap();
        assert_eq!(
            encoded,
            r#"{"job":1,"percent":40,"message":"verifying the download","at":1760000000000}"#
        );
        assert_eq!(
            serde_json::from_str::<JobUpdate>(&encoded).unwrap(),
            progress
        );

        let finished = JobUpdate::Finished(JobFinish {
            job: JobId(1),
            outcome: JobOutcome::Cancelled,
            at: Timestamp(1_760_000_000_001),
        });

        let encoded = serde_json::to_string(&finished).unwrap();
        assert_eq!(
            serde_json::from_str::<JobUpdate>(&encoded).unwrap(),
            finished
        );
    }
}
