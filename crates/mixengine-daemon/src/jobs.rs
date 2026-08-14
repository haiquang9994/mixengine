//! The jobs this daemon is running, and the only thing that starts, watches or cancels one.
//!
//! **The division is T19's, one table across.** `mixengine_core::jobs` owns the row and the state
//! machine and has no loop, no clock and no task; what owns the timing, the [`CancellationToken`]
//! each job hangs off and the `Events` every move is announced on is the daemon — so it is here,
//! for the same reason the service runner is and not in `mixengine-supervisor`.
//!
//! **Nothing in this build produces a job yet**, and that is deliberate rather than an oversight.
//! The first producer is T21's download; T23's `runtime.install` is the first method to return one.
//! Building the registry now is what T19 did for services before anything could declare one — the
//! alternative is writing the loop twice, once inside the first producer and once properly
//! afterwards. What the tests use instead is [`Jobs::begin`] with work of their own, which is the
//! same shape a real producer will have.
//!
//! **A job's work is a task, so it does not survive this process**, which is the whole difference
//! between recovery here and the adoption in [`crate::services`]: there is nothing to adopt.
//! `mixengine_core::jobs::abandon` reconciles what a stopped daemon left behind, before the first
//! client is served.

use std::collections::BTreeMap;
use std::future::Future;
use std::sync::{Arc, Mutex};
use std::time::{Duration, SystemTime};

use mixengine_core::Store;
use mixengine_proto::{
    DaemonEvent, Error, JobFilter, JobId, JobKind, JobOutcome, JobSummary, Millis, Timestamp,
};
use tokio_util::sync::CancellationToken;

use crate::api::Events;
use crate::error::ToWire as _;

/// The longest `job.wait` will hold a connection, whatever a client asks for.
///
/// **A ceiling rather than a default** — [`JobWait::timeout`](mixengine_proto::JobWait) is what a
/// caller chooses, and this is what the daemon will grant. Without it one client could hold a
/// connection for a day against the rule in `.claude/architecture/daemon-and-ipc.md` that this
/// method is already the exception to. Two minutes is longer than any interactive wait and short
/// enough that a wedged client is noticed.
const LONGEST_WAIT: Duration = Duration::from_secs(120);

/// What a producer is given so it can report on itself.
///
/// Deliberately small: a job's work reports how far along it is and looks at whether it has been
/// cancelled, and everything else — the row, the event, the state machine — belongs to the registry.
/// A producer that could write its own ending would be a producer that could disagree with the row.
#[derive(Debug, Clone)]
pub(crate) struct JobHandle {
    /// Which job this is.
    id: JobId,

    /// Where the row lives.
    store: Store,

    /// How a move is announced.
    events: Events,

    /// Cancelled by `job.cancel`, and by the daemon's own shutdown.
    cancel: CancellationToken,
}

impl JobHandle {
    /// Which job this is, for a producer that wants to name it in a log line.
    // Unused outside tests until T21 writes the first producer. `cfg_attr` rather than a plain
    // `expect`, because these *are* used in this module's own tests and an expectation that holds
    // in one build and not the other is itself a warning.
    #[cfg_attr(
        not(test),
        expect(dead_code, reason = "the first producer arrives with T21")
    )]
    pub(crate) fn id(&self) -> JobId {
        self.id
    }

    /// Report how far along the work is.
    ///
    /// **Best-effort on purpose.** A write that fails — the row ended underneath it, the database is
    /// unreachable — is logged and swallowed rather than handed back, because a producer that had to
    /// handle a failed progress report would either ignore it (and this signature is the honest
    /// version of that) or abandon work that is going perfectly well over a status line. The one
    /// failure that matters is the ending, and that one the registry writes itself.
    ///
    /// It is a change that should be reported and not a heartbeat: this publishes to the same
    /// bounded stream every service transition uses, so a producer reporting every socket read would
    /// spend a client's whole allowance on a progress bar. See
    /// [`DaemonEvent::JobProgress`].
    // Unused outside tests until T21 writes the first producer. `cfg_attr` rather than a plain
    // `expect`, because these *are* used in this module's own tests and an expectation that holds
    // in one build and not the other is itself a warning.
    #[cfg_attr(
        not(test),
        expect(dead_code, reason = "the first producer arrives with T21")
    )]
    pub(crate) async fn progress(&self, percent: u8, message: impl Into<String>) {
        let at = Timestamp::from_system_time(SystemTime::now());

        match mixengine_core::jobs::progress(&self.store, self.id, percent, message.into(), at)
            .await
        {
            Ok(progress) => self.events.publish(DaemonEvent::JobProgress(progress)),
            Err(error) => {
                tracing::debug!(job = %self.id, %error, "a job's progress could not be recorded");
            }
        }
    }

    /// Whether somebody has asked this job to stop.
    ///
    /// **Cancellation is cooperative and this is the whole of the mechanism.** Nothing kills the
    /// work: a download that is half way through a file has a staging directory to remove and a
    /// partial file to delete, and a task dropped mid-`await` does the first of those and not the
    /// second. So the work is expected to look, and to return when it sees.
    // Unused outside tests until T21 writes the first producer. `cfg_attr` rather than a plain
    // `expect`, because these *are* used in this module's own tests and an expectation that holds
    // in one build and not the other is itself a warning.
    #[cfg_attr(
        not(test),
        expect(dead_code, reason = "the first producer arrives with T21")
    )]
    pub(crate) fn is_cancelled(&self) -> bool {
        self.cancel.is_cancelled()
    }

    /// The same question as something to wait on, for work that is already in a `select!`.
    // Unused outside tests until T21 writes the first producer. `cfg_attr` rather than a plain
    // `expect`, because these *are* used in this module's own tests and an expectation that holds
    // in one build and not the other is itself a warning.
    #[cfg_attr(
        not(test),
        expect(dead_code, reason = "the first producer arrives with T21")
    )]
    pub(crate) fn cancelled(&self) -> tokio_util::sync::WaitForCancellationFuture<'_> {
        self.cancel.cancelled()
    }
}

