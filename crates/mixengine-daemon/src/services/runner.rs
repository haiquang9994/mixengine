//! One task per service: spawn it, wait for it to be ready, watch it, restart it, stop it.
//!
//! **This is the loop `mixengine-supervisor` deliberately does not contain.** That crate delivers
//! the mechanisms — capture, ready, health, restart — as pieces with no loop, no clock and no state
//! row, because the thing that owns the timing is also the thing that owns the registry of running
//! services, the [`CancellationToken`] they hang off and the
//! [`transition`](mixengine_core::services::transition) that persists each move. That is the daemon,
//! and this module is where the four are tied together.
//!
//! Every state change goes through `mixengine_core::services::transition` and is published from the
//! value it hands back, so the row and the event cannot describe different events — the registry
//! never writes `services.state` behind `core`'s back.

use std::collections::BTreeMap;
use std::ffi::OsString;
use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;

use mixengine_core::{Store, services};
use mixengine_platform::Host;
use mixengine_platform::process::{self, Adopted, CAN_ASK_TO_STOP, Exit, Supervised};
use mixengine_proto::{
    EnvValue, RestartPolicy, ServiceId, ServiceSpec, ServiceState, StateReason, StopBehaviour,
};
use mixengine_supervisor::logs::Capture;
use mixengine_supervisor::{Decision, Health, Restarts, Surroundings, Verdict, ready};
use tokio::sync::{Notify, watch};
use tokio::time::Instant;
use tokio_util::sync::CancellationToken;

use super::now;
use crate::api::Events;

/// How often a running service is asked whether it is still there.
///
/// Coarser than the 50 ms `ready::wait` polls at, and for a different question: that one is racing a
/// probe on a service somebody is waiting for, this one runs for days on a service nobody is
/// watching. A quarter of a second is well inside what a person perceives as immediate and costs a
/// handful of syscalls a second across every service on the machine.
const WATCH: Duration = Duration::from_millis(250);

/// How often a stopping service is asked whether it has gone yet.
///
/// Paid only during a stop, and every one of these is latency a user is sitting through, so it is
/// the fine-grained one.
const POLL: Duration = Duration::from_millis(50);

/// How long a killed survivor is waited for before the daemon gives up on watching it go.
///
/// Only ever reached by an **adopted** service (roadmap task T18), because that is the one this
/// process cannot wait on: it is not the survivor's parent, so there is no status to reap and the
/// only question available is "is it still there", asked at [`POLL`]. A `Supervised` child is waited
/// for by the kernel and needs no ceiling at all.
///
/// Generous, because what it is measuring is a `SIGKILL` being delivered and the process leaving the
/// table — milliseconds unless the machine is in trouble. What happens when it runs out is in
/// [`Runner::stop_adopted`], and it is deliberately not "record it as stopped anyway".
const GONE: Duration = Duration::from_secs(5);

/// How long an **adopted** service's environment is waited for before its stop goes on without it.
///
/// Paid on one path only — see [`Runner::where_commands_run`] — and it is the shutdown path, which
/// is what makes it a deadline rather than a patience. A `Keyring` value is read through the OS
/// credential store, which on Linux is a D-Bus round trip to a daemon that may be *prompting the
/// user*: a locked keyring answers when somebody types a password, or never. Without a ceiling here
/// a `mix stop` of an adopted MariaDB, or a whole daemon shutting down, waits for that forever.
///
/// Generous against an unlocked store, which answers in milliseconds, and short against a person who
/// is not at the machine.
const ENVIRONMENT: Duration = Duration::from_secs(3);

/// How long the last lines of a stopped service are waited for.
///
/// Bounded because end of file is not the process exiting but the *last holder of the pipe*
/// exiting — see [`Capture::finish`], which explains why an unbounded wait here would hang the
/// supervisor at the one moment it has something to report.
const FLUSH: Duration = Duration::from_secs(2);

/// What a walk of the spec's environment came back with: every entry that resolved, and the error of
/// each that did not.
///
/// Both halves, because the two callers of [`Runner::walk_environment`] disagree about what a
/// failure means — a start refuses anything less than all of it, a stop command runs with what there
/// is.
type Resolved = (BTreeMap<String, String>, Vec<(String, anyhow::Error)>);

/// What a walk of the spec's environment does with an entry that will not resolve.
///
/// The difference is not bookkeeping: resolving a `Keyring` entry can put a prompt on the user's
/// screen, so how far the walk goes decides how many of them a single start or stop puts there.
#[derive(Clone, Copy, Debug)]
enum OnFailure {
    /// Stop there, with that entry's error.
    ///
    /// What a start uses. It refuses anything less than the whole environment, so every entry after
    /// the first failure is one whose value will not be used — and on a locked keyring, walking on
    /// asks the user to unlock it once per credential for a start that has already failed.
    Stop,

    /// Write it down and keep walking.
    ///
    /// What a stop command uses: it runs with whatever entries there are, so the ones after a
    /// failure are still worth having.
    Record,
}

/// Whether the service this runner supervises is somewhere traffic can go.
///
/// **The registry reads this and never the task's liveness**, which is the distinction that makes a
/// tiered walk mean anything: a runner is alive through a restart backoff, through a stop and
/// through a start that has not finished, as well as through a healthy hour. "Something is
/// supervising it" is not "it is up", and a walk that took the one for the other would start a site
/// against a database that is in its fourth crash.
///
/// Derived from the transition [`Runner::move_to`] has just persisted rather than described a second
/// time beside it, on the same reasoning as the event: two descriptions of one move drift.
#[derive(Debug, Clone)]
pub(super) enum Readiness {
    /// A start is in flight and has not been decided. The value a runner begins with, and the one
    /// it returns to whenever a backoff releases it.
    Deciding,

    /// The ready check passed and the process is there.
    Up,

    /// It is not usable, and this is what was persisted about why.
    ///
    /// [`None`] is never produced here: it is what the registry reports for a runner that ended
    /// without deciding at all — see [`super::settled`].
    Down(Option<StateReason>),
}

