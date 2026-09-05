//! What `job.*` answers, where [`crate::job`] is the vocabulary a job is *described* in.
//!
//! The same split [`crate::service_api`] draws over [`crate::service`]: one module holds the state
//! machine and the values a producer writes, this one holds the shapes a client asks with and
//! renders. A client renders these and never constructs a [`JobUpdate`](crate::JobUpdate) — that is
//! the daemon's side of the wall.

use crate::{JobId, JobKind, JobOutcome, JobState, Millis, Timestamp};

/// Which job a call is about.
///
/// One params type for `job.status` and `job.cancel`, because the question each asks is the same
/// one, and the id is **required** in both: a status with no subject is a `job.list` that was typed
/// wrongly, and a cancel with no subject is not a request anybody should be able to make by
/// accident — see [`ServiceQuery`](crate::ServiceQuery), which is required for the first reason and
/// where this one adds the second.
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS), ts(export))]
pub struct JobQuery {
    /// The job to describe, or to cancel.
    pub job: JobId,
}

/// Which job to wait for, and how long to wait.
///
/// **`job.wait` is the one call in this API that blocks on purpose**, and the timeout is what keeps
/// that from contradicting the rule it is an exception to. `.claude/architecture/daemon-and-ipc.md`
/// says never to block an RPC call for minutes; a script that has nothing to do but wait for a
/// download is the case that rule was not written about, and it still does not get to hold a
/// connection open forever. What comes back when the time runs out is the job as it stands, not an
/// error: [`JobSummary::state`] says whether it finished, and a caller that wants to keep waiting
/// calls again.
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS), ts(export))]
pub struct JobWait {
    /// The job to wait for.
    pub job: JobId,

    /// How long to wait before answering with whatever the job is doing then.
    ///
    /// Defaults to thirty seconds, which is long enough that a short job is answered when it
    /// finishes and short enough that nothing holds a connection past a person's patience. The
    /// daemon bounds it: a client asking for an hour is answered by the daemon's own ceiling rather
    /// than being trusted with the socket.
    #[serde(default = "JobWait::default_timeout")]
    pub timeout: Millis,
}

impl JobWait {
    /// The wait a client that says nothing gets.
    fn default_timeout() -> Millis {
        Millis::from_secs(30)
    }
}

/// Which jobs `job.list` should answer with.
///
/// Every field has a default, so `job.list` with no parameters is a question a person can type.
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS), ts(export))]
pub struct JobFilter {
    /// Only jobs in this state, or all of them.
    ///
    /// `{"state":"running"}` is the one a GUI asks on every refresh — what is happening right now —
    /// and it is a filter rather than its own method because "running" is not special to the daemon,
    /// only to the client that is drawing a progress list.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub state: Option<JobState>,

    /// At most this many, newest first.
    ///
    /// **Bounded because the table is not.** `jobs` keeps every job a home has ever run, and a
    /// listing that answered with all of them would grow without limit on a machine that installs a
    /// runtime a week for two years. Fifty is what a person scrolls; a caller that wants the
    /// history asks for more.
    #[serde(default = "JobFilter::default_limit")]
    pub limit: u32,
}

impl JobFilter {
    /// How many a client that says nothing gets.
    fn default_limit() -> u32 {
        50
    }
}

impl Default for JobFilter {
    fn default() -> Self {
        Self {
            state: None,
            limit: Self::default_limit(),
        }
    }
}

/// What `job.list` answers.
///
/// An object around the list rather than a bare array, on [`ServiceList`](crate::ServiceList)'s
/// precedent: a field can be added beside it without changing every existing client's parser.
#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS), ts(export))]
pub struct JobList {
    /// The jobs that matched, newest first.
    pub jobs: Vec<JobSummary>,
}

