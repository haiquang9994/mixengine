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
use mixengine_proto::{EnvValue, ServiceSpec, ServiceState, StateReason, StopBehaviour};
use mixengine_supervisor::logs::Capture;
use mixengine_supervisor::{Decision, Health, Restarts, Verdict, ready};
use tokio::sync::oneshot;
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

/// How the first start of a service ended, for the walk that is waiting on it.
///
/// Only the first: a service that crashes an hour later has an event to say so, and nothing is
/// still holding the other end of this.
#[derive(Debug)]
pub(super) enum Start {
    /// The ready check passed. Traffic can be routed to it, and the next tier may start.
    Ready,

    /// It did not come up, and this is what was persisted.
    ///
    /// [`None`] when the failure was the daemon's own — a database that would not take the write —
    /// which is in `daemon.log` and is not a state a client could render.
    Failed(Option<StateReason>),
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

impl Runner {
    /// Supervise this service until it is stopped, gives up, or the daemon does.
    ///
    /// `announce` reports the **first** start only, which is what makes a tiered walk possible: the
    /// tier below may begin the moment this service is ready, and a service that never becomes ready
    /// stops the walk instead of leaving it waiting for a process that is not coming.
    pub(super) async fn run(mut self, announce: oneshot::Sender<Start>) {
        let mut announce = Some(announce);
        let mut restarts = Restarts::under(self.spec.restart());
        let mut reason = StateReason::Requested;

        loop {
            if !self.move_to(ServiceState::Starting, reason).await {
                self.report(&mut announce, Start::Failed(None));
                return;
            }

            match self.attempt(&mut restarts, &mut announce).await {
                After::Done => return,

                After::Again { after, attempt } => {
                    if !self.wait_out(after).await {
                        self.report(&mut announce, Start::Failed(Some(StateReason::Requested)));
                        return;
                    }

                    reason = StateReason::BackoffElapsed { attempt };
                }
            }
        }
    }

    /// One life of the process: spawn it, wait for readiness, then watch it until it ends.
    async fn attempt(
        &mut self,
        restarts: &mut Restarts,
        announce: &mut Option<oneshot::Sender<Start>>,
    ) -> After {
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

                return self.give_up(StateReason::SpawnFailed, announce).await;
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

                    return self.give_up(StateReason::SpawnFailed, announce).await;
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
            return self.stop(supervised, capture, announce).await;
        };

        match outcome {
            Ok(ready::Ready::Ready) => {
                if !self
                    .move_to(ServiceState::Running, StateReason::Ready)
                    .await
                {
                    self.kill(supervised, capture).await;
                    self.report(announce, Start::Failed(None));

                    return After::Done;
                }

                self.report(announce, Start::Ready);
                self.supervise(supervised, capture, restarts, announce)
                    .await
            }

            // The most common way a service fails to start, and the reason `ready::wait` races the
            // exit rather than polling the probe: this is the same path a crash an hour from now
            // takes, and it is the restart policy that decides between them.
            Ok(ready::Ready::Exited(exit)) => {
                let capture = self.kill(supervised, capture).await;
                let decision = restarts.ended(&exit, std::time::Instant::now(), &capture);

                self.after_exit(decision, exit.code(), announce).await
            }

            // Running, and never going to be usable. Killed rather than left: the next attempt would
            // collide with the port and the data directory this one is holding.
            Ok(ready::Ready::TimedOut) => {
                let after = self.spec.ready().timeout();
                self.kill(supervised, capture).await;
                self.record_exit(None).await;

                self.give_up(StateReason::ReadyTimeout { after }, announce)
                    .await
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

                self.give_up(reason, announce).await
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
        announce: &mut Option<oneshot::Sender<Start>>,
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
                    return self.stop(supervised, capture, announce).await;
                }

                () = tokio::time::sleep_until(wake) => {}
            }

            match supervised.exited() {
                Ok(Some(exit)) => {
                    let capture = self.kill(supervised, capture).await;
                    let decision = restarts.ended(&exit, std::time::Instant::now(), &capture);

                    return self.after_exit(decision, exit.code(), announce).await;
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
    /// while the ready check was still running — which is why it reports to `announce` at the end:
    /// a walk waiting on this service learns that it will not be coming up, rather than waiting on
    /// a task that has already finished.
    async fn stop(
        &self,
        mut supervised: Supervised,
        capture: Capture,
        announce: &mut Option<oneshot::Sender<Start>>,
    ) -> After {
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
        self.report(announce, Start::Failed(Some(StateReason::Requested)));

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
    async fn after_exit(
        &self,
        decision: Decision,
        code: Option<i32>,
        announce: &mut Option<oneshot::Sender<Start>>,
    ) -> After {
        self.record_exit(code).await;

        match decision {
            // It did what it was asked to. Through `Stopping`, which is the only edge the machine
            // has into `Stopped` — a service that exited cleanly is stopping for exactly as long as
            // these two writes take.
            Decision::Rest { reason } => {
                self.move_to(ServiceState::Stopping, reason.clone()).await;
                self.move_to(ServiceState::Stopped, reason.clone()).await;
                self.report(announce, Start::Failed(Some(reason)));

                After::Done
            }

            Decision::GiveUp { reason } => self.give_up(reason, announce).await,

            Decision::Restart { after, attempt } => {
                if !self
                    .move_to(ServiceState::Restarting, StateReason::Exited { code })
                    .await
                {
                    self.report(announce, Start::Failed(None));

                    return After::Done;
                }

                After::Again { after, attempt }
            }
        }
    }

    /// Wait out a backoff. `false` if the service was asked to stop while waiting.
    async fn wait_out(&self, after: Duration) -> bool {
        tokio::select! {
            () = self.cancel.cancelled() => {
                // Nothing is running to stop — the process is already gone — so this goes straight
                // through `Stopping` to the state a user asked for.
                self.move_to(ServiceState::Stopping, StateReason::Requested).await;
                self.move_to(ServiceState::Stopped, StateReason::Requested).await;

                false
            }

            () = tokio::time::sleep(after) => true,
        }
    }

    /// Move to `Failed` for `reason`, and tell whoever is waiting on the first start.
    async fn give_up(
        &self,
        reason: StateReason,
        announce: &mut Option<oneshot::Sender<Start>>,
    ) -> After {
        self.move_to(ServiceState::Failed, reason.clone()).await;
        self.report(announce, Start::Failed(Some(reason)));

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

    /// Persist a state change and publish the value that was persisted. `false` if it did not land.
    async fn move_to(&self, to: ServiceState, reason: StateReason) -> bool {
        super::record(&self.store, &self.events, self.spec.id(), to, reason).await
    }

    /// Tell the walk how the first start went, once.
    fn report(&self, announce: &mut Option<oneshot::Sender<Start>>, outcome: Start) {
        if let Some(waiting) = announce.take() {
            // A receiver that has gone is a walk that gave up on this service, which is not this
            // task's problem to report.
            let _ = waiting.send(outcome);
        }
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