impl Readiness {
    /// What a service that has just reached `state` under `restart` is, to a walk that wants to know
    /// whether it may go on.
    ///
    /// Exhaustive over [`ServiceState`], which is closed for readers exactly like this one: a state
    /// added without a decision here would be a service the walk silently misjudges.
    fn of(state: ServiceState, reason: StateReason, restart: RestartPolicy) -> Self {
        match state {
            // `Degraded` is up on purpose: a service answering badly is the amber case the GUI
            // shows and `mix doctor` explains, not an absent one, and a dependent that refused to
            // start against it would turn one slow database into a machine with nothing running.
            ServiceState::Running | ServiceState::Degraded => Self::Up,

            // A start in flight, whether it is the first or the one a backoff has just released. A
            // second walk that arrives here waits for the same answer rather than inventing one.
            ServiceState::Starting => Self::Deciding,

            // **`Restarting` is the one the policy decides**, and it is what keeps a walk finite.
            //
            // A bounded policy arrives somewhere by itself: `Running` when an attempt takes,
            // `Failed` when the ceiling is reached. A walk under one waits the backoff out and is
            // answered by whichever the policy reached — which is what the default `OnFailure` needs,
            // because its first crash is a transient the runner recovers from half a second later,
            // and a walk that read that one crash as `Down` would leave the tier below `Failed` and
            // unsupervised beside a service that came up fine.
            //
            // `RestartPolicy::Always` is the one with no ceiling. Nothing under it ever reaches
            // `Failed`, so a walk that waited for it to *give up* would wait for ever, and the first
            // attempt is the only answer there is.
            ServiceState::Restarting => match restart {
                RestartPolicy::Always { .. } => Self::Down(Some(reason)),
                _ => Self::Deciding,
            },

            ServiceState::Stopping | ServiceState::Stopped | ServiceState::Failed => {
                Self::Down(Some(reason))
            }
        }
    }
}

/// Everything one supervised service needs, and nothing about any other.
#[derive(Debug)]
pub(super) struct Runner {
    /// What to run and how to judge it.
    pub(super) spec: ServiceSpec,

    /// Where the state row lives. Every move is written here before it is published.
    pub(super) store: Store,

    /// `logs/services/<service-id>/`, which `Capture` writes `current.log` into.
    pub(super) directory: PathBuf,

    /// The OS, for the one thing a spawn needs from it that the spec cannot carry: a credential.
    pub(super) host: Arc<dyn Host>,

    /// Where a persisted transition is announced.
    pub(super) events: Events,

    /// Cancelled to stop this service. A child of the daemon's root token, so a daemon on its way
    /// out stops its services rather than dropping them.
    pub(super) cancel: CancellationToken,

    /// Notified when somebody asks for this service to start *now*, rather than when its restart
    /// policy next comes round. Roadmap task **T19c**, and the only edge that runs from the registry
    /// into a runner: everything else it does to a live runner is a read.
    ///
    /// A [`Notify`] because the question carries nothing but the asking, and because a request that
    /// arrives while nothing is waiting is kept as one permit — so an explicit start is honoured by
    /// the backoff this runner is in, or by the next one it enters, and two of them arriving together
    /// are one restart rather than two.
    pub(super) asked_to_start: Arc<Notify>,

    /// Where this service's own commands are run — a health probe, a shutdown command.
    ///
    /// Resolved once, at the spawn that begins each life of the process, and kept: the environment
    /// it holds is the one the *service* was given, credentials and all, and re-deriving it would
    /// mean an OS keyring read on every health probe of every service for as long as the machine is
    /// up. See [`Surroundings`] for why a probe run anywhere else would be asking about a different
    /// server.
    ///
    /// [`None`] until this runner has spawned something, which is the adopted case (roadmap task
    /// T18): nothing in *this* process ever built that environment, so a stop command there resolves
    /// one when it is needed — see [`Runner::where_commands_run`].
    pub(super) surroundings: Option<Surroundings>,

    /// Where this runner says whether its service is usable.
    ///
    /// A [`watch`] rather than a one-shot, because the question is asked more than once and by more
    /// than the walk that started the service: the answer has to be *current* for whoever asks next,
    /// where a one-shot would leave the walk after it reading the outcome of a start that ended an
    /// hour ago. Dropped with this runner, which is how the registry learns that a task ended
    /// without deciding.
    pub(super) readiness: watch::Sender<Readiness>,
}

/// A process the runner can ask to stop and then watch for.
///
/// **Two implementations, and the trait exists to keep one `StopBehaviour` reading rather than two.**
/// A `Supervised` child is this daemon's own and a survivor it adopted (roadmap task T18) is not,
/// but the grace period a spec asks for is about the *service* — the seconds MariaDB needs to flush
/// — and is the same either way. What differs is only what a request travels on and where the answer
/// comes from: a status the kernel keeps for a child of ours, and the process's identity for one
/// that was somebody else's.
///
/// Deliberately the two calls [`Runner::ask_to_stop`] makes and no more. The kill afterwards is not
/// here because the two are genuinely different — a group whose ownership this process holds, and a
/// pid it can only signal — and folding them together would hide the one place adoption is weaker.
trait Stoppable: Send {
    /// Ask it to stop, the way this system asks. See [`process::CAN_ASK_TO_STOP`].
    fn ask_to_stop(&self) -> mixengine_platform::Result<()>;

    /// Whether it has ended, without waiting for it either way.
    fn exited(&mut self) -> mixengine_platform::Result<Option<Exit>>;
}

impl Stoppable for Supervised {
    fn ask_to_stop(&self) -> mixengine_platform::Result<()> {
        Self::ask_to_stop(self)
    }

    fn exited(&mut self) -> mixengine_platform::Result<Option<Exit>> {
        Self::exited(self)
    }
}

impl Stoppable for Adopted {
    fn ask_to_stop(&self) -> mixengine_platform::Result<()> {
        Self::ask_to_stop(self)
    }

    /// Takes `&mut self` although the underlying question does not, because the other implementation
    /// needs it: `Supervised::exited` reaps a child, which is a mutation of the handle.
    fn exited(&mut self) -> mixengine_platform::Result<Option<Exit>> {
        Self::exited(self)
    }
}

/// What the runner does once a life of the process is over.
#[derive(Debug)]
enum After {
    /// Nothing more. The service is `Stopped` or `Failed` and this task ends.
    Done,

    /// Start it again, once this has elapsed.
    Again {
        /// The backoff the restart policy chose.
        after: Duration,
        /// Which attempt the next start is, counted from 1 since the service was last healthy.
        attempt: u32,
    },
}

/// What let a runner out of a backoff.
#[derive(Debug)]
enum Released {
    /// The wait the restart policy asked for is over.
    Elapsed,

    /// Somebody asked for this service to start now — see [`Runner::asked_to_start`].
    Asked,

    /// The service was asked to stop while it was waiting. It is `Stopped` and the task is over.
    Stopped,
}

impl Runner {
    /// Supervise this service until it is stopped, gives up, or the daemon does.
    ///
    /// **What a walk waits on is [`Runner::readiness`] and not this task**, which is what makes a
    /// tiered walk both possible and finite: the tier below may begin the moment this service is
    /// ready, a service that does not come up stops the walk rather than leaving it waiting for a
    /// process that is not coming, and a service being put back by a policy with no ceiling is
    /// answered after the first attempt rather than after a `Failed` that is never coming either.
    pub(super) async fn run(mut self) {
        let mut restarts = Restarts::under(self.spec.restart());

        self.live(&mut restarts, StateReason::Requested).await;
    }