/// One job, as the `jobs` row describes it. The whole of what `job.status` answers.
///
/// One type for the listing, the single lookup, the wait and the cancel, on
/// [`ServiceSummary`](crate::ServiceSummary)'s precedent: all four are the same sentence about a
/// job, so a client renders them with one function.
#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS), ts(export))]
pub struct JobSummary {
    /// Which job.
    pub id: JobId,

    /// What kind of work it is — the method that produced it.
    pub kind: JobKind,

    /// Where it is.
    pub state: JobState,

    /// How far along, as last reported. Stays at whatever it reached when the job ended, rather than
    /// being snapped to 100: a job that failed at 40% failed at 40%, and rewriting that would erase
    /// the one number that says where to look.
    pub percent: u8,

    /// What it last said it was doing. Empty for a job that has not reported anything yet.
    pub message: String,

    /// When it started.
    pub started_at: Timestamp,

    /// When it ended, or [`None`] while it is still going.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub finished_at: Option<Timestamp>,

    /// What it produced, once it has.
    ///
    /// [`None`] exactly while [`JobSummary::state`] is [`JobState::Running`] — the two are written
    /// together by the same transaction, so a client may rely on that rather than checking both.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub outcome: Option<JobOutcome>,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_wait_with_no_timeout_gets_the_default_one() {
        let wait: JobWait = serde_json::from_str(r#"{"job":3}"#).expect("only the id is required");

        assert_eq!(wait.job, JobId(3));
        assert_eq!(wait.timeout, Millis::from_secs(30));
    }

    #[test]
    fn a_wait_without_a_job_does_not_decode() {
        serde_json::from_str::<JobWait>("{}").expect_err("there is no job to wait for");
    }

    /// A cancel with no subject is not something anybody should be able to send by accident.
    #[test]
    fn a_query_without_a_job_does_not_decode() {
        serde_json::from_str::<JobQuery>("{}").expect_err("no subject");
    }

    #[test]
    fn a_listing_with_no_parameters_is_a_question_a_person_can_type() {
        let filter: JobFilter = serde_json::from_str("{}").expect("every field has a default");

        assert_eq!(filter, JobFilter::default());
        assert_eq!(filter.state, None, "every state");
        assert_eq!(filter.limit, 50, "bounded, because the table is not");
    }

    #[test]
    fn a_running_job_carries_neither_an_ending_nor_an_outcome() {
        let summary = JobSummary {
            id: JobId(1),
            kind: JobKind::parse("runtime.install").expect("a valid kind"),
            state: JobState::Running,
            percent: 40,
            message: "downloading php 8.3.12".to_owned(),
            started_at: Timestamp(1_760_000_000_000),
            finished_at: None,
            outcome: None,
        };

        let encoded = serde_json::to_value(&summary).unwrap();
        assert!(encoded.get("finished_at").is_none(), "{encoded}");
        assert!(encoded.get("outcome").is_none(), "{encoded}");
        assert_eq!(
            serde_json::from_value::<JobSummary>(encoded).unwrap(),
            summary
        );
    }

    /// The percentage a job stopped at is kept rather than snapped to 100, because it is the number
    /// that says where the failure was.
    #[test]
    fn a_job_that_failed_keeps_the_percentage_it_failed_at() {
        let summary = JobSummary {
            id: JobId(2),
            kind: JobKind::parse("runtime.install").expect("a valid kind"),
            state: JobState::Failed,
            percent: 40,
            message: "verifying the download".to_owned(),
            started_at: Timestamp(1_760_000_000_000),
            finished_at: Some(Timestamp(1_760_000_009_000)),
            outcome: Some(JobOutcome::Failed {
                error: crate::Error::new(crate::ErrorCode::Io, "the checksum did not match"),
            }),
        };

        let encoded = serde_json::to_value(&summary).unwrap();
        assert_eq!(encoded["percent"], 40);
        assert_eq!(encoded["outcome"]["ending"], "failed");
        assert_eq!(
            serde_json::from_value::<JobSummary>(encoded).unwrap(),
            summary
        );
    }
}
