//! The `jobs` table: what the daemon has been asked to do that is too long to answer inline.
//!
//! This module owns every write to that table and nothing else. What it does *not* own is the work:
//! a job here is a row and a state machine, and the thing that downloads eighty megabytes is the
//! daemon's — the same division [`crate::services`] draws, where this crate persists a transition
//! and the supervisor is what causes one.
//!
//! **Persisted and emitted are one value**, exactly as at T14: [`progress`] and [`finish`] return
//! the [`JobProgress`] and [`JobFinish`] they wrote, and the daemon publishes *those* as
//! [`DaemonEvent::JobProgress`](mixengine_proto::DaemonEvent::JobProgress) and
//! [`JobFinished`](mixengine_proto::DaemonEvent::JobFinished). A move that did not survive its
//! transaction cannot be announced.
//!
//! **There is no `delete`, and no trimming.** `jobs` is the one table in this schema that grows with
//! use, and a home that installs a runtime a week accumulates rows for years. Bounding it is
//! [`JobFilter::limit`](mixengine_proto::JobFilter) — a bound on what is *read* — because deleting
//! history is a decision with a policy behind it (how long, whose, what a support conversation still
//! needs), and inventing that policy here, before anything has produced a single job, would be
//! guessing at it. The `events` table beside it has the same shape and the same answer.

use mixengine_proto::{
    JobFilter, JobFinish, JobId, JobKind, JobOutcome, JobProgress, JobState, JobSummary, Timestamp,
};

use crate::{Error, Result, Store};

/// Start a job, and hand back the row that was written.
///
/// A job is created **running**: there is no queue in front of it and no state before it starts, so
/// a `Pending` would be a state nothing ever leaves and every reader would have to handle. Whoever
/// calls this is about to spawn the work.
///
/// # Errors
///
/// [`Error::Database`] when the row cannot be written.
pub async fn create(store: &Store, kind: &JobKind, at: Timestamp) -> Result<JobSummary> {
    let (kind_text, started, state) = (kind.as_str(), at.0, JobState::Running.as_str());

    let id = sqlx::query_scalar!(
        "INSERT INTO jobs (kind, state, percent, message, started_at)
         VALUES (?, ?, 0, '', ?)
         RETURNING id",
        kind_text,
        state,
        started
    )
    .fetch_one(store.pool())
    .await
    .map_err(|source| store.failure("write", source))?;

    tracing::info!(job = id, kind = kind_text, "a job started");

    Ok(JobSummary {
        id: JobId(id),
        kind: kind.clone(),
        state: JobState::Running,
        percent: 0,
        message: String::new(),
        started_at: at,
        finished_at: None,
        outcome: None,
    })
}

/// Report how far along a running job is.
///
/// **A job that has ended cannot report progress**, and that is enforced here rather than trusted:
/// the work is a task with its own cancellation, so a producer that was cancelled mid-download and
/// reports `60%` on its way out is the ordinary case and not a bug. What would be a bug is the row
/// moving backwards out of `cancelled`, so the write is refused and the caller is told which
/// ending got there first.
///
/// `at` is passed in rather than read from the clock for the reason
/// [`services::transition`](crate::services::transition) takes one: the caller already has a
/// reading, and a test needs to be able to say when.
///
/// # Errors
///
/// [`Error::NotFound`] when there is no such job; [`Error::JobEnded`] when it is over;
/// [`Error::UnknownJobState`] when the row holds a word this build does not recognise; and
/// [`Error::Database`] when the row cannot be written.
pub async fn progress(
    store: &Store,
    job: JobId,
    percent: u8,
    message: String,
    at: Timestamp,
) -> Result<JobProgress> {
    // `BEGIN IMMEDIATE` for the reason `services::transition` gives: the read decides whether the
    // write is allowed, and a deferred `BEGIN` would leave the `UPDATE` to upgrade a read snapshot —
    // which WAL refuses outright, without even running the busy handler.
    let mut tx = store
        .pool()
        .begin_with("BEGIN IMMEDIATE")
        .await
        .map_err(|source| store.failure("write", source))?;

    running(store, &mut tx, job).await?;

    let (id, percent_column) = (job.0, i64::from(percent.min(100)));
    sqlx::query!(
        "UPDATE jobs SET percent = ?, message = ? WHERE id = ? AND state = 'running'",
        percent_column,
        message,
        id
    )
    .execute(&mut *tx)
    .await
    .map_err(|source| store.failure("write", source))?;

    tx.commit()
        .await
        .map_err(|source| store.failure("write", source))?;

    Ok(JobProgress {
        job,
        percent: percent.min(100),
        message,
        at,
    })
}