    /// Supervise a process that was **already running** when this daemon started — roadmap task
    /// **T18**.
    ///
    /// The other way into this runner, and the difference is only how the first life of the process
    /// began: a daemon that was killed left this one behind, and its row, its pid and the moment it
    /// began are what the registry identified it by. From the moment that process ends, everything
    /// is ordinary again — the restart policy decides, and a service it puts back is spawned by
    /// [`Runner::attempt`] as a child of this daemon, with its pipes, its group and its log capture
    /// restored.
    ///
    /// **No transition is written on the way in**, and that is the point of adopting rather than
    /// restarting: nothing happened to the service. Its row said `running` before this daemon
    /// existed and says `running` still, so a state change here would be an event announcing that a
    /// service somebody has been using all along has just started. What *is* published is the
    /// readiness, because that lives in this process and this process has only just learned it.
    ///
    /// What the adopted life is missing is stated where a user pays for it: its output is not
    /// captured — the pipes belong to a daemon that is gone — so `current.log` has a hole in it from
    /// the moment that daemon died until the service is next started properly, and a crash loop
    /// decided during this life carries no tail to explain itself with. `mix doctor` owes the
    /// sentence (T47).
    pub(super) async fn adopt(mut self, adopted: Adopted) {
        tracing::info!(
            service = self.spec.id().as_str(),
            pid = adopted.pid(),
            "adopted a process that outlived the daemon supervising it"
        );

        // The row already says where this service is; what nothing in *this* process knows yet is
        // that it is usable, which is what a walk waiting on it is waiting for.
        self.readiness.send_replace(Readiness::Up);

        let mut restarts = Restarts::under(self.spec.restart());

        if let After::Again { after, attempt } = self.watch_adopted(adopted, &mut restarts).await
            && let Some(reason) = self
                .wait_before_starting_again(after, attempt, &mut restarts)
                .await
        {
            self.live(&mut restarts, reason).await;
        }
    }

    /// One life of the process after another, for as long as the policy keeps putting it back.
    ///
    /// `reason` is what the *first* start of this loop is for: somebody asking, a backoff that
    /// elapsed, or — for a service this runner adopted — the crash of the process it took over.
    async fn live(&mut self, restarts: &mut Restarts, mut reason: StateReason) {
        loop {
            // A `Starting` that will not persist ends this task with the readiness still undecided,
            // deliberately: the failure is the daemon's own, it is in `daemon.log`, and it is not a
            // state a client could render — which is what the registry reports for it.
            if !self.move_to(ServiceState::Starting, reason).await {
                return;
            }

            match self.attempt(restarts).await {
                After::Done => return,

                After::Again { after, attempt } => {
                    match self
                        .wait_before_starting_again(after, attempt, restarts)
                        .await
                    {
                        None => return,
                        Some(next) => reason = next,
                    }
                }
            }
        }
    }

    /// Wait out a backoff and say what the start after it is for. [`None`] if there is not going to
    /// be one.
    async fn wait_before_starting_again(
        &self,
        after: Duration,
        attempt: u32,
        restarts: &mut Restarts,
    ) -> Option<StateReason> {
        match self.wait_out(after).await {
            Released::Stopped => None,

            Released::Elapsed => Some(StateReason::BackoffElapsed { attempt }),

            // **A person asking is not the policy coming round again**, and the difference is what
            // `Restarts::recovered` records: the wait goes back to the shortest the policy allows,
            // while the failure history stays, because a service somebody has restarted four times
            // is still a service that has crashed four times.
            Released::Asked => {
                restarts.recovered();

                Some(StateReason::Requested)
            }
        }
    }

    /// One life of the process: spawn it, wait for readiness, then watch it until it ends.
    async fn attempt(&mut self, restarts: &mut Restarts) -> After {
        let env = match self.environment().await {
            Ok(env) => env,

            // A credential the spec names and the keyring does not hold. The process was never
            // started, which is exactly what `SpawnFailed` says — and the entry is named in
            // `daemon.log` and never in the event, because the event is rendered in a GUI.
            Err(error) => {
                tracing::error!(
                    service = self.spec.id().as_str(),
                    error = %error,
                    "cannot resolve the environment this service is to be started with"
                );

                return self.give_up(StateReason::SpawnFailed).await;
            }
        };

        let args: Vec<OsString> = self.spec.args().iter().map(OsString::from).collect();

        let mut supervised =
            match process::spawn_supervised(self.spec.program(), &args, self.spec.cwd(), &env) {
                Ok(supervised) => supervised,

                Err(error) => {
                    tracing::error!(
                        service = self.spec.id().as_str(),
                        program = %self.spec.program().display(),
                        error = %error,
                        "cannot start this service"
                    );

                    return self.give_up(StateReason::SpawnFailed).await;
                }
            };

        // Kept for the life of this process rather than rebuilt: what a health probe and a shutdown
        // command need is the environment the service is *running* with, and resolving it again
        // would be an OS keyring read every ten seconds for as long as the service is up.
        self.surroundings = Some(Surroundings::new(self.spec.cwd(), env));

        // Before anything waits on the process: a pipe nobody drains stops the service writing to
        // it, and a ready check that matches a log pattern has nothing to match against until this
        // exists.
        let capture = Capture::start(
            &mut supervised,
            self.spec.id(),
            self.spec.logs(),
            Some(&self.directory),
        );

        // The three columns nothing wrote before T19, and the pair T18 adopts on. The start time is
        // read while the handle is still held, which is what makes it this child's: an unreaped
        // child keeps its pid reserved on Unix, and on Windows this process holds a handle to it, so
        // the number cannot have been given away in between. A reading that fails leaves the column
        // null rather than guessing — the process is supervised either way, and what a null costs is
        // only that a daemon restart will not adopt it.
        let started_at = match supervised.started_at() {
            Ok(started_at) => started_at,

            Err(error) => {
                tracing::warn!(
                    service = self.spec.id().as_str(),
                    error = %error,
                    "cannot read when this service's process began; a daemon restart will not adopt \
                     it"
                );

                None
            }
        };

        if let Err(error) = services::started(
            &self.store,
            self.spec.id(),
            supervised.pid(),
            started_at.map(process::StartTime::stored),
            now(),
        )
        .await
        {
            tracing::error!(
                service = self.spec.id().as_str(),
                error = %error,
                "cannot record the process this service is running as; it is supervised but a \
                 daemon restart will not adopt it"
            );
        }

        // Raced against the stop for the same reason the watch loop is, and it matters more here:
        // a ready check is allowed tens of seconds, and a daemon that only looked at its token
        // after `ready::wait` returned would sit through all of them on a service it is shutting
        // down. The future is dropped rather than resumed, so nothing here needs to be cancel safe.
        let outcome = tokio::select! {
            biased;

            () = self.cancel.cancelled() => None,

            outcome = ready::wait(self.spec.ready(), &mut supervised, &capture) => Some(outcome),
        };

        // Asked to stop before it was ever ready. Through the same stop the spec asks for: the
        // process is up, `Starting` reaches `Stopping`, and a service that is mid-start is exactly
        // the one whose data directory should not be left to a destructor that kills.
        let Some(outcome) = outcome else {
            return self.stop(supervised, capture).await;
        };

        match outcome {
            Ok(ready::Ready::Ready) => {
                if !self
                    .move_to(ServiceState::Running, StateReason::Ready)
                    .await
                {
                    self.kill(supervised, capture).await;
                    self.record_exit(None).await;

                    return After::Done;
                }

                self.answered_by_this_start();

                self.supervise(supervised, capture, restarts).await
            }

            // The most common way a service fails to start, and the reason `ready::wait` races the
            // exit rather than polling the probe: this is the same path a crash an hour from now
            // takes, and it is the restart policy that decides between them.
            Ok(ready::Ready::Exited(exit)) => {
                let capture = self.kill(supervised, capture).await;
                let decision = restarts.ended(&exit, std::time::Instant::now(), &capture);

                self.after_exit(decision, exit.code()).await
            }

            // Running, and never going to be usable. Killed rather than left: the next attempt would
            // collide with the port and the data directory this one is holding.
            Ok(ready::Ready::TimedOut) => {
                let after = self.spec.ready().timeout();
                self.kill(supervised, capture).await;
                self.record_exit(None).await;

                self.give_up(StateReason::ReadyTimeout { after }).await
            }

            // A spec this build or this machine cannot check. Not a timeout, and reported as what it
            // is — see `StateReason::Uncheckable`.
            Err(error) => {
                let reason = uncheckable(&error);
                tracing::error!(
                    service = self.spec.id().as_str(),
                    error = %error,
                    "this service cannot be checked for readiness here"
                );

                self.kill(supervised, capture).await;
                self.record_exit(None).await;

                self.give_up(reason).await
            }
        }
    }

