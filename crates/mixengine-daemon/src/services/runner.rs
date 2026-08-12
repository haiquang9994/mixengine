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
use mixengine_platform::process::{self, CAN_ASK_TO_STOP, Exit, Supervised};
use mixengine_proto::{
    EnvValue, RestartPolicy, ServiceSpec, ServiceState, StateReason, StopBehaviour,
};
use mixengine_supervisor::logs::Capture;
use mixengine_supervisor::{Decision, Health, Restarts, Verdict, ready};
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

/// How long the last lines of a stopped service are waited for.
///
/// Bounded because end of file is not the process exiting but the *last holder of the pipe*
/// exiting — see [`Capture::finish`], which explains why an unbounded wait here would hang the
/// supervisor at the one moment it has something to report.
const FLUSH: Duration = Duration::from_secs(2);

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

    /// Where this runner says whether its service is usable.
    ///
    /// A [`watch`] rather than a one-shot, because the question is asked more than once and by more
    /// than the walk that started the service: the answer has to be *current* for whoever asks next,
    /// where a one-shot would leave the walk after it reading the outcome of a start that ended an
    /// hour ago. Dropped with this runner, which is how the registry learns that a task ended
    /// without deciding.
    pub(super) readiness: watch::Sender<Readiness>,
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
        let mut reason = StateReason::Requested;

        loop {
            // A `Starting` that will not persist ends this task with the readiness still undecided,
            // deliberately: the failure is the daemon's own, it is in `daemon.log`, and it is not a
            // state a client could render — which is what the registry reports for it.
            if !self.move_to(ServiceState::Starting, reason).await {
                return;
            }

            match self.attempt(&mut restarts).await {
                After::Done => return,

                After::Again { after, attempt } => match self.wait_out(after).await {
                    Released::Stopped => return,

                    Released::Elapsed => reason = StateReason::BackoffElapsed { attempt },

                    // **A person asking is not the policy coming round again**, and the difference is
                    // what `Restarts::recovered` records: the wait goes back to the shortest the
                    // policy allows, while the failure history stays, because a service somebody has
                    // restarted four times is still a service that has crashed four times.
                    Released::Asked => {
                        restarts.recovered();
                        reason = StateReason::Requested;
                    }
                },
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

        // Before anything waits on the process: a pipe nobody drains stops the service writing to
        // it, and a ready check that matches a log pattern has nothing to match against until this
        // exists.
        let capture = Capture::start(
            &mut supervised,
            self.spec.id(),
            self.spec.logs(),
            Some(&self.directory),
        );

        // The three columns nothing wrote before T19. `pid_start_time` is `None` until the platform
        // reading T18 owns exists — a null column is what adoption refuses, where a zero would look
        // like an answer.
        if let Err(error) =
            services::started(&self.store, self.spec.id(), supervised.pid(), None, now()).await
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

            match watching.examine().await {
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

                // A probe this build cannot make. **The service is left alone**, deliberately: it is
                // running and its readiness was proved, and degrading it for a check nobody can make
                // would report a fault in the spec as a fault in the service. Said once, because the
                // answer will not change, and then never probed again.
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

            due = Some(Instant::now() + watching.interval());
        }
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
    async fn ask_to_stop(&self, supervised: &mut Supervised) -> Option<Exit> {
        let grace = match self.spec.stop() {
            // Nothing to ask. Honest rather than a grace period spent on a request nobody sent.
            StopBehaviour::Kill => return None,

            StopBehaviour::Signal { grace } => {
                // ADR 0008: Windows has no request a daemon can send to a process it gave no console
                // to, so the grace period is not spent pretending otherwise.
                if !CAN_ASK_TO_STOP {
                    return None;
                }

                if let Err(error) = supervised.ask_to_stop() {
                    tracing::warn!(
                        service = self.spec.id().as_str(),
                        error = %error,
                        "cannot ask this service to stop; it will be killed"
                    );

                    return None;
                }

                *grace
            }

            // Owed to Phase 3, where the first service that needs one arrives (T33's
            // `mariadb-admin shutdown`). Running a command is `mixengine-platform`'s to offer and
            // this crate must not reach around it, so what happens meanwhile is a kill and a line
            // saying so — never a silent one, because for a database it is a recovery on next boot.
            StopBehaviour::Command { program, .. } => {
                tracing::error!(
                    service = self.spec.id().as_str(),
                    program = %program.display(),
                    "this build cannot run a stop command (roadmap task T15a); killing the service \
                     instead, which may leave it to recover on its next start"
                );

                return None;
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

        let deadline = Instant::now() + grace.as_duration();

        loop {
            match supervised.exited() {
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

    /// The environment the child is given: the spec's, with every named credential fetched.
    ///
    /// Off the runtime because a keyring read blocks — on Linux on a D-Bus round trip to a daemon
    /// that may be prompting the user to unlock it.
    ///
    /// A named credential that is not there fails the start rather than being passed as an empty
    /// string: a MariaDB started with no root password is a worse outcome than one that did not
    /// start.
    async fn environment(&self) -> anyhow::Result<BTreeMap<String, String>> {
        let named: Vec<(String, EnvValue)> = self
            .spec
            .env()
            .iter()
            .map(|(name, value)| (name.clone(), value.clone()))
            .collect();

        let host = Arc::clone(&self.host);

        tokio::task::spawn_blocking(move || {
            let mut env = BTreeMap::new();

            for (name, value) in named {
                let resolved = match value {
                    EnvValue::Literal { value } => value,

                    EnvValue::Keyring { service, key } => {
                        host.keyring().secret(&service, &key)?.ok_or_else(|| {
                            anyhow::anyhow!("no credential is stored at {service}/{key}")
                        })?
                    }
                };

                env.insert(name, resolved);
            }

            Ok(env)
        })
        .await?
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