/// One job this daemon is running.
struct Entry {
    /// Cancelled by `job.cancel` and by the shutdown, and watched by the work.
    cancel: CancellationToken,

    /// Cancelled by the registry **after** the ending has been persisted, which is what makes
    /// [`Jobs::wait`] able to read the row it was waiting for rather than racing it.
    ///
    /// A token rather than a `watch` or a `Notify` because it carries nothing and cannot be missed:
    /// a waiter that arrives after the job has ended sees a token that is already cancelled, which
    /// is the same answer as having waited for it.
    finished: CancellationToken,
}

/// Every job this daemon is running, and the only door into one.
#[derive(Debug)]
pub(crate) struct Jobs {
    /// Where the rows live.
    store: Store,

    /// How a move is announced.
    ///
    /// Read only by [`Jobs::begin`] and [`Jobs::ended`], so it is unused outside tests until T21
    /// writes the first producer — see the note on those.
    #[cfg_attr(
        not(test),
        expect(dead_code, reason = "the first producer arrives with T21")
    )]
    events: Events,

    /// The daemon's root token. Every job's own token is a child of it, so a shutdown cancels every
    /// job without this having to hold a list for that purpose — and a job started after the
    /// shutdown began is cancelled before its work ever looks.
    root: CancellationToken,

    /// The jobs with a task behind them. A job is removed once its ending is written, so what is in
    /// here is exactly what `state = 'running'` means while this daemon is up.
    running: Mutex<BTreeMap<JobId, Entry>>,
}

impl std::fmt::Debug for Entry {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Entry")
            .field("cancelled", &self.cancel.is_cancelled())
            .field("finished", &self.finished.is_cancelled())
            .finish()
    }
}

impl Jobs {
    /// A registry with nothing running.
    pub(crate) fn new(store: &Store, events: Events, root: CancellationToken) -> Self {
        Self {
            store: store.clone(),
            events,
            root,
            running: Mutex::new(BTreeMap::new()),
        }
    }