    /// Watch a running service: for the daemon asking it to stop, for it ending, for it going sick.
    ///
    /// One timer rather than two, and it is the cheaper one that sets the pace: the health check's
    /// interval is measured in seconds and the liveness poll in milliseconds, so the loop wakes on
    /// whichever is next and asks only the question that is due.
    async fn supervise(
        &mut self,
        mut supervised: Supervised,
        capture: Capture,
        restarts: &mut Restarts,
    ) -> After {
        let mut health = self.spec.health().map(Health::watching);
        let mut due = health
            .as_ref()
            .map(|health| Instant::now() + health.interval());

        // Whether the current run of unmakeable probes has already been reported. See the `Err` arm
        // below: a transient fault is worth one line, not one line every interval.
        let mut complained = false;

        // Taken once, before the loop: the borrow checker's reason is that the loop reaches `&mut
        // self`, and the better one is that a probe every ten seconds must not be a keyring read
        // every ten seconds. Only `HealthProbe::Command` reads it.
        let place = self.where_commands_run().await;

        loop {
            let watch = Instant::now() + WATCH;
            let wake = due.map_or(watch, |due| due.min(watch));

            tokio::select! {
                // Biased towards the stop: a service the daemon has been asked to shut down should
                // not spend one more health interval being probed.
                biased;

                () = self.cancel.cancelled() => {
                    return self.stop(supervised, capture).await;
                }

                () = tokio::time::sleep_until(wake) => {}
            }

            match supervised.exited() {
                Ok(Some(exit)) => {
                    // Said before the kill rather than after, and this is the only place a readiness
                    // is published without a row behind it. Between a process ending and
                    // `after_exit` persisting what that meant lie a drain bounded by `FLUSH` and a
                    // write, and a walk arriving inside that window would otherwise read the `Up`
                    // this service stopped being — and start the tier below against a database that
                    // has gone. `Deciding` and not `Down` because what happens next is the restart
                    // policy's to say: whoever is waiting waits a moment longer for the answer
                    // rather than being handed a failure that a restart is about to contradict.
                    self.readiness.send_replace(Readiness::Deciding);

                    let capture = self.kill(supervised, capture).await;
                    let decision = restarts.ended(&exit, std::time::Instant::now(), &capture);

                    return self.after_exit(decision, exit.code()).await;
                }

                Ok(None) => {}

                // The OS will not say. Nothing to decide on, and the next tick asks again.
                Err(error) => tracing::warn!(
                    service = self.spec.id().as_str(),
                    error = %error,
                    "cannot tell whether this service is still running"
                ),
            }

            let Some(watching) = health.as_mut() else {
                continue;
            };

            if due.is_some_and(|due| Instant::now() < due) {
                continue;
            }

            // Raced against the stop for the same reason the sleep above is: the probe is a program
            // the spec named or an HTTP request, and both are bounded by `HealthCheck::timeout`
            // rather than by anything short. A `mariadb-admin ping` against a database that has
            // stopped answering hangs for the whole of it, and awaiting that outright would make a
            // `mix stop` arriving mid-probe wait out a deadline set for judging health, not for
            // shutting down. Dropping the future is the cancellation: `run_once` kills the child it
            // spawned, and the fold this verdict would have gone into belongs to a loop that is
            // ending anyway.
            let examined = tokio::select! {
                biased;

                () = self.cancel.cancelled() => {
                    return self.stop(supervised, capture).await;
                }

                examined = watching.examine(&place) => examined,
            };

            match examined {
                Ok(Some(Verdict::Degraded)) => {
                    self.move_to(ServiceState::Degraded, StateReason::Unhealthy)
                        .await;
                }

                Ok(Some(Verdict::Recovered)) => {
                    // The backoff, not the failure history: a service that recovers between crashes
                    // is still crashing, and `Restarts` is what remembers that.
                    restarts.recovered();
                    self.move_to(ServiceState::Running, StateReason::Healthy)
                        .await;
                }

                Ok(None) => {}

                // A probe that could not be made *this time*: the binary was being replaced by an
                // upgrade, the machine was out of process slots. **Not a verdict about the service**
                // — nothing was measured, so degrading it would report a bad moment as a sick
                // database — and not a reason to stop asking either, because the next interval is
                // entitled to a different answer. Said once and then only counted, so a fault that
                // lasts an hour is one line in `daemon.log` rather than three hundred.
                Err(error) if error.might_work_later() => {
                    if !complained {
                        complained = true;

                        tracing::warn!(
                            service = self.spec.id().as_str(),
                            error = %error,
                            "this service's health probe could not be made; it will be tried again \
                             at the next interval"
                        );
                    }

                    due = Some(Instant::now() + watching.interval());

                    continue;
                }

                // A probe this build or this machine cannot make. **The service is left alone**,
                // deliberately: it is running and its readiness was proved, and degrading it for a
                // check nobody can make would report a fault in the spec as a fault in the service.
                // Said once, because the answer will not change, and then never probed again.
                Err(error) => {
                    tracing::warn!(
                        service = self.spec.id().as_str(),
                        error = %error,
                        "this service cannot be health-checked here; it will be watched for exiting \
                         and nothing else"
                    );

                    health = None;
                    due = None;

                    continue;
                }
            }

            // Cleared by any probe that was actually made, so a fault that comes back after an hour
            // of working is reported again rather than swallowed by the first one.
            complained = false;

            due = Some(Instant::now() + watching.interval());
        }
    }