/// End a job, and hand back the ending that was written.
///
/// The state is [`JobOutcome::state`] and is never passed separately, so a row cannot say
/// `succeeded` beside an outcome carrying an error. The outcome, the state and the moment are one
/// `UPDATE`, which is what the two `CHECK`s on the table assert from the other side.
///
/// # Errors
///
/// As [`progress`]: [`Error::NotFound`], [`Error::JobEnded`] when something already ended it,
/// [`Error::UnknownJobState`], and [`Error::Database`].
pub async fn finish(
    store: &Store,
    job: JobId,
    outcome: JobOutcome,
    at: Timestamp,
) -> Result<JobFinish> {
    let mut tx = store
        .pool()
        .begin_with("BEGIN IMMEDIATE")
        .await
        .map_err(|source| store.failure("write", source))?;

    let from = running(store, &mut tx, job).await?;
    let to = outcome.state();

    // Unreachable while `running` is what precedes it — every ending is reachable from `Running` —
    // and asserted all the same, on `can_become`'s own principle: the machine is the authority, and
    // a caller that has lost track of it should be told rather than obeyed.
    if !from.can_become(to) {
        return Err(Error::IllegalJobTransition {
            job: job.0,
            from,
            to,
        });
    }

    // Cannot fail: `JobOutcome` is one of ours and holds a `serde_json::Value` that was itself
    // decoded from JSON. Mapped rather than unwrapped because nothing in this crate panics.
    let result = serde_json::to_string(&outcome)
        .map_err(|source| Error::JobOutcomeUnwritable { job: job.0, source })?;

    let (id, state, finished) = (job.0, to.as_str(), at.0);
    let updated = sqlx::query!(
        "UPDATE jobs SET state = ?, finished_at = ?, result_json = ?
         WHERE id = ? AND state = 'running'",
        state,
        finished,
        result,
        id
    )
    .execute(&mut *tx)
    .await
    .map_err(|source| store.failure("write", source))?;

    if updated.rows_affected() == 0 {
        return Err(Error::JobEnded {
            job: job.0,
            state: from,
        });
    }

    tx.commit()
        .await
        .map_err(|source| store.failure("write", source))?;

    tracing::info!(job = job.0, %to, "a job ended");

    Ok(JobFinish { job, outcome, at })
}