    /// Start a job: write the row, spawn the work, and answer with what a client renders.
    ///
    /// The row is written **before** the task is spawned, so the id in the answer is an id
    /// `job.status` already knows about — a client that asks the instant it is told cannot be told
    /// `not_found` about work it was just handed.
    ///
    /// `work` is given a [`JobHandle`] and answers with what the job produced, or with the wire
    /// error a client should be shown. It is not given the registry: a producer that could write its
    /// own ending could write one the row disagrees with.
    ///
    /// # Errors
    ///
    /// The wire error of a row that could not be written. Nothing is spawned in that case.
    // Unused outside this module's tests until T21 writes the first producer — the deliberate
    // position described at the top of the file, and the same one `service.*` was in before T30.
    // `ended` is reachable only from here, so it carries the same note.
    #[cfg_attr(
        not(test),
        expect(dead_code, reason = "the first producer arrives with T21")
    )]
    pub(crate) async fn begin<F, W>(
        self: &Arc<Self>,
        kind: &JobKind,
        work: F,
    ) -> Result<JobSummary, Error>
    where
        F: FnOnce(JobHandle) -> W + Send + 'static,
        W: Future<Output = Result<serde_json::Value, Error>> + Send + 'static,
    {
        let at = Timestamp::from_system_time(SystemTime::now());
        let started = mixengine_core::jobs::create(&self.store, kind, at)
            .await
            .map_err(|error| error.to_wire())?;

        let entry = Entry {
            // A child of the root, so the shutdown reaches it without this registry keeping a second
            // list for that purpose.
            cancel: self.root.child_token(),
            finished: CancellationToken::new(),
        };
        let handle = JobHandle {
            id: started.id,
            store: self.store.clone(),
            events: self.events.clone(),
            cancel: entry.cancel.clone(),
        };
        let cancel = entry.cancel.clone();

        // Registered before the work is spawned, or a `job.cancel` arriving in the same millisecond
        // would find nothing to cancel and answer as though the job were already over.
        self.running
            .lock()
            .expect("the jobs registry lock is not held across an await")
            .insert(started.id, entry);

        let jobs = Arc::clone(self);
        let id = started.id;

        tokio::spawn(async move {
            let produced = work(handle).await;

            // **A job that finished while being cancelled has finished.** The same reading T15a's
            // stop command takes: an outcome is judged by what the work actually produced, and only
            // work that gave up is reported as cancelled. Otherwise a download that completed in the
            // instant somebody clicked cancel would be recorded as though it had not.
            let outcome = match produced {
                Ok(result) => JobOutcome::Succeeded { result },
                Err(_) if cancel.is_cancelled() => JobOutcome::Cancelled,
                Err(error) => JobOutcome::Failed { error },
            };

            jobs.ended(id, outcome).await;
        });

        Ok(started)
    }

    /// Persist an ending, announce it, and let go of the job — in that order.
    ///
    /// The order is what [`Jobs::wait`] rests on: the row is written first, so a waiter released by
    /// `finished` reads the ending rather than racing it, and the entry is removed last, so "is
    /// there an entry" is never true for a job whose ending is not yet readable.
    #[cfg_attr(
        not(test),
        expect(dead_code, reason = "reachable only from `begin` — see there")
    )]
    async fn ended(&self, id: JobId, outcome: JobOutcome) {
        let at = Timestamp::from_system_time(SystemTime::now());

        match mixengine_core::jobs::finish(&self.store, id, outcome, at).await {
            Ok(finish) => self.events.publish(DaemonEvent::JobFinished(finish)),

            // Reached when something else ended this job first — recovery at a boot that raced this
            // one, or a second producer. The row keeps the ending that got there first, which is
            // `core::jobs`' rule, and this is a line in the log rather than a panic.
            Err(error) => tracing::warn!(job = %id, %error, "a job's ending could not be recorded"),
        }

        let entry = self
            .running
            .lock()
            .expect("the jobs registry lock is not held across an await")
            .remove(&id);

        if let Some(entry) = entry {
            entry.finished.cancel();
        }
    }

    /// `job.list` — what this home has run, newest first.
    ///
    /// # Errors
    ///
    /// The wire error of a listing that could not be read.
    pub(crate) async fn list(&self, filter: &JobFilter) -> Result<Vec<JobSummary>, Error> {
        mixengine_core::jobs::records(&self.store, filter)
            .await
            .map_err(|error| error.to_wire())
    }

    /// `job.status` — one of them.
    ///
    /// # Errors
    ///
    /// `not_found` when there is no such job, or the wire error of a row that could not be read.
    pub(crate) async fn status(&self, id: JobId) -> Result<JobSummary, Error> {
        mixengine_core::jobs::record(&self.store, id)
            .await
            .map_err(|error| error.to_wire())
    }

    /// `job.cancel` — ask a running job to stop, and answer with the job as it stands.
    ///
    /// **Asking is all it does**, and the summary that comes back may still say `running`: the work
    /// ends when it next looks at its token, and [`DaemonEvent::JobFinished`] is what says it did.
    /// Answering anything else would be this daemon claiming an outcome it has not seen.
    ///
    /// Cancelling a job that has already ended is **not an error**. The caller wanted it stopped and
    /// it is stopped; reporting a failure would make a script that cancels on its way out fail for
    /// having been slightly late. A job that never existed still is one, because that is a caller
    /// asking about something else entirely.
    ///
    /// # Errors
    ///
    /// `not_found` when there is no such job.
    pub(crate) async fn cancel(&self, id: JobId) -> Result<JobSummary, Error> {
        // Read first: a job with no entry may be one that ended a moment ago or one that never
        // existed, and only the row can tell those apart.
        let summary = self.status(id).await?;

        let cancel = self
            .running
            .lock()
            .expect("the jobs registry lock is not held across an await")
            .get(&id)
            .map(|entry| entry.cancel.clone());

        if let Some(cancel) = cancel {
            tracing::info!(job = %id, "a client asked a job to stop");
            cancel.cancel();
        }

        Ok(summary)
    }

    /// `job.wait` — answer when the job ends, or when the caller's patience runs out.
    ///
    /// **A wait that runs out is an answer and not an error**, on `ServiceWalk::complete`'s
    /// precedent: what comes back is the job as it stands, and
    /// [`JobState::is_finished`](mixengine_proto::JobState::is_finished) is what a caller branches
    /// on. A script that wants to keep waiting calls again.
    ///
    /// The row is read **after** the wait rather than before, and both paths read it: a job that
    /// ended while the caller was being polled is a job that finished, and answering from a reading
    /// taken beforehand would report it as still running.
    ///
    /// # Errors
    ///
    /// `not_found` when there is no such job.
    pub(crate) async fn wait(&self, id: JobId, timeout: Millis) -> Result<JobSummary, Error> {
        // Read first so a job that does not exist is refused rather than waited for.
        let summary = self.status(id).await?;

        let finished = self
            .running
            .lock()
            .expect("the jobs registry lock is not held across an await")
            .get(&id)
            .map(|entry| entry.finished.clone());

        // No entry means the ending is already written — `ended` removes it last, on purpose.
        let Some(finished) = finished else {
            return Ok(summary);
        };

        let granted = Duration::from_millis(timeout.0).min(LONGEST_WAIT);

        tokio::select! {
            () = finished.cancelled() => {}
            () = tokio::time::sleep(granted) => {}
            // The daemon going is an answer too: what the row says then is whatever the job managed
            // to record on its way out, which is the honest one.
            () = self.root.cancelled() => {}
        }

        self.status(id).await
    }

    /// Wait for every running job to finish, on the daemon's way out.
    ///
    /// The root token has already cancelled each job's own, so this is where the process *waits* for
    /// the work to notice — the same shape `Registry::shut_down` has, and for the same reason: a
    /// task dropped mid-download leaves a staging directory behind, and the job's own cleanup is the
    /// thing that removes it.
    ///
    /// **Bounded by the caller**, which is the shutdown budget: a job that will not stop is left,
    /// and its row is reconciled by the next daemon's `abandon`. That is a worse outcome than
    /// waiting, and a better one than a daemon that cannot be stopped.
    pub(crate) async fn shut_down(&self, within: Duration) {
        let deadline = tokio::time::Instant::now() + within;

        loop {
            let waiting: Vec<CancellationToken> = self
                .running
                .lock()
                .expect("the jobs registry lock is not held across an await")
                .values()
                .map(|entry| entry.finished.clone())
                .collect();

            let Some(next) = waiting.into_iter().next() else {
                return;
            };

            if tokio::time::timeout_at(deadline, next.cancelled())
                .await
                .is_err()
            {
                tracing::warn!(
                    "jobs were still running when the shutdown budget ran out; \
                     the next daemon will mark them failed"
                );
                return;
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use std::time::Duration;

    use mixengine_proto::{ErrorCode, JobState};

    use super::*;

    /// Long enough that a machine under load does not fail a test, short enough that a genuinely
    /// stuck wait is noticed. The same value the services fixture uses, for the same reason.
    const EVENTUALLY: Duration = Duration::from_secs(10);

    /// The same patience, as the wire type `job.wait` takes.
    const EVENTUALLY_MILLIS: Millis = Millis(10_000);

    async fn store() -> (tempfile::TempDir, Store) {
        let home = tempfile::tempdir().expect("a temporary directory");
        let store = Store::open(&home.path().join("mixengine.db"))
            .await
            .expect("a database");
        (home, store)
    }

    fn kind() -> JobKind {
        JobKind::parse("runtime.install").expect("a valid kind")
    }

    /// A registry, the events it publishes on, and the token a shutdown would cancel.
    async fn jobs() -> (tempfile::TempDir, Arc<Jobs>, Events, CancellationToken) {
        let (home, store) = store().await;
        let events = Events::new();
        let root = CancellationToken::new();
        let jobs = Arc::new(Jobs::new(&store, events.clone(), root.clone()));

        (home, jobs, events, root)
    }

    #[tokio::test]
    async fn a_job_that_succeeds_is_answered_running_and_ends_with_what_it_produced() {
        let (_home, jobs, _events, _root) = jobs().await;

        let started = jobs
            .begin(&kind(), |_handle| async {
                Ok(serde_json::json!({"version": "8.3.12"}))
            })
            .await
            .expect("the row is written before anything is spawned");

        assert_eq!(
            started.state,
            JobState::Running,
            "a job is answered as accepted, not as finished"
        );

        let finished = jobs
            .wait(started.id, Millis::from_secs(10))
            .await
            .expect("the job exists");

        assert_eq!(finished.state, JobState::Succeeded);
        assert_eq!(
            finished.outcome,
            Some(JobOutcome::Succeeded {
                result: serde_json::json!({"version": "8.3.12"})
            })
        );
    }

    #[tokio::test]
    async fn a_job_that_fails_carries_the_error_a_client_renders() {
        let (_home, jobs, _events, _root) = jobs().await;

        let started = jobs
            .begin(&kind(), |_handle| async {
                Err(Error::new(ErrorCode::Io, "the checksum did not match"))
            })
            .await
            .expect("a job");

        let finished = jobs
            .wait(started.id, Millis::from_secs(10))
            .await
            .expect("the job exists");

        assert_eq!(finished.state, JobState::Failed);
        assert!(
            matches!(&finished.outcome, Some(JobOutcome::Failed { error }) if error.code == ErrorCode::Io),
            "{finished:?}"
        );
    }

    /// The whole of the cancellation mechanism: the work looks, and returns when it sees.
    #[tokio::test]
    async fn cancelling_a_job_reaches_the_work_and_is_recorded_as_a_cancellation() {
        let (_home, jobs, _events, _root) = jobs().await;

        let (seen, told) = (Arc::new(Mutex::new(None)), Arc::new(Mutex::new(false)));
        let (reports, asked_it) = (Arc::clone(&seen), Arc::clone(&told));

        let started = jobs
            .begin(&kind(), |handle| async move {
                // The handle knows which job it belongs to, which is what a producer's own log
                // lines are keyed on.
                *reports.lock().expect("a test lock") = Some(handle.id());

                handle.cancelled().await;
                *asked_it.lock().expect("a test lock") = handle.is_cancelled();

                Err(Error::new(ErrorCode::Internal, "gave up when asked"))
            })
            .await
            .expect("a job");

        let asked = jobs.cancel(started.id).await.expect("the job exists");
        assert_eq!(
            asked.state,
            JobState::Running,
            "asking is all a cancel does — the work ends when it next looks"
        );

        let finished = jobs
            .wait(started.id, EVENTUALLY_MILLIS)
            .await
            .expect("the job exists");

        assert_eq!(
            finished.state,
            JobState::Cancelled,
            "not `failed`: nothing went wrong, somebody asked"
        );
        assert_eq!(finished.outcome, Some(JobOutcome::Cancelled));
        assert_eq!(
            *seen.lock().expect("a test lock"),
            Some(started.id),
            "the work knows which job it is"
        );
        assert!(
            *told.lock().expect("a test lock"),
            "and it saw the asking, rather than being killed"
        );
    }

    /// The same reading T15a's stop command takes: work that finished while being asked to stop has
    /// finished, and recording it as cancelled would erase a download that completed.
    #[tokio::test]
    async fn work_that_finished_while_being_cancelled_is_reported_as_having_finished() {
        let (_home, jobs, _events, _root) = jobs().await;

        let started = jobs
            .begin(&kind(), |handle| async move {
                handle.cancelled().await;

                Ok(serde_json::json!("it was already done"))
            })
            .await
            .expect("a job");

        jobs.cancel(started.id).await.expect("the job exists");

        let finished = jobs
            .wait(started.id, EVENTUALLY_MILLIS)
            .await
            .expect("the job exists");

        assert_eq!(finished.state, JobState::Succeeded);
    }

    /// Cancelling something that has already ended is what a script on its way out does, and it is
    /// not a failure: the caller wanted it stopped and it is.
    #[tokio::test]
    async fn cancelling_a_job_that_has_already_ended_changes_nothing_and_is_not_an_error() {
        let (_home, jobs, _events, _root) = jobs().await;

        let started = jobs
            .begin(&kind(), |_handle| async { Ok(serde_json::Value::Null) })
            .await
            .expect("a job");
        jobs.wait(started.id, EVENTUALLY_MILLIS)
            .await
            .expect("the job exists");

        let after = jobs.cancel(started.id).await.expect("not an error");

        assert_eq!(after.state, JobState::Succeeded, "exactly as it ended");
    }

    /// A caller asking about something else entirely still gets told.
    #[tokio::test]
    async fn cancelling_a_job_that_never_existed_is_not_found() {
        let (_home, jobs, _events, _root) = jobs().await;

        let error = jobs.cancel(JobId(404)).await.expect_err("no such job");

        assert_eq!(error.code, ErrorCode::NotFound);
    }

    /// The timeout is an answer, not an error — and the job carries on behind it.
    #[tokio::test]
    async fn a_wait_that_runs_out_answers_with_the_job_as_it_stands() {
        let (_home, jobs, _events, _root) = jobs().await;

        let started = jobs
            .begin(&kind(), |handle| async move {
                handle.cancelled().await;

                Ok(serde_json::Value::Null)
            })
            .await
            .expect("a job");

        let waited = jobs
            .wait(started.id, Millis(50))
            .await
            .expect("a wait that runs out is an answer");

        assert_eq!(waited.state, JobState::Running);
        assert!(!waited.state.is_finished(), "and it says so");

        // Still going, which is what "the wait ran out" means.
        jobs.cancel(started.id).await.expect("the job exists");
        jobs.wait(started.id, EVENTUALLY_MILLIS)
            .await
            .expect("the job exists");
    }

    /// Progress reaches the row and the stream as one value, which is the rule the whole design
    /// rests on.
    #[tokio::test]
    async fn progress_reaches_both_the_row_and_the_stream() {
        let (_home, jobs, events, _root) = jobs().await;
        let mut watching = events.subscribe();

        let started = jobs
            .begin(&kind(), |handle| async move {
                handle.progress(40, "verifying the download").await;
                handle.cancelled().await;

                Ok(serde_json::Value::Null)
            })
            .await
            .expect("a job");

        let mut reported = None;
        while let Some(frame) = watching.next_or_heartbeat().await {
            if let crate::api::events::Frame::Event(DaemonEvent::JobProgress(progress)) = frame {
                reported = Some(progress);
                break;
            }
        }

        let reported = reported.expect("the progress was announced");
        assert_eq!(reported.job, started.id);
        assert_eq!(reported.percent, 40);
        assert_eq!(reported.message, "verifying the download");

        let row = jobs.status(started.id).await.expect("the job exists");
        assert_eq!(row.percent, 40, "the same value the event carried");
        assert_eq!(row.message, "verifying the download");

        jobs.cancel(started.id).await.expect("the job exists");
    }

    /// The daemon's own token reaches every job, without the registry keeping a second list for it.
    #[tokio::test]
    async fn a_shutdown_cancels_every_running_job() {
        let (_home, jobs, _events, root) = jobs().await;

        let started = jobs
            .begin(&kind(), |handle| async move {
                handle.cancelled().await;

                Err(Error::new(ErrorCode::Internal, "the daemon is going"))
            })
            .await
            .expect("a job");

        root.cancel();
        jobs.shut_down(EVENTUALLY).await;

        let finished = jobs.status(started.id).await.expect("the job exists");
        assert_eq!(finished.state, JobState::Cancelled);
    }

    /// A shutdown with nothing running returns at once rather than spending its budget.
    #[tokio::test]
    async fn a_shutdown_with_no_jobs_waits_for_nothing() {
        let (_home, jobs, _events, root) = jobs().await;

        root.cancel();
        jobs.shut_down(EVENTUALLY).await;
    }

    /// A job that will not stop is left, and the row is the next daemon's to reconcile — which is a
    /// worse outcome than waiting and a better one than a daemon that cannot be stopped.
    #[tokio::test]
    async fn a_job_that_will_not_stop_does_not_hold_the_shutdown_open() {
        let (_home, jobs, _events, root) = jobs().await;

        let started = jobs
            .begin(&kind(), |_handle| async {
                std::future::pending::<()>().await;

                Ok(serde_json::Value::Null)
            })
            .await
            .expect("a job");

        root.cancel();
        jobs.shut_down(Duration::from_millis(100)).await;

        assert_eq!(
            jobs.status(started.id).await.expect("the job exists").state,
            JobState::Running,
            "still going, and the row still says so — `abandon` is what closes it"
        );
    }
}