    /// Watch a service this daemon adopted: for the daemon asking it to stop, and for it ending.
    ///
    /// **Those two and nothing else, which is what adoption costs.** A health check is not run here
    /// even where the probe would work, because a service that went `Degraded` under it would be put
    /// back by its policy on the strength of a check this daemon has no log to explain the failure
    /// with; and readiness is not re-decided, because the process was proved ready by the daemon
    /// that started it and the check that proved it — a log pattern, most of the time — needs pipes
    /// this one does not have. What the user gets is a service that keeps running, is stopped
    /// properly, and is put back by its policy the moment it crashes, at which point everything is
    /// ordinary again.
    ///
    /// The poll is [`WATCH`], the same one a supervised service is asked at, and the question is the
    /// identity rather than a status: see [`Adopted::exited`].
    async fn watch_adopted(&mut self, adopted: Adopted, restarts: &mut Restarts) -> After {
        loop {
            tokio::select! {
                // Biased towards the stop for the reason the supervised loop is: a daemon on its way
                // out should not spend a whole poll interval on a service it is shutting down.
                biased;

                () = self.cancel.cancelled() => return self.stop_adopted(adopted).await,

                () = tokio::time::sleep(WATCH) => {}
            }

            match adopted.exited() {
                Ok(Some(exit)) => {
                    // The same window the supervised loop publishes `Deciding` for, and for the same
                    // reason: between the process going and `after_exit` persisting what that meant,
                    // a walk must not read the `Up` this service has stopped being.
                    self.readiness.send_replace(Readiness::Deciding);

                    // **An empty capture, not an absent one.** A crash loop decided during an
                    // adopted life has no tail to attach, because the lines went to a pipe that
                    // belonged to a daemon that is gone — which is a fact about this life of the
                    // process and not a fault in the reason, so it is reported as the empty evidence
                    // it is rather than by omitting the reason.
                    let decision =
                        restarts.ended(&exit, std::time::Instant::now(), &Capture::detached());

                    return self.after_exit(decision, exit.code()).await;
                }

                Ok(None) => {}

                // The OS will not say. Nothing to decide on, and the next tick asks again — the same
                // answer the supervised loop gives, and it matters more here: this question is asked
                // of the OS about somebody else's process, so a transient refusal must not be read
                // as the service having ended.
                Err(error) => tracing::warn!(
                    service = self.spec.id().as_str(),
                    error = %error,
                    "cannot tell whether the adopted process is still running"
                ),
            }
        }
    }

    /// Stop a service this daemon adopted, the way its spec asks.
    ///
    /// The shape of [`Runner::stop`], with the one difference adoption forces: this process cannot
    /// *wait* for a process it is not the parent of, so where the supervised path blocks in the
    /// kernel this one polls the identity until it stops answering.
    ///
    /// **A survivor that will not go leaves its row where it is**, which is the only honest answer
    /// available and is also self-healing. Recording `Stopped` for a process that is still holding
    /// the port would be the orphan this whole task exists to prevent, written down as a fact; so
    /// the row keeps its `stopping` and its pid, and the daemon that starts next meets exactly the
    /// case crash recovery already handles — a supervised state with a live process behind it — and
    /// stops it again.
    ///
    /// That row is also what answers the person who asked. This task ends either way, so
    /// [`Registry::stop_one`](super::Registry) reads the row afterwards rather than the task's
    /// ending, and a walk that could not take the service down says so instead of reporting a stop
    /// that did not happen.
    async fn stop_adopted(&self, mut adopted: Adopted) -> After {
        self.move_to(ServiceState::Stopping, StateReason::Requested)
            .await;

        self.ask_to_stop(&mut adopted).await;

        // Killed whatever the polite half achieved, on the same reasoning as the supervised path:
        // the leader exiting is not the workers exiting. On Unix this reaches the group the survivor
        // still leads; on Windows it reaches the one process, the job object having gone with the
        // daemon that made it.
        if let Err(error) = adopted.stop() {
            tracing::warn!(
                service = self.spec.id().as_str(),
                pid = adopted.pid(),
                error = %error,
                "cannot stop the adopted process"
            );
        }

        if !gone(self.spec.id(), &adopted).await {
            tracing::error!(
                service = self.spec.id().as_str(),
                pid = adopted.pid(),
                "this adopted process did not go when it was stopped; its row is left saying so, \
                 for the next daemon to stop it again"
            );

            return After::Done;
        }

        self.record_exit(None).await;

        self.move_to(ServiceState::Stopped, StateReason::Requested)
            .await;

        After::Done
    }

    /// Stop the service the way its spec asks, then record that it is stopped.
    ///
    /// Reached from either side of readiness — from the watch loop, and from a stop that arrived
    /// while the ready check was still running. A walk waiting on this service is answered by the
    /// `Stopping` this begins with rather than by anything at the end: it learns that the service
    /// will not be coming up at the moment that becomes true, instead of sitting through the grace
    /// period of a stop it did not ask for.
    async fn stop(&self, mut supervised: Supervised, capture: Capture) -> After {
        self.move_to(ServiceState::Stopping, StateReason::Requested)
            .await;

        let exit = self.ask_to_stop(&mut supervised).await;

        // Whatever the polite half achieved, the group is killed afterwards: the leader exiting is
        // not the workers exiting, and a php-fpm pool left holding the port is what the next start
        // collides with.
        self.kill(supervised, capture).await;
        self.record_exit(exit.and_then(|exit| exit.code())).await;

        self.move_to(ServiceState::Stopped, StateReason::Requested)
            .await;

        After::Done
    }