/// End every job that a daemon which is no longer here was running.
///
/// **A job does not survive the process running it, and that is the difference between this and
/// [`crate::services`]' crash recovery (T18).** A service is a process of its own: it can outlive
/// the daemon that spawned it, so recovery there asks the OS whether it did and adopts what
/// survived. The work behind a job is a task *inside* the daemon — a download, a verification, an
/// unpack — and there is nothing to adopt. A row still saying `running` at boot therefore means
/// exactly one thing: the daemon that was doing it stopped without finishing.
///
/// So it is reconciled rather than resumed, and the ending is [`JobOutcome::Failed`] rather than
/// [`JobOutcome::Cancelled`]: nobody asked for it to stop. What the error says is what a user can
/// act on — the work did not finish and the thing to do is ask again — and the state it leaves
/// behind is what the *next* thing to read this row expects, so nothing downstream has to handle a
/// job that claims to be running with no task behind it.
///
/// Called before the first client is served, for the same reason service recovery is: a `job.list`
/// answered before this ran would show work nobody is doing.
///
/// # Errors
///
/// [`Error::Database`] when the rows cannot be read or written, and [`Error::UnknownJobState`] or
/// [`Error::UnreadableJobRow`] from reading back what was abandoned.
pub async fn abandon(store: &Store, at: Timestamp) -> Result<Vec<JobFinish>> {
    let outcome = JobOutcome::Failed {
        error: mixengine_proto::Error::new(
            mixengine_proto::ErrorCode::Internal,
            "the daemon that was running this stopped before it finished",
        )
        .with_hint("nothing was left half-applied that a retry cannot repeat — ask for it again"),
    };

    // Cannot fail: the value is built two statements above out of types this crate owns.
    let result = serde_json::to_string(&outcome)
        .map_err(|source| Error::JobOutcomeUnwritable { job: 0, source })?;

    let (state, finished) = (JobState::Failed.as_str(), at.0);
    let abandoned = sqlx::query_scalar!(
        "UPDATE jobs SET state = ?, finished_at = ?, result_json = ?
         WHERE state = 'running'
         RETURNING id",
        state,
        finished,
        result
    )
    .fetch_all(store.pool())
    .await
    .map_err(|source| store.failure("write", source))?;

    if !abandoned.is_empty() {
        tracing::warn!(
            jobs = abandoned.len(),
            "jobs were left unfinished by a daemon that stopped, and are marked failed"
        );
    }

    Ok(abandoned
        .into_iter()
        .map(|id| JobFinish {
            job: JobId(id),
            outcome: outcome.clone(),
            at,
        })
        .collect())
}

/// One job's row.
///
/// # Errors
///
/// [`Error::NotFound`] when there is no such job; [`Error::UnknownJobState`] or
/// [`Error::UnreadableJobRow`] when the row holds something this build cannot read back; and
/// [`Error::Database`] when the file cannot be read.
pub async fn record(store: &Store, job: JobId) -> Result<JobSummary> {
    let id = job.0;

    let row = sqlx::query!(
        "SELECT id, kind, state, percent, message, started_at, finished_at, result_json
         FROM jobs WHERE id = ?",
        id
    )
    .fetch_optional(store.pool())
    .await
    .map_err(|source| store.failure("read", source))?
    .ok_or_else(|| Error::NotFound {
        kind: "job",
        id: job.to_string(),
    })?;

    summary(
        row.id,
        row.kind,
        row.state,
        row.percent,
        row.message,
        row.started_at,
        row.finished_at,
        row.result_json,
    )
}

/// The jobs a listing asks for, newest first.
///
/// **Bounded by the caller's [`JobFilter::limit`]**, because this table is the one that grows
/// without end — see the note at the top of this module.
///
/// # Errors
///
/// As [`record`], minus [`Error::NotFound`]: a home that has run no jobs has no rows, which is an
/// answer and not a failure.
pub async fn records(store: &Store, filter: &JobFilter) -> Result<Vec<JobSummary>> {
    let state = filter.state.map(JobState::as_str);
    let limit = i64::from(filter.limit);

    // The same value bound twice rather than a numbered parameter: `?1` is valid SQLite, and it is
    // the sqlx macro's inference that has an opinion about it. Two binds cost nothing here.
    let rows = sqlx::query!(
        "SELECT id, kind, state, percent, message, started_at, finished_at, result_json
         FROM jobs
         WHERE (? IS NULL OR state = ?)
         ORDER BY started_at DESC, id DESC
         LIMIT ?",
        state,
        state,
        limit
    )
    .fetch_all(store.pool())
    .await
    .map_err(|source| store.failure("read", source))?;

    rows.into_iter()
        .map(|row| {
            summary(
                row.id,
                row.kind,
                row.state,
                row.percent,
                row.message,
                row.started_at,
                row.finished_at,
                row.result_json,
            )
        })
        .collect()
}

/// The state of a job that is still going, or the reason it is not one.
///
/// Read inside the caller's transaction, which is what makes "is it still running" and the `UPDATE`
/// that depends on it one decision rather than two.
async fn running(
    store: &Store,
    tx: &mut sqlx::Transaction<'_, sqlx::Sqlite>,
    job: JobId,
) -> Result<JobState> {
    let id = job.0;

    let stored = sqlx::query_scalar!("SELECT state FROM jobs WHERE id = ?", id)
        .fetch_optional(&mut **tx)
        .await
        .map_err(|source| store.failure("read", source))?
        .ok_or_else(|| Error::NotFound {
            kind: "job",
            id: job.to_string(),
        })?;

    let state = parse_state(job, stored)?;

    match state.is_finished() {
        true => Err(Error::JobEnded { job: id, state }),
        false => Ok(state),
    }
}

/// One row, as the type a client renders.
///
/// Every column is checked rather than assumed, because this is where a hand-edited database or a
/// row written by a version that knew more than this one arrives. The two `CHECK`s on the table make
/// the state and the ending agree; what is not expressible in SQL is that `result_json` parses, so
/// that is checked here.
#[expect(
    clippy::too_many_arguments,
    reason = "the columns of one row, listed once, rather than a struct that exists only to be \
              destructured immediately"
)]
fn summary(
    id: i64,
    kind: String,
    state: String,
    percent: i64,
    message: String,
    started_at: i64,
    finished_at: Option<i64>,
    result_json: Option<String>,
) -> Result<JobSummary> {
    let job = JobId(id);
    let state = parse_state(job, state)?;

    let outcome = result_json
        .map(|stored| {
            serde_json::from_str::<JobOutcome>(&stored).map_err(|source| Error::UnreadableJobRow {
                job: id,
                column: "result_json",
                source,
            })
        })
        .transpose()?;

    Ok(JobSummary {
        id: job,
        kind: JobKind::parse(&kind).ok_or_else(|| Error::UnknownJobKind {
            job: id,
            value: kind,
        })?,
        state,
        // The column is `CHECK`ed to 0–100, so nothing this build or any other could write lands
        // outside a `u8`. Clamped rather than unwrapped for the reason `services::process_id`
        // narrows rather than trusting: the alternative to a defensible reading is a panic.
        percent: u8::try_from(percent).unwrap_or(100).min(100),
        message,
        started_at: Timestamp(started_at),
        finished_at: finished_at.map(Timestamp),
        outcome,
    })
}

/// Turn the stored word into a state, blaming the row rather than the reader.
fn parse_state(job: JobId, stored: String) -> Result<JobState> {
    JobState::parse(&stored).ok_or_else(|| Error::UnknownJobState {
        job: job.0,
        value: stored,
    })
}

#[cfg(test)]
mod tests {
    use mixengine_proto::{Error as WireError, ErrorCode};

    use super::*;

    const NOW: Timestamp = Timestamp(1_760_000_000_000);

    async fn store() -> (tempfile::TempDir, Store) {
        let home = tempfile::tempdir().expect("a temporary directory");
        let store = Store::open(&home.path().join(crate::paths::DATABASE_FILE_NAME))
            .await
            .expect("a database");
        (home, store)
    }

    fn kind() -> JobKind {
        JobKind::parse("runtime.install").expect("a valid kind")
    }

    async fn started(store: &Store) -> JobId {
        create(store, &kind(), NOW).await.expect("a job").id
    }

    #[tokio::test]
    async fn a_job_starts_running_with_nothing_to_show_yet() {
        let (_home, store) = store().await;

        let job = create(&store, &kind(), NOW).await.expect("a job");

        assert_eq!(job.state, JobState::Running);
        assert_eq!(job.percent, 0);
        assert_eq!(job.started_at, NOW);
        assert_eq!(job.finished_at, None);
        assert_eq!(job.outcome, None);

        assert_eq!(
            record(&store, job.id).await.expect("the row"),
            job,
            "the value handed back is the value that survived the insert"
        );
    }