    /// Ask the group to leave on its own, and wait as long as the spec says. `None` if it did not.
    ///
    /// Written against [`Stoppable`] rather than against [`Supervised`] because a service this
    /// daemon adopted is stopped by the same `StopBehaviour` as one it started: the spec is the
    /// user's statement about what the *service* needs in order to shut down cleanly, and it does
    /// not become less true because the daemon that spawned the process was killed. What differs
    /// between the two is only what the request travels on, which is the trait's whole surface.
    async fn ask_to_stop(&self, process: &mut dyn Stoppable) -> Option<Exit> {
        // Started before the request rather than after it, which only `Command` can tell the
        // difference: a signal is sent in microseconds, while running `mariadb-admin shutdown` is
        // itself part of what the spec's grace period was written to cover. The rule is T9a's, one
        // level down — whatever the spec allows, minus what has already been spent. The `Command`
        // arm moves it once more, for the one thing that is neither.
        let mut began = Instant::now();

        let grace = match self.spec.stop() {
            // Nothing to ask. Honest rather than a grace period spent on a request nobody sent.
            StopBehaviour::Kill => return None,

            StopBehaviour::Signal { grace } => {
                // ADR 0008: Windows has no request a daemon can send to a process it gave no console
                // to, so the grace period is not spent pretending otherwise.
                if !CAN_ASK_TO_STOP {
                    return None;
                }

                if let Err(error) = process.ask_to_stop() {
                    tracing::warn!(
                        service = self.spec.id().as_str(),
                        error = %error,
                        "cannot ask this service to stop; it will be killed"
                    );

                    return None;
                }

                *grace
            }

            // The polite stop for a service that has something to flush, and on Windows the *only*
            // one there is (ADR 0008). What the command does is ask; what proves it worked is the
            // process going, which is the wait below — a `mariadb-admin shutdown` returns as soon as
            // the server has accepted the instruction, not once it has finished acting on it.
            StopBehaviour::Command {
                program,
                args,
                grace,
            } => {
                let place = self.where_commands_run().await;

                // **The clock starts after this, not before it.** Resolving the environment is the
                // daemon's own preparation rather than any part of the service shutting down, and
                // for an adopted service it is a keyring read allowed the whole of `ENVIRONMENT`.
                // Charged to the grace period it is spent before the request is even sent: a
                // three-second read and a `mariadb-admin shutdown` that returns in two would use up
                // a five-second grace outright, and the service would be killed mid-flush — the
                // "recovery on its next start" the arms below exist to avoid.
                began = Instant::now();

                match place.run(program, args, grace.as_duration()).await {
                    Ok(ran) if ran.succeeded() => *grace,

                    // It ran and refused, or it ran out of the whole grace period. Either way the
                    // service has not been asked successfully and waiting longer buys nothing, so
                    // this falls through to the kill — loudly, and carrying whatever the program
                    // said, because for a database that kill is a recovery on its next start and
                    // `ERROR 1045: Access denied` is the whole of what the user has to act on.
                    Ok(ran) => {
                        if let Some(exit) = self.ended_meanwhile(process) {
                            tracing::info!(
                                service = self.spec.id().as_str(),
                                program = %program.display(),
                                timed_out = ran.timed_out(),
                                complaint = ran.complaint().unwrap_or("it said nothing"),
                                "this service's stop command did not report success, but the \
                                 service stopped anyway"
                            );

                            return Some(exit);
                        }

                        tracing::error!(
                            service = self.spec.id().as_str(),
                            program = %program.display(),
                            timed_out = ran.timed_out(),
                            complaint = ran.complaint().unwrap_or("it said nothing"),
                            "this service's stop command did not work; killing it instead, which \
                             may leave it to recover on its next start"
                        );

                        return None;
                    }

                    // The program a spec names is not on this machine, or cannot be started. A
                    // spec to fix rather than a service to blame, and the service still has to stop.
                    Err(error) => {
                        let ended = self.ended_meanwhile(process);

                        // Said either way: a program a spec names and this machine does not have is
                        // a spec to fix, and it stays broken whether or not the service happened to
                        // go by itself this time.
                        tracing::error!(
                            service = self.spec.id().as_str(),
                            program = %program.display(),
                            error = %error,
                            killing = ended.is_none(),
                            "cannot run this service's stop command"
                        );

                        return ended;
                    }
                }
            }

            // `StopBehaviour` is `#[non_exhaustive]`. A behaviour this build does not know is not a
            // licence to invent one, and the service still has to stop.
            other => {
                tracing::warn!(
                    service = self.spec.id().as_str(),
                    behaviour = ?other,
                    "unknown stop behaviour; killing the service"
                );

                return None;
            }
        };

        let deadline = began + grace.as_duration();

        loop {
            match process.exited() {
                Ok(Some(exit)) => return Some(exit),
                Ok(None) => {}
                Err(error) => {
                    tracing::warn!(
                        service = self.spec.id().as_str(),
                        error = %error,
                        "cannot tell whether this service has stopped; it will be killed"
                    );

                    return None;
                }
            }

            if Instant::now() >= deadline {
                tracing::info!(
                    service = self.spec.id().as_str(),
                    grace = %grace,
                    "this service did not stop when asked; killing it"
                );

                return None;
            }

            tokio::time::sleep(POLL).await;
        }
    }

    /// Whether the service ended by itself while it was being asked to stop.
    ///
    /// **The one thing a failed stop request must not lose.** Running `mariadb-admin shutdown` is a
    /// whole grace period's worth of time, and a server that took the instruction and exited inside
    /// that window has stopped exactly as it was asked to — even if the program carrying the
    /// instruction then returned non-zero, or ran out of patience waiting for a server that had
    /// already gone. Answering [`None`] there records no exit code at all and reports a kill that
    /// never happened, on a service that shut down cleanly.
    ///
    /// The wait loop below reads this every `POLL`; the arms that return early have to read it once
    /// themselves, because they are the paths that do not reach the loop.
    fn ended_meanwhile(&self, process: &mut dyn Stoppable) -> Option<Exit> {
        match process.exited() {
            Ok(exit) => exit,

            // Not knowing is treated as still running, which is the safe half: the caller kills, and
            // killing a process that has already gone costs nothing.
            Err(error) => {
                tracing::warn!(
                    service = self.spec.id().as_str(),
                    error = %error,
                    "cannot tell whether this service has stopped"
                );

                None
            }
        }
    }