    #[tokio::test]
    async fn progress_is_written_and_handed_back() {
        let (_home, store) = store().await;
        let job = started(&store).await;

        let reported = progress(&store, job, 40, "verifying".to_owned(), NOW)
            .await
            .expect("a running job takes progress");

        assert_eq!(reported.percent, 40);
        assert_eq!(reported.message, "verifying");

        let row = record(&store, job).await.expect("the row");
        assert_eq!(row.percent, 40);
        assert_eq!(row.message, "verifying");
        assert_eq!(row.state, JobState::Running);
    }

    #[tokio::test]
    async fn an_ending_writes_the_state_the_outcome_names() {
        let (_home, store) = store().await;
        let job = started(&store).await;

        let outcome = JobOutcome::Succeeded {
            result: serde_json::json!({"version": "8.3.12"}),
        };
        let finish = finish(&store, job, outcome.clone(), Timestamp(NOW.0 + 5_000))
            .await
            .expect("a running job can end");

        assert_eq!(finish.outcome, outcome);

        let row = record(&store, job).await.expect("the row");
        assert_eq!(row.state, JobState::Succeeded, "derived from the outcome");
        assert_eq!(row.finished_at, Some(Timestamp(NOW.0 + 5_000)));
        assert_eq!(row.outcome, Some(outcome));
    }

    /// A failure travels as the wire error, through the column and back, with its hint intact —
    /// which is the whole reason `JobOutcome::Failed` carries one rather than a string.
    #[tokio::test]
    async fn a_failed_job_keeps_the_error_a_client_would_have_been_answered_with() {
        let (_home, store) = store().await;
        let job = started(&store).await;

        let error = WireError::new(ErrorCode::Io, "the checksum did not match")
            .with_hint("install again, or report the mirror");
        finish(
            &store,
            job,
            JobOutcome::Failed {
                error: error.clone(),
            },
            NOW,
        )
        .await
        .expect("a running job can fail");

        let row = record(&store, job).await.expect("the row");
        assert_eq!(row.state, JobState::Failed);
        assert_eq!(row.outcome, Some(JobOutcome::Failed { error }));
    }

    /// The percentage a job stopped at is what the row keeps: it is the number that says where to
    /// look, and an ending does not rewrite it.
    #[tokio::test]
    async fn an_ending_leaves_the_percentage_where_the_job_got_to() {
        let (_home, store) = store().await;
        let job = started(&store).await;

        progress(&store, job, 40, "verifying".to_owned(), NOW)
            .await
            .expect("progress");
        finish(&store, job, JobOutcome::Cancelled, NOW)
            .await
            .expect("an ending");

        assert_eq!(record(&store, job).await.expect("the row").percent, 40);
    }

    /// A producer cancelled mid-work and reporting on its way out is the ordinary case, not a bug —
    /// and the row must not move backwards out of the ending that got there first.
    #[tokio::test]
    async fn a_job_that_has_ended_takes_no_more_progress() {
        let (_home, store) = store().await;
        let job = started(&store).await;

        finish(&store, job, JobOutcome::Cancelled, NOW)
            .await
            .expect("an ending");

        let error = progress(&store, job, 60, "still going".to_owned(), NOW)
            .await
            .expect_err("it is over");

        assert!(
            matches!(
                error,
                Error::JobEnded {
                    state: JobState::Cancelled,
                    ..
                }
            ),
            "{error:?}"
        );
        assert_eq!(
            record(&store, job).await.expect("the row").percent,
            0,
            "the refused write rolled back rather than half-applying"
        );
    }

    /// A job ends once. Two producers racing to end the same one — the work finishing as a cancel
    /// arrives — is exactly the race the compare-and-swap is here for.
    #[tokio::test]
    async fn a_job_ends_once_and_the_second_ending_is_refused() {
        let (_home, store) = store().await;
        let job = started(&store).await;

        finish(&store, job, JobOutcome::Cancelled, NOW)
            .await
            .expect("the first ending");

        let error = finish(
            &store,
            job,
            JobOutcome::Succeeded {
                result: serde_json::Value::Null,
            },
            NOW,
        )
        .await
        .expect_err("it already ended");

        assert!(
            matches!(
                error,
                Error::JobEnded {
                    state: JobState::Cancelled,
                    ..
                }
            ),
            "{error:?}"
        );
        assert_eq!(
            record(&store, job).await.expect("the row").state,
            JobState::Cancelled,
            "the ending that got there first is the one that stands"
        );
    }

    /// A job has no process of its own, so a row still saying `running` at boot is work nobody is
    /// doing. Reconciled rather than resumed, and as a failure rather than a cancellation: nobody
    /// asked for it to stop.
    #[tokio::test]
    async fn a_job_left_behind_by_a_daemon_that_stopped_is_failed_at_the_next_boot() {
        let (_home, store) = store().await;
        let unfinished = started(&store).await;
        let ended = started(&store).await;

        finish(&store, ended, JobOutcome::Cancelled, NOW)
            .await
            .expect("an ending");

        let abandoned = abandon(&store, Timestamp(NOW.0 + 1_000))
            .await
            .expect("recovery runs before the first client");

        assert_eq!(
            abandoned
                .iter()
                .map(|finish| finish.job)
                .collect::<Vec<_>>(),
            [unfinished],
            "only the one nobody was doing"
        );

        let row = record(&store, unfinished).await.expect("the row");
        assert_eq!(row.state, JobState::Failed);
        assert_eq!(row.finished_at, Some(Timestamp(NOW.0 + 1_000)));
        assert!(
            matches!(row.outcome, Some(JobOutcome::Failed { .. })),
            "{row:?}"
        );

        assert_eq!(
            record(&store, ended).await.expect("the row").state,
            JobState::Cancelled,
            "a job that had already ended is left exactly as it ended"
        );
    }

    /// A boot with nothing to reconcile writes nothing, which is what makes the step safe to run on
    /// every start.
    #[tokio::test]
    async fn a_boot_with_no_unfinished_jobs_reconciles_nothing() {
        let (_home, store) = store().await;

        let abandoned = abandon(&store, NOW).await.expect("a clean home");

        assert!(abandoned.is_empty(), "{abandoned:?}");
    }

    #[tokio::test]
    async fn a_job_that_is_not_there_is_named_as_such() {
        let (_home, store) = store().await;

        let error = record(&store, JobId(404)).await.expect_err("no such row");

        assert!(
            matches!(&error, Error::NotFound { kind: "job", id } if id == "#404"),
            "{error:?}"
        );
    }

    #[tokio::test]
    async fn a_listing_is_newest_first_and_bounded_by_what_was_asked_for() {
        let (_home, store) = store().await;

        for offset in 0..5 {
            create(&store, &kind(), Timestamp(NOW.0 + offset))
                .await
                .expect("a job");
        }

        let listed = records(
            &store,
            &JobFilter {
                state: None,
                limit: 3,
            },
        )
        .await
        .expect("a listing");

        let moments: Vec<i64> = listed.iter().map(|job| job.started_at.0).collect();
        assert_eq!(moments, [NOW.0 + 4, NOW.0 + 3, NOW.0 + 2], "newest first");
    }

    #[tokio::test]
    async fn a_listing_can_ask_for_one_state() {
        let (_home, store) = store().await;
        let ended = started(&store).await;
        let running = started(&store).await;

        finish(&store, ended, JobOutcome::Cancelled, NOW)
            .await
            .expect("an ending");

        let listed = records(
            &store,
            &JobFilter {
                state: Some(JobState::Running),
                limit: 50,
            },
        )
        .await
        .expect("a listing");

        let ids: Vec<JobId> = listed.iter().map(|job| job.id).collect();
        assert_eq!(ids, [running], "only the one still going");
    }