    /// What happens after the process ended by itself.
    async fn after_exit(&self, decision: Decision, code: Option<i32>) -> After {
        self.record_exit(code).await;

        match decision {
            // It did what it was asked to. Through `Stopping`, which is the only edge the machine
            // has into `Stopped` — a service that exited cleanly is stopping for exactly as long as
            // these two writes take.
            Decision::Rest { reason } => {
                self.move_to(ServiceState::Stopping, reason.clone()).await;
                self.move_to(ServiceState::Stopped, reason).await;

                After::Done
            }

            Decision::GiveUp { reason } => self.give_up(reason).await,

            // The `Restarting` is what answers a walk that is waiting on this service, which is why
            // a failure to persist it ends the task: a runner that went on restarting from a row
            // nobody could read would leave that walk with nothing to wait for.
            Decision::Restart { after, attempt } => {
                if !self
                    .move_to(ServiceState::Restarting, StateReason::Exited { code })
                    .await
                {
                    return After::Done;
                }

                After::Again { after, attempt }
            }
        }
    }

    /// Wait out a backoff, unless something happens that is worth more than the rest of the wait.
    ///
    /// **The bias is the priority order.** A stop beats a start request, because a daemon on its way
    /// out is not going to spawn one more process; a start request beats the remaining wait, which is
    /// the whole of T19c — a person who has just typed `mix service start` at a service in its thirty
    /// second backoff is asking for something the runner would otherwise make them sit through.
    async fn wait_out(&self, after: Duration) -> Released {
        tokio::select! {
            biased;

            () = self.cancel.cancelled() => {
                // Nothing is running to stop — the process is already gone — so this goes straight
                // through `Stopping` to the state a user asked for.
                self.move_to(ServiceState::Stopping, StateReason::Requested).await;
                self.move_to(ServiceState::Stopped, StateReason::Requested).await;

                Released::Stopped
            }

            () = self.asked_to_start.notified() => Released::Asked,

            () = tokio::time::sleep(after) => Released::Elapsed,
        }
    }

    /// Take the request this start has just answered, so that no later crash is released by it.
    ///
    /// **A permit is only ever consumed by [`Runner::wait_out`], and a runner that is mid-start is
    /// not in one.** A request that arrives while `ready::wait` is running — two walks sharing a
    /// dependency, which is the ordinary case and not the rare one — is kept by the [`Notify`] until
    /// something waits on it. If this start then *succeeds*, nothing does for as long as the service
    /// stays up: the permit outlives the request entirely, and the next crash — hours later and
    /// asked for by nobody — leaves its backoff the instant it enters it, with the ladder reset by
    /// [`Restarts::recovered`] and the move published as [`StateReason::Requested`].
    ///
    /// Reaching `Running` is what makes such a request *answered* rather than dropped: whoever asked
    /// for this service to be started now has it started now, which is the whole of what they asked
    /// for. A request that arrives after this is about the life that follows, and [`Runner::wait_out`]
    /// is where it belongs.
    fn answered_by_this_start(&self) {
        let asked = std::pin::pin!(self.asked_to_start.notified());

        // `enable` is how a stored permit is taken without waiting for one: it registers this
        // future's interest and says whether there was already something to receive. The future is
        // dropped on the next line, which is what makes this a read and not a wait.
        if asked.enable() {
            tracing::debug!(
                service = self.spec.id().as_str(),
                "a start asked for while this service was starting is answered by that start"
            );
        }
    }

    /// Move to `Failed` for `reason`. Whoever is waiting on this service is answered by that move.
    async fn give_up(&self, reason: StateReason) -> After {
        self.move_to(ServiceState::Failed, reason).await;

        After::Done
    }

    /// Kill whatever is left of the group and collect the last lines it printed.
    ///
    /// In that order: killing first is what makes the drain finish, because a worker still holding a
    /// copy of the service's stdout keeps the pipe open long after the leader has gone.
    ///
    /// Off the runtime, because both halves block — `.claude/standards/rust.md` requires it of
    /// anything that waits. A blocking task that panicked leaves an empty capture rather than taking
    /// the supervisor of every other service down with it.
    async fn kill(&self, supervised: Supervised, mut capture: Capture) -> Capture {
        let service = self.spec.id().clone();

        tokio::task::spawn_blocking(move || {
            // Its `Drop` is the kill: the group goes, whether or not the leader had already exited.
            drop(supervised);

            if !capture.finish(FLUSH) {
                tracing::warn!(
                    service = service.as_str(),
                    "the last lines of this service were not read before it was let go"
                );
            }

            capture
        })
        .await
        .unwrap_or_else(|error| {
            tracing::error!(%error, "the task stopping a service did not finish");

            Capture::detached()
        })
    }

    /// Record that no process belongs to this service any more.
    async fn record_exit(&self, code: Option<i32>) {
        if let Err(error) = services::ended(&self.store, self.spec.id(), code).await {
            tracing::error!(
                service = self.spec.id().as_str(),
                error = %error,
                "cannot record that this service's process has ended; the row still names a pid \
                 that has gone"
            );
        }
    }

    /// Persist a state change, publish the value that was persisted, and say what the service now is
    /// to anything waiting on it. `false` if the change did not land.
    ///
    /// The three in that order, and the readiness last for the same reason the event is not first:
    /// nothing may be told about a move that did not happen. A state that would not persist leaves
    /// the readiness as it was — the service really is still whatever the row still says.
    async fn move_to(&self, to: ServiceState, reason: StateReason) -> bool {
        let persisted = super::record(
            &self.store,
            &self.events,
            self.spec.id(),
            to,
            reason.clone(),
        )
        .await;

        if !persisted {
            return false;
        }

        // Sent whether or not anything is listening: `send_replace` keeps the value for whoever asks
        // next, where `send` would report a walk that has already had its answer as a failure.
        self.readiness
            .send_replace(Readiness::of(to, reason, self.spec.restart()));

        true
    }

    /// Where this service's own commands are run — its directory, and the environment it is running
    /// with.
    ///
    /// The cached one whenever there is one, which is every life of a process this daemon spawned.
    /// **A service it adopted has none**, and that is the case worth spelling out: the environment
    /// was built by a daemon that is gone, so a stop command for a survivor resolves one here — once,
    /// at the moment it is stopped, which is the only moment an adopted service runs a command at
    /// all.
    ///
    /// An environment that cannot be resolved is not a reason to skip the stop: a spec whose
    /// credential the keyring no longer holds still names a `mariadb-admin shutdown` that is far
    /// better than a kill, and the alternative to trying it with what is available is a database
    /// recovering on its next start. It is said once, in `daemon.log`, and never in the event.
    ///
    /// **What is dropped is the entry that failed, and only that entry.** The whole environment
    /// would be the wrong thing to throw away for one unreadable credential: a `mariadb-admin`
    /// run without the `HOME` the spec declares as a literal cannot find its defaults file or its
    /// socket, so a stop that was meant to survive a locked keyring would fail a second time for a
    /// reason nobody chose.
    ///
    /// **And an environment that will not *arrive* is the same answer**, which is why [`ENVIRONMENT`]
    /// bounds the read. The uncached path is only ever taken while a service is being stopped, and a
    /// keyring that is waiting for somebody to type a password would otherwise hold a `mix stop` —
    /// or a whole daemon shutdown — open indefinitely. Giving up on the read leaves the blocking task
    /// where it is, still waiting on the store; what it does not do is make the stop wait with it —
    /// and the literals are still known here, without the task that has stopped answering.
    async fn where_commands_run(&self) -> Surroundings {
        if let Some(place) = &self.surroundings {
            return place.clone();
        }

        let literals = || {
            self.spec
                .env()
                .iter()
                .filter_map(|(name, value)| match value {
                    EnvValue::Literal { value } => Some((name.clone(), value.clone())),
                    EnvValue::Keyring { .. } => None,
                })
                .collect::<BTreeMap<String, String>>()
        };

        let env = match tokio::time::timeout(ENVIRONMENT, self.walk_environment(OnFailure::Record))
            .await
        {
            Ok(Ok((env, failed))) => {
                for (name, error) in failed {
                    tracing::warn!(
                        service = self.spec.id().as_str(),
                        entry = name.as_str(),
                        error = %error,
                        "cannot resolve one entry of the environment this service is running with; \
                         its own commands will be run without that entry"
                    );
                }

                env
            }

            Ok(Err(error)) => {
                tracing::warn!(
                    service = self.spec.id().as_str(),
                    error = %error,
                    "cannot resolve the environment this service is running with; its own commands \
                     will be run with the entries the spec states outright"
                );

                literals()
            }

            Err(_) => {
                tracing::warn!(
                    service = self.spec.id().as_str(),
                    after = ?ENVIRONMENT,
                    "the environment this service is running with did not resolve in time — a \
                     locked OS keyring is the usual reason; its own commands will be run with the \
                     entries the spec states outright"
                );

                literals()
            }
        };

        Surroundings::new(self.spec.cwd(), env)
    }

    /// The environment the child is given: the spec's, with every named credential fetched.
    ///
    /// A named credential that is not there fails the start rather than being passed as an empty
    /// string: a MariaDB started with no root password is a worse outcome than one that did not
    /// start. The first entry that would not resolve is the error, named — the rest are neither
    /// worth listing nor worth asking for, which is why the walk stops there ([`OnFailure::Stop`]).
    async fn environment(&self) -> anyhow::Result<BTreeMap<String, String>> {
        let (env, failed) = self.walk_environment(OnFailure::Stop).await?;

        match failed.into_iter().next() {
            Some((name, error)) => Err(error.context(format!("the environment entry {name}"))),
            None => Ok(env),
        }
    }

    /// Walk the spec's environment: every entry that resolves, and the error of each that did not.
    ///
    /// Off the runtime because a keyring read blocks — on Linux on a D-Bus round trip to a daemon
    /// that may be prompting the user to unlock it.
    ///
    /// `on_failure` is what lets the two callers differ: a start refuses anything less than the
    /// whole environment and so has nothing to gain from the entries past the first failure, a stop
    /// command runs with whatever there is. The [`Err`] here is neither — it is the blocking task
    /// itself not finishing, which says nothing about any entry.
    async fn walk_environment(&self, on_failure: OnFailure) -> anyhow::Result<Resolved> {
        let named: Vec<(String, EnvValue)> = self
            .spec
            .env()
            .iter()
            .map(|(name, value)| (name.clone(), value.clone()))
            .collect();

        let host = Arc::clone(&self.host);

        Ok(tokio::task::spawn_blocking(move || {
            let mut env = BTreeMap::new();
            let mut failed = Vec::new();

            for (name, value) in named {
                let resolved = match value {
                    EnvValue::Literal { value } => Ok(value),

                    EnvValue::Keyring { service, key } => host
                        .keyring()
                        .secret(&service, &key)
                        .map_err(anyhow::Error::from)
                        .and_then(|secret| {
                            secret.ok_or_else(|| {
                                anyhow::anyhow!("no credential is stored at {service}/{key}")
                            })
                        }),
                };

                match resolved {
                    Ok(value) => {
                        env.insert(name, value);
                    }

                    Err(error) => {
                        failed.push((name, error));

                        if matches!(on_failure, OnFailure::Stop) {
                            break;
                        }
                    }
                }
            }

            (env, failed)
        })
        .await?)
    }
}

/// Poll an adopted process until it has gone. `false` if it had not within [`GONE`].
///
/// **A free function because both halves of T18 need it and neither owns the other**: this runner
/// waits here when it is asked to stop a service it took over, and [`Registry::discard`] waits here
/// before it clears the row of a survivor it refused. The two are the same claim — nothing may be
/// written down as stopped while the process it names is still running — and writing it twice is how
/// they would come to disagree.
///
/// [`Registry::discard`]: super::Registry
pub(super) async fn gone(service: &ServiceId, adopted: &Adopted) -> bool {
    let deadline = Instant::now() + GONE;

    loop {
        match adopted.exited() {
            Ok(Some(_)) => return true,

            Ok(None) => {}

            // Unanswerable is not gone. The deadline below is what ends this either way.
            Err(error) => tracing::warn!(
                service = service.as_str(),
                error = %error,
                "cannot tell whether the adopted process has stopped"
            ),
        }

        if Instant::now() >= deadline {
            return false;
        }

        tokio::time::sleep(POLL).await;
    }
}

/// Turn a supervisor refusal into the reason a user reads.
///
/// [`Error::UnsupportedCheck`](mixengine_supervisor::Error::UnsupportedCheck) already carries both
/// halves in the words the spec's author needs, so they are passed through rather than re-worded.
/// Everything else — a pattern that will not compile, a socket this OS does not have — is described
/// by its own chain, which is where `mixengine-platform` writes what it refused and why.
fn uncheckable(error: &mixengine_supervisor::Error) -> StateReason {
    match error {
        mixengine_supervisor::Error::UnsupportedCheck { check, reason } => {
            StateReason::Uncheckable {
                check: (*check).to_owned(),
                reason: reason.clone(),
            }
        }

        // A pattern that will not compile, a socket this OS does not have. Flattened rather than
        // printed, because these types carry the cause as a `source` precisely so that no layer
        // repeats it — and the layer showing it to a person is the one that has to join it back up.
        other => StateReason::Uncheckable {
            check: "the readiness check this service declares".to_owned(),
            reason: mixengine_proto::flatten(other),
        },
    }
}