    #[tokio::test]
    async fn a_home_that_has_run_no_jobs_lists_nothing_rather_than_failing() {
        let (_home, store) = store().await;

        let listed = records(&store, &JobFilter::default())
            .await
            .expect("an empty home is an answer");

        assert!(listed.is_empty(), "{listed:?}");
    }

    /// The constraint and the enum have to agree, or one of them is decoration.
    #[tokio::test]
    async fn the_column_accepts_every_state_and_nothing_else() {
        let (_home, store) = store().await;

        for state in JobState::ALL {
            // A running job has no ending and a finished one has both halves of it, which is what
            // the table's two paired CHECKs say. Written by hand here because the point is the
            // column, not the module above it.
            let (finished, result) = match state.is_finished() {
                true => (Some(NOW.0), Some(r#"{"ending":"cancelled"}"#)),
                false => (None, None),
            };

            sqlx::query(
                "INSERT INTO jobs (kind, state, started_at, finished_at, result_json)
                 VALUES ('runtime.install', ?, ?, ?, ?)",
            )
            .bind(state.as_str())
            .bind(NOW.0)
            .bind(finished)
            .bind(result)
            .execute(store.pool())
            .await
            .unwrap_or_else(|error| panic!("the column refused {state}: {error}"));
        }

        let refused = sqlx::query(
            "INSERT INTO jobs (kind, state, started_at) VALUES ('runtime.install', 'done', ?)",
        )
        .bind(NOW.0)
        .execute(store.pool())
        .await;

        assert!(
            refused.is_err(),
            "the CHECK let a word through that JobState cannot read back"
        );
    }

    /// The paired `CHECK`s are what stop a third thing that never happens: a job still going with a
    /// result, or one that ended with nothing to show.
    #[tokio::test]
    async fn a_row_cannot_say_it_is_running_and_finished_at_once() {
        let (_home, store) = store().await;

        for (state, finished, result) in [
            ("running", Some(NOW.0), Some(r#"{"ending":"cancelled"}"#)),
            ("cancelled", None, None),
            ("succeeded", Some(NOW.0), None),
        ] {
            let refused = sqlx::query(
                "INSERT INTO jobs (kind, state, started_at, finished_at, result_json)
                 VALUES ('runtime.install', ?, ?, ?, ?)",
            )
            .bind(state)
            .bind(NOW.0)
            .bind(finished)
            .bind(result)
            .execute(store.pool())
            .await;

            assert!(refused.is_err(), "{state} with {finished:?}/{result:?}");
        }
    }

    /// What a hand-edited database looks like from in here, asked of the function rather than
    /// through a doctored row — the `CHECK` makes the row unreachable through any write of ours, and
    /// producing one would be a test about SQLite's schema cache. Same reasoning as
    /// `services::tests::a_state_this_build_does_not_know_blames_the_row`.
    #[test]
    fn a_state_this_build_does_not_know_blames_the_row() {
        let error = parse_state(JobId(3), "done".to_owned()).expect_err("not a job state");

        assert!(
            matches!(&error, Error::UnknownJobState { job: 3, value } if value == "done"),
            "{error:?}"
        );
    }

    /// `result_json` is the one column no `CHECK` can constrain, so it is the one a corrupt row
    /// reaches this module through.
    #[test]
    fn an_outcome_that_does_not_parse_names_the_column() {
        let error = summary(
            3,
            "runtime.install".to_owned(),
            "succeeded".to_owned(),
            100,
            String::new(),
            NOW.0,
            Some(NOW.0),
            Some("not json".to_owned()),
        )
        .expect_err("the column holds something that is not an outcome");

        assert!(
            matches!(
                &error,
                Error::UnreadableJobRow {
                    job: 3,
                    column: "result_json",
                    ..
                }
            ),
            "{error:?}"
        );
    }
}
