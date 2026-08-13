//! The registry of running services: what the daemon is supervising, and how a plan is walked.
//!
//! **This is where the timing lives.** `mixengine-supervisor` has the mechanisms and no loop;
//! `mixengine-core` has the graph, the state machine and the row; the daemon is what holds a task
//! per service, the [`CancellationToken`] it stops on, and the clock both of those are measured by.
//! Roadmap task **T19**.
//!
//! It is also where a daemon's first act lives: [`Registry::recover`] reconciles the rows the last
//! daemon left behind — adopting the processes that survived it, stopping the ones nothing can
//! supervise, and clearing the rows whose process is gone. Roadmap task **T18**, and the reason this
//! module reads a `services` row it did not write.
//!
//! A walk is **sequential over [`Plan::flat`]**, which is what T17 left this free to be: the tiers
//! are already computed, so M3's ten-second budget buys concurrency by changing this walker and
//! nothing else. A tier that fails stops the walk, and everything below it is marked
//! [`StateReason::DependencyFailed`] rather than spawned against a dependency that is not there.

#[cfg(test)]
pub(crate) mod fixture;
mod runner;
mod spec;

use std::collections::{BTreeSet, HashMap};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex, MutexGuard, PoisonError};
use std::time::SystemTime;

use mixengine_core::services::{self, Plan, ServiceGraph};
use mixengine_core::{Paths, Store};
use mixengine_platform::Host;
use mixengine_platform::process::{Adopted, StartTime};
use mixengine_proto::{DaemonEvent, ServiceId, ServiceSpec, ServiceState, StateReason, Timestamp};
use tokio::sync::{Notify, watch};
use tokio::task::JoinHandle;
use tokio_util::sync::CancellationToken;

use crate::api::Events;
use runner::{Readiness, Runner, gone};

#[cfg(test)]
pub(crate) use spec::Undeclared;
pub(crate) use spec::{SpecSource, declared};

/// Everything the daemon is supervising, and the only thing that starts or stops one.
#[derive(Debug)]
pub(crate) struct Registry {
    /// Where a service's `current.log` goes.
    paths: Paths,

    /// The state rows. Every move is written here before it is announced.
    store: Store,

    /// The OS, for the credentials a spec names and cannot carry.
    host: Arc<dyn Host>,

    /// Where a persisted transition is published.
    events: Events,

    /// Where a [`ServiceSpec`] comes from — see [`spec`].
    specs: Arc<dyn SpecSource>,

    /// The daemon's root token. Every runner's token is a child of this one, so nothing this
    /// registry spawns can outlive the daemon.
    shutdown: CancellationToken,

    /// One entry per service with a task supervising it.
    ///
    /// A `std` mutex rather than tokio's: nothing awaits while holding it, and the alternative
    /// would make every reader of "what is running" an async function for no reason.
    running: Arc<Mutex<HashMap<ServiceId, Running>>>,

    /// Hands out the generation below.
    generations: AtomicU64,
}

/// One service being supervised.
#[derive(Debug)]
struct Running {
    /// Cancel it to stop the service the way its spec asks.
    cancel: CancellationToken,

    /// Notify it to ask the runner to start the service *now*.
    ///
    /// **The one thing the registry can do to a live runner besides read it**, and the whole of
    /// T19c: a runner sitting out a restart backoff is not reachable through [`Running::readiness`],
    /// which is a report and not a request, so an explicit start had nothing to act on. See
    /// [`Registry::begin`].
    asked_to_start: Arc<Notify>,

    /// The runner, so a stop can wait for it rather than assume.
    task: JoinHandle<()>,

    /// Which run of this service this is.
    ///
    /// What keeps a task that is ending from removing an entry that is no longer its own: a service
    /// that fails and is started again by the same walk has two tasks alive for an instant, and
    /// without this the older one's tidy-up would deregister the newer one.
    generation: u64,

    /// What that runner last decided about the service, which is the only thing that says whether it
    /// is up. **Not the task's liveness** — see [`Readiness`].
    readiness: watch::Receiver<Readiness>,
}

/// How a start of one service ended, for the walk that is waiting on it.
#[derive(Debug)]
enum Start {
    /// The service is up. Traffic can be routed to it, and the next tier may start.
    Ready,

    /// It is not, and this is what was persisted about why.
    ///
    /// [`None`] when the failure was the daemon's own — a database that would not take the write, a
    /// runner task that panicked — which is in `daemon.log` and is not a state a client could render.
    Failed(Option<StateReason>),
}

/// What a walk did.
///
/// Not a `Result`: a plan of six services where the fourth fails has three that are running, one
/// that failed and two that were never tried, and a caller that has to render that needs all three
/// lists. T19a's `service.start` is the first such caller.
#[derive(Debug, Default)]
pub(crate) struct Walk {
    /// Services that reached what the walk was aiming for, in the order they got there.
    pub(crate) reached: Vec<ServiceId>,

    /// The service that stopped the walk, and what was persisted about it.
    ///
    /// [`None`] as the reason when the failure was the daemon's own — see [`Start::Failed`] — and
    /// for the one failure a *stop* has, which is a survivor that would not die: there is no
    /// persisted reason to quote there, because the row is deliberately left in the state it was
    /// already in. See [`Registry::stop`].
    pub(crate) failed: Option<(ServiceId, Option<StateReason>)>,

    /// Services never tried, because something they depend on failed.
    pub(crate) blocked: Vec<ServiceId>,
}

/// What one boot's reconciliation found — roadmap task **T18**.
///
/// Lists rather than counts, because the one caller that is not a test writes them into
/// `daemon.log`: "adopted mariadb@main" is the line somebody reads a week later to understand why a
/// database had been up for longer than the daemon watching it, and a number would not answer that.
///
/// **Every row this touched is in exactly one of them**, including the ones it could not finish
/// with. A reconciliation that reported only its successes would let the summary line say nothing
/// happened in the very boot where a survivor refused to die — which is the boot somebody is reading
/// the log for.
#[derive(Debug, Default)]
pub(crate) struct Recovery {
    /// Services whose process survived and is now supervised again.
    pub(crate) adopted: Vec<ServiceId>,

    /// Survivors this daemon stopped rather than adopt: nothing declares them, or they were left in
    /// a state adoption cannot resume.
    pub(crate) stopped: Vec<ServiceId>,

    /// Rows that claimed a process which was not there. Nothing was signalled for these.
    pub(crate) cleared: Vec<ServiceId>,

    /// Survivors this daemon meant to stop and could not, whose rows are therefore **left as they
    /// were found** — still naming the process, still in a supervised state.
    ///
    /// Not a failure of the boot: the daemon serves clients either way, and the next one meets the
    /// same case and tries again. It is here because it is the one outcome that leaves the machine
    /// holding a port nothing supervises, and it must not be reported as quiet.
    pub(crate) refused: Vec<ServiceId>,
}

impl Recovery {
    /// Whether there was anything to reconcile at all, which is the ordinary case.
    pub(crate) fn is_empty(&self) -> bool {
        self.adopted.is_empty()
            && self.stopped.is_empty()
            && self.cleared.is_empty()
            && self.refused.is_empty()
    }
}

/// Why the declared services could not be assembled into a graph.
///
/// **Two variants because they are two different people's problem**, and the wire mapping has to
/// tell them apart: a source that failed is the daemon's and is an internal error, while a set that
/// is not a graph is what the user declared and is `invalid_argument` — the mapping T17 fixed, which
/// [`crate::error::ToWire`] already applies to [`mixengine_core::Error::Graph`].
///
/// Not an [`std::error::Error`] itself: nothing wraps it, and the one caller matches it and hands
/// each half to the mapping that already exists for it — [`crate::error::ToWire`], where the
/// `service.*` handlers meet it.
#[derive(Debug)]
pub(crate) enum Undeclarable {
    /// The source could not produce them: a package that is not installed, a template that does not
    /// render, a database that cannot be read.
    Unavailable(anyhow::Error),

    /// They are not a graph: a cycle, a dependency on something that is not declared, two services
    /// with the same id.
    Invalid(mixengine_core::Error),
}

impl Registry {
    /// A registry with nothing running.
    pub(crate) fn new(
        paths: &Paths,
        store: &Store,
        host: Arc<dyn Host>,
        events: Events,
        specs: Arc<dyn SpecSource>,
        shutdown: CancellationToken,
    ) -> Self {
        Self {
            paths: paths.clone(),
            store: store.clone(),
            host,
            events,
            specs,
            shutdown,
            running: Arc::new(Mutex::new(HashMap::new())),
            generations: AtomicU64::new(0),
        }
    }

    /// The declared services, checked and ordered.
    ///
    /// Asked afresh rather than cached, because the set is a *rendering* of state that changes —
    /// a service created, a port edited, a package upgraded — and a graph held from startup would
    /// answer for a home that no longer exists.
    ///
    /// # Errors
    ///
    /// [`Undeclarable`], which keeps the two apart on purpose: a source that failed is the daemon's
    /// problem, and a set that is not a graph is the user's declaration and belongs in
    /// `invalid_argument`.
    pub(crate) async fn graph(&self) -> Result<ServiceGraph, Undeclarable> {
        let specs = self
            .specs
            .declared()
            .await
            .map_err(Undeclarable::Unavailable)?;

        ServiceGraph::new(specs)
            .map_err(|error| Undeclarable::Invalid(mixengine_core::Error::Graph(error)))
    }

    /// Reconcile what the daemon before this one left behind — roadmap task **T18**.
    ///
    /// Called once, before anything is served, and it is the only thing in this registry that reads
    /// a `services` row it did not write. A daemon that was killed — or a machine that lost power —
    /// leaves rows claiming a supervisor that no longer exists, and every one of them is one of two
    /// things:
    ///
    /// - **a survivor**: the pid is still there *and* the process bearing it began at the moment
    ///   that was recorded. It is adopted, and from here on it is supervised like anything else. The
    ///   pair is the whole check, because a pid on its own is reused within minutes and signalling
    ///   somebody else's program is the one accident this product cannot have.
    /// - **gone**: nothing has that pid, or what does is a different process. The row is cleared and
    ///   the service is recorded as `stopped` with [`StateReason::Vanished`]. **Nothing is
    ///   signalled**, which is the point of failing the identity check rather than trusting the
    ///   number.
    ///
    /// A survivor is not always adoptable, and the third outcome is the one that stops it: a service
    /// nothing declares any more cannot be supervised at all, and one whose row says `starting`,
    /// `stopping` or `restarting` cannot be resumed from where it was — a readiness that was never
    /// decided cannot be re-decided without the pipes that went with the old daemon. Leaving either
    /// running would leave the port and the data directory held by a process the next start collides
    /// with, so they are stopped, and the reason says which it was.
    ///
    /// A survivor that will not go is the fourth outcome and the only one that leaves work undone:
    /// its row is left exactly as it was found, and it is reported in [`Recovery::refused`] so the
    /// boot does not summarise itself as quiet. The next daemon meets the same row and tries again.
    ///
    /// **A stopped survivor is killed rather than asked**, unlike every other stop in this crate.
    /// A boot is not the moment to spend a `StopBehaviour`'s grace period per service on processes
    /// this daemon has already decided it cannot supervise — the daemon would answer no client until
    /// the last of them had gone. For a database that means recovery on its next start, which is the
    /// same cost T15a's missing stop command carries and is stated in the row rather than hidden.
    ///
    /// Adoption writes **no transition** for the service it takes over: nothing happened to it. Its
    /// row said `running` before this process existed and says `running` still.
    pub(crate) async fn recover(&self) -> Recovery {
        let mut recovery = Recovery::default();

        let records = match services::records(&self.store).await {
            Ok(records) => records,

            // Nothing can be reconciled and nothing is: the daemon carries on with an empty registry
            // rather than refusing to start, because a `services` table that cannot be read is a
            // problem every later request will report for itself.
            Err(error) => {
                tracing::error!(
                    %error,
                    "cannot read what the last daemon left behind; no service will be adopted"
                );

                return recovery;
            }
        };

        // Asked once for the whole reconciliation rather than per row. A source that cannot answer
        // is not a reason to leave survivors running — it only means none of them can be adopted,
        // which is what an empty graph then says for every one of them.
        let declared = match self.graph().await {
            Ok(graph) => Some(graph),

            Err(error) => {
                let reason: &dyn std::fmt::Display = match &error {
                    Undeclarable::Unavailable(why) => why,
                    Undeclarable::Invalid(why) => why,
                };

                tracing::error!(
                    %reason,
                    "cannot tell which services are declared; anything that outlived the last \
                     daemon will be stopped rather than adopted"
                );

                None
            }
        };

        for (stored, record) in records {
            // A row in `stopped` or `failed` with no process named is already telling the truth
            // about a machine with no daemon on it. Everything else is either claiming a supervisor
            // or naming a pid, and both are this function's business.
            if !record.state.is_supervised() && record.pid.is_none() {
                continue;
            }

            let service = match ServiceId::parse(&stored) {
                Ok(service) => service,

                Err(error) => {
                    tracing::error!(
                        service = stored,
                        %error,
                        "the services table holds an id this build cannot read; leaving the row \
                         alone"
                    );

                    continue;
                }
            };

            self.reconcile(&service, &record, declared.as_ref(), &mut recovery)
                .await;
        }

        recovery
    }

    /// What to do about one row the last daemon left behind. See [`Registry::recover`].
    async fn reconcile(
        &self,
        service: &ServiceId,
        record: &services::ServiceRecord,
        declared: Option<&ServiceGraph>,
        recovery: &mut Recovery,
    ) {
        let survivor = match survivor(service, record) {
            Ok(survivor) => survivor,

            // The OS has the answer and would not give it, which is neither "it is there" nor "it is
            // gone". Treated as gone, because the alternative is a row left claiming a supervisor
            // for ever — and because the only thing this branch forgoes is *adopting*: nothing is
            // signalled on a pid whose identity was never confirmed.
            Err(error) => {
                tracing::warn!(
                    service = service.as_str(),
                    error = %error,
                    "cannot tell whether this service's process outlived the daemon that started \
                     it; treating it as gone"
                );

                None
            }
        };

        let Some(adopted) = survivor else {
            if self
                .discard(service, record, None, StateReason::Vanished)
                .await
            {
                recovery.cleared.push(service.clone());
            }

            return;
        };

        let spec = declared.and_then(|graph| graph.spec(service));

        // Only a service that was *up* can be taken over as it is. The mid-flight states have a
        // process and no way to resume what was being done to it: a `starting` service was never
        // proved ready and cannot be re-checked without the pipes that went with the old daemon, and
        // a `stopping` one is halfway through a stop somebody asked for.
        let resumable = matches!(record.state, ServiceState::Running | ServiceState::Degraded);

        let stopped = match (spec, resumable) {
            (Some(spec), true) => {
                self.supervise(spec, &mut lock(&self.running), Some(adopted));
                recovery.adopted.push(service.clone());

                return;
            }

            // **Two different sentences, because they are two different people's problem.** A
            // service nothing declares is one somebody removed and the answer is that its process
            // goes with it; a daemon that could not be told what is declared has stopped a service
            // that may be perfectly well declared, and a row saying "nothing declares this" would
            // send its owner looking for a declaration that is there. The log line `recover` writes
            // says the same thing once for the whole boot; this is what `mix service list` shows
            // for each service afterwards.
            (None, _) => {
                let reason = if declared.is_some() {
                    "nothing declares this service any more, so nothing could supervise the \
                     process it left behind"
                } else {
                    "this daemon could not read which services are declared, so it had nothing to \
                     supervise the process it left behind against"
                };

                self.discard(
                    service,
                    record,
                    Some(adopted),
                    StateReason::Unadopted {
                        reason: reason.to_owned(),
                    },
                )
                .await
            }

            (Some(_), false) => {
                self.discard(
                    service,
                    record,
                    Some(adopted),
                    StateReason::Unadopted {
                        reason: format!(
                            "the daemon supervising it went away while it was {}, which is not a \
                             state another daemon can take over",
                            record.state
                        ),
                    },
                )
                .await
            }
        };

        if stopped {
            recovery.stopped.push(service.clone());
        } else {
            recovery.refused.push(service.clone());
        }
    }

    /// Let go of a service the last daemon left behind: stop whatever survived, clear the pid it
    /// named, and record where that leaves it. `false` if the survivor would not go.
    ///
    /// **In that order, and the order is the whole of the guarantee**: a row is only cleared once the
    /// process it named is no longer running, so a daemon killed in the middle of this leaves the next
    /// one exactly the case it already knows how to handle. The corollary is that a survivor which
    /// will not go leaves its row untouched — still claiming the pid, still in the state it was found
    /// in — rather than being written down as stopped while it holds the port. That is the same rule
    /// [`Runner::stop_adopted`](runner) follows, and it is why this can report a failure at all.
    ///
    /// [`runner`]: runner::Runner
    async fn discard(
        &self,
        service: &ServiceId,
        row: &services::ServiceRecord,
        survivor: Option<Adopted>,
        reason: StateReason,
    ) -> bool {
        if let Some(adopted) = survivor {
            tracing::info!(
                service = service.as_str(),
                pid = adopted.pid(),
                %reason,
                "stopping a process that outlived the daemon supervising it"
            );

            if let Err(error) = adopted.stop() {
                tracing::error!(
                    service = service.as_str(),
                    pid = adopted.pid(),
                    error = %error,
                    "cannot stop it; its row is left naming it, for the next daemon to try again"
                );

                return false;
            }

            if !gone(service, &adopted).await {
                tracing::error!(
                    service = service.as_str(),
                    pid = adopted.pid(),
                    "this process did not go when it was stopped; its row is left naming it, for \
                     the next daemon to try again"
                );

                return false;
            }
        }

        if row.pid.is_some()
            && let Err(error) = services::ended(&self.store, service, None).await
        {
            tracing::error!(
                service = service.as_str(),
                error = %error,
                "cannot clear the process this service's row names; the next daemon will meet the \
                 same pid again"
            );
        }

        if !row.state.is_supervised() {
            // A row that already says `stopped` or `failed` and merely held a stale pid. There is
            // no move to make and nothing to announce: it was where it says it is.
            return true;
        }

        // Through `Stopping`, which is the only edge into `Stopped` — and which the machine already
        // means: this is the last thing anybody did to the process. A row that was *already*
        // stopping is skipped rather than asked to move to where it is, which is not an event.
        if row.state != ServiceState::Stopping {
            record(
                &self.store,
                &self.events,
                service,
                ServiceState::Stopping,
                reason.clone(),
            )
            .await;
        }

        record(
            &self.store,
            &self.events,
            service,
            ServiceState::Stopped,
            reason,
        )
        .await;

        true
    }

    /// Start everything in `plan`, in its order, waiting for each to be ready before the next.
    ///
    /// A service that is **already up** is counted as reached rather than restarted: `mix service
    /// start` on something that is up is a request for it to be up. One that is merely already
    /// *supervised* — in a restart backoff, or mid-start for another walk — is not the same thing: it
    /// is asked to start now and waited for. Both decisions are [`Registry::begin`]'s, because the
    /// first has to be taken under the same lock as the registration. See the note there.
    ///
    /// **Every service in the plan is asked, not only the one the caller named.** A plan is already
    /// the transitive set, this walks it one service at a time, and a `db` in its fourth crash is
    /// exactly what a person typing `mix service start web` needs unstuck — a walk that woke the top
    /// of the plan and left its dependencies sitting out their backoffs would fail, and tell them to
    /// go and start `db` by hand.
    pub(crate) async fn start(&self, graph: &ServiceGraph, plan: &Plan) -> Walk {
        let mut walk = Walk::default();

        for id in plan.flat() {
            let Some(spec) = graph.spec(id) else {
                // Unreachable through the API, where the plan is built from this same graph. Worth
                // a line rather than a panic: it would take the daemon down for one bad request.
                tracing::error!(
                    service = id.as_str(),
                    "the plan names a service the graph does not hold"
                );

                walk.failed = Some((id.clone(), None));
                break;
            };

            match self.begin(spec).await {
                Start::Ready => walk.reached.push(id.clone()),

                Start::Failed(reason) => {
                    walk.failed = Some((id.clone(), reason));
                    break;
                }
            }
        }

        if let Some((failed, _)) = &walk.failed {
            walk.blocked = self.block(graph, plan, failed).await;
        }

        walk
    }

    /// Stop everything in `plan`, in its order, waiting for each to have gone before the next.
    ///
    /// A service that is not running is already where the caller wants it, so nearly every stop
    /// reaches everything it was asked to. **Nearly, and not always — since T18.** A survivor this
    /// daemon adopted and could not kill keeps its row in `stopping` and keeps holding the port, and
    /// a walk that reported it as reached would tell somebody their database is down while it is
    /// answering queries. So the row is what decides, and a service that is still supervised when its
    /// runner has finished stops the walk.
    ///
    /// **Stopping there rather than carrying on is the stop order doing its job.** A plan is
    /// dependents first — `web` before the `db` it talks to — precisely so that nothing is left
    /// pointed at a service that is going away; going on to stop `db` because `web` would not die
    /// would produce exactly the arrangement the order exists to prevent.
    pub(crate) async fn stop(&self, plan: &Plan) -> Walk {
        let mut walk = Walk::default();

        for id in plan.flat() {
            if !self.stop_one(id).await {
                // No reason to carry: what a client would render here is the state the row is still
                // in, which it can already read, and the sentence saying why is the runner's own
                // `error!` in `daemon.log`.
                walk.failed = Some((id.clone(), None));
                break;
            }

            walk.reached.push(id.clone());
        }

        walk
    }

    /// Wait for every supervised service to have stopped.
    ///
    /// **The order is deliberately not this function's.** By the time the daemon calls it the root
    /// token has been cancelled, so every runner is already performing the stop its spec asks for;
    /// what this adds is the *waiting*, so the process does not exit while a database is still
    /// flushing and leave the job to [`Supervised`](mixengine_platform::process::Supervised)'s
    /// destructor, which kills rather than asks.
    ///
    /// Stopping in reverse dependency order is `daemon.shutdown` (T9a), which walks
    /// [`Registry::stop`] *before* anything cancels the root token. A signal cancels everything at
    /// once and there is no order left to impose.
    pub(crate) async fn shut_down(&self) {
        let running: Vec<(ServiceId, Running)> = lock(&self.running).drain().collect();

        if running.is_empty() {
            return;
        }

        tracing::info!(
            services = running.len(),
            "waiting for supervised services to stop"
        );

        for (id, entry) in running {
            entry.cancel.cancel();

            if let Err(error) = entry.task.await {
                tracing::warn!(
                    service = id.as_str(),
                    %error,
                    "the task supervising this service did not finish cleanly"
                );
            }
        }
    }

    /// Which services have a task supervising them right now.
    ///
    /// **Not a second opinion about what a service is doing** — that is the row's, and this registry
    /// never writes one behind `core`'s back. It answers the other question, and since T18 the two
    /// only come apart within one run of the daemon: a row that says `running` with nothing in here
    /// used to be what a killed daemon left behind, and [`Registry::recover`] now reconciles those
    /// before the first client is served. What is left for this to show is a service whose runner
    /// ended without the row following it — which is a fault, and is what `service.list` makes
    /// visible instead of implying.
    pub(crate) fn supervised(&self) -> BTreeSet<ServiceId> {
        lock(&self.running)
            .iter()
            .filter(|(_, entry)| !entry.task.is_finished())
            .map(|(id, _)| id.clone())
            .collect()
    }

    /// Whether a task is supervising this service right now.
    fn is_running(&self, id: &ServiceId) -> bool {
        lock(&self.running)
            .get(id)
            .is_some_and(|entry| !entry.task.is_finished())
    }

    /// Have `spec` supervised if it is not already, and wait until it is decided whether it is up.
    ///
    /// **Already supervised is answered in here, under the lock that registers.** Asking first and
    /// spawning afterwards would be two decisions where there is one: the daemon's runtime is
    /// multi-threaded, so two `service.start` for the same service would both find nothing running
    /// and both spawn, and the second registration would overwrite the first — leaving a process
    /// holding the port and the data directory that no `stop` and no shutdown can still name.
    ///
    /// **What that lock decides is whether to spawn, and nothing about the service.** A runner is
    /// alive through a restart backoff, through a stop and through a start that has not finished, so
    /// the answer comes from the [`Readiness`] it publishes rather than from its task being alive:
    /// `mix service start` on something genuinely up is a request for it to be up and is reached,
    /// while on something in its fourth crash it is not, and the tier below must not be started
    /// against it.
    ///
    /// **A runner that is not up is asked rather than only read** — T19c, and the case a person is
    /// most likely to type `mix service start` at. Reading a service crash-looping under
    /// [`RestartPolicy::Always`](mixengine_proto::RestartPolicy::Always) can only ever report the
    /// attempt that just failed: its runner never deregisters, nothing in this path could shorten the
    /// backoff it is sitting in, and every start therefore re-walked the tier, emitted two more
    /// events and spawned nothing. So the request goes *in*, and what this then waits for is the
    /// attempt that request causes rather than the one before it.
    async fn begin(&self, spec: &ServiceSpec) -> Start {
        let id = spec.id().clone();

        let (mut readiness, asked) = {
            // Held across the spawn as well, so that a runner which ends immediately cannot
            // deregister an entry that has not been made yet. Nothing awaits while it is held.
            let mut running = lock(&self.running);

            let supervised = running
                .get(&id)
                .filter(|entry| !entry.task.is_finished())
                .map(|entry| (entry.readiness.clone(), Arc::clone(&entry.asked_to_start)));

            if let Some((mut readiness, asked_to_start)) = supervised {
                // Read and marked seen in the same breath as the request, which is what makes the
                // wait below sound: whatever the runner publishes after this — including a start that
                // is up again before this function is next polled — is a change this receiver has not
                // seen, so it cannot be missed and the value it replaces cannot be mistaken for it.
                let before = readiness.borrow_and_update().clone();

                match before {
                    // Already where the caller wants it. Nothing is asked for, and nothing must be:
                    // a request left as an unconsumed permit would cut short the backoff of some
                    // crash an hour from now that nobody asked about.
                    Readiness::Up => (readiness, None),

                    _ => {
                        asked_to_start.notify_one();

                        (readiness, Some(before))
                    }
                }
            } else {
                (self.supervise(spec, &mut running, None), None)
            }
        };

        match asked {
            Some(before) => settled_after_asking(&mut readiness, before).await,
            None => settled(&mut readiness).await,
        }
    }

    /// Put a task in charge of this service and register it. The readiness it will publish on.
    ///
    /// **The caller holds the lock**, which is what the `running` argument says rather than
    /// documents: registering has to happen in the same critical section as the decision to spawn,
    /// or two `service.start` for one service would each find nothing running, each spawn, and leave
    /// a process holding the port that no stop can still name.
    ///
    /// `adopted` is the difference between the two ways a runner begins: [`None`] spawns the process
    /// itself, and [`Some`] takes over one that survived the daemon that started it (roadmap task
    /// T18). Everything after the first life of the process is the same code either way, which is
    /// the reason this is one function and not two.
    fn supervise(
        &self,
        spec: &ServiceSpec,
        running: &mut HashMap<ServiceId, Running>,
        adopted: Option<Adopted>,
    ) -> watch::Receiver<Readiness> {
        let id = spec.id().clone();
        let cancel = self.shutdown.child_token();
        let generation = self.generations.fetch_add(1, Ordering::Relaxed);
        let (published, readiness) = watch::channel(Readiness::Deciding);
        let asked_to_start = Arc::new(Notify::new());

        let runner = Runner {
            spec: spec.clone(),
            store: self.store.clone(),
            directory: self.paths.service_logs(&id),
            host: Arc::clone(&self.host),
            events: self.events.clone(),
            cancel: cancel.clone(),
            asked_to_start: Arc::clone(&asked_to_start),
            readiness: published,
        };

        let deregister = Arc::clone(&self.running);
        let named = id.clone();

        let task = tokio::spawn(async move {
            match adopted {
                Some(adopted) => runner.adopt(adopted).await,
                None => runner.run().await,
            }

            let mut running = lock(&deregister);
            if running
                .get(&named)
                .is_some_and(|entry| entry.generation == generation)
            {
                running.remove(&named);
            }
        });

        running.insert(
            id,
            Running {
                cancel,
                asked_to_start,
                task,
                generation,
                readiness: readiness.clone(),
            },
        );

        readiness
    }

    /// Cancel one service, wait for its runner to finish, and say where that left it.
    ///
    /// **The answer comes from the row and not from the task having ended**, which is the whole
    /// reason this returns anything at all. Since T18 a runner can finish with the service still up:
    /// a survivor this daemon adopted and could not kill leaves its row in `stopping` on purpose —
    /// see [`Runner::stop_adopted`](runner) — because writing `stopped` for a process that is still
    /// holding the port is the one lie crash recovery exists to prevent. The task ends either way,
    /// so a caller that read only that would report the stop it did not get.
    ///
    /// A row nobody can read is not evidence the service is still running, and neither is one that
    /// is not there: both are answered `true`, because the stop itself was performed and the failure
    /// is the daemon's own — it is in `daemon.log`, and it is not a state a client could render.
    ///
    /// [`runner`]: runner::Runner
    async fn stop_one(&self, id: &ServiceId) -> bool {
        // Bound in a statement of its own so the guard is dropped before the await below: an
        // `if let` would hold it across, and a lock held over an await is one no other thread can
        // take for as long as this stop lasts.
        let entry = lock(&self.running).remove(id);

        if let Some(entry) = entry {
            entry.cancel.cancel();

            if let Err(error) = entry.task.await {
                tracing::warn!(
                    service = id.as_str(),
                    %error,
                    "the task supervising this service did not finish cleanly"
                );
            }
        }

        // Asked after the task, never before: a runner writes `Stopped` and *then* returns, so this
        // reads what the stop actually persisted rather than what it was about to.
        match services::record(&self.store, id).await {
            Ok(record) => !record.state.is_supervised(),

            Err(error) => {
                tracing::error!(
                    service = id.as_str(),
                    %error,
                    "cannot read where stopping this service left it; reporting it as stopped"
                );

                true
            }
        }
    }

    /// Mark everything that can no longer come up, and say which edge broke.
    ///
    /// The dependency named is the **direct** one each service declares rather than the root of the
    /// chain, which is why this accumulates as it walks: a chain of four then reads as four honest
    /// sentences leading to the one service that actually broke, instead of three copies of a name
    /// none of them mention.
    ///
    /// Through `Starting`, because that is the only edge the machine has into `Failed` from where
    /// these services are — and it is true: they were asked to start, and this is how that ended.
    async fn block(&self, graph: &ServiceGraph, plan: &Plan, failed: &ServiceId) -> Vec<ServiceId> {
        let Ok(blocked) = graph.blocked_by(failed) else {
            return Vec::new();
        };

        let mut hopeless: BTreeSet<ServiceId> = std::iter::once(failed.clone()).collect();
        let mut marked = Vec::new();

        for id in plan.flat() {
            if !blocked.contains(id) || self.is_running(id) {
                continue;
            }

            let Ok(dependencies) = graph.dependencies_of(id) else {
                continue;
            };

            let Some(dependency) = dependencies
                .iter()
                .find(|dependency| hopeless.contains(*dependency))
            else {
                continue;
            };

            let reason = StateReason::DependencyFailed {
                dependency: dependency.clone(),
            };

            // Both writes are attempted, and the second is not conditional on the first: if the
            // row is somewhere `Starting` cannot be reached from, `Failed` may still be reachable
            // from where it actually is, and the alternative is a row left claiming a start that
            // nothing is performing.
            let entered = record(
                &self.store,
                &self.events,
                id,
                ServiceState::Starting,
                StateReason::Requested,
            )
            .await;

            if record(&self.store, &self.events, id, ServiceState::Failed, reason).await {
                marked.push(id.clone());
            } else if entered {
                tracing::error!(
                    service = id.as_str(),
                    "this service was recorded as starting and then could not be recorded as \
                     failed; its row now names a start nothing is performing"
                );
            }

            hopeless.insert(id.clone());
        }

        marked
    }
}

/// The process this row names, if it is still the one the row was written about.
///
/// **Both halves or nothing.** A row with a pid and no start time is one this build wrote when the
/// OS would not say when the process began, and there is no way to tell now whether the number still
/// means what it meant — so it is treated as gone, which costs an adoption and never a wrong signal.
///
/// # Errors
///
/// Whatever [`Adopted::identify`] could not ask the OS. Not "there is no such process", which is
/// [`None`].
fn survivor(
    service: &ServiceId,
    row: &services::ServiceRecord,
) -> mixengine_platform::Result<Option<Adopted>> {
    let (Some(pid), Some(started)) = (row.pid, row.pid_start_time) else {
        if row.pid.is_some() {
            tracing::warn!(
                service = service.as_str(),
                "this service's row names a process and not when it began, so it cannot be \
                 identified; treating it as gone"
            );
        }

        return Ok(None);
    };

    Adopted::identify(pid, StartTime::from_stored(started))
}

/// Persist a state change and publish the value that was persisted. `false` if it did not land.
///
/// The one place a `services.state` write happens in this crate, which is what keeps the row and the
/// event from ever describing different events: what is published is the [`ServiceTransition`] the
/// transaction handed back, not a second description built beside it.
///
/// [`ServiceTransition`]: mixengine_proto::ServiceTransition
async fn record(
    store: &Store,
    events: &Events,
    service: &ServiceId,
    to: ServiceState,
    reason: StateReason,
) -> bool {
    match services::transition(store, service, to, reason, now()).await {
        Ok(change) => {
            events.publish(DaemonEvent::ServiceStateChanged(change));

            true
        }

        Err(error) => {
            tracing::error!(
                service = service.as_str(),
                to = %to,
                %error,
                "cannot record a state change for this service"
            );

            false
        }
    }
}

/// Wait until a runner has decided whether its service is up, and say so in the walk's terms.
///
/// **Bounded by the restart policy where the policy is bounded, and by one attempt where it is
/// not.** A policy with a ceiling arrives at `Running` or at `Failed` by itself, and this waits
/// through its backoffs for whichever it reached — a first crash under the default `OnFailure` is a
/// transient, not an answer. Only a
/// [`RestartPolicy::Always`](mixengine_proto::RestartPolicy::Always) never reaches `Failed` at all,
/// and there the outcome of the attempt in flight is the only thing there is to wait for. Which of
/// the two applies is [`Readiness::of`](runner::Readiness::of)'s to decide; a service being put back
/// by its policy is meanwhile reported by its events, where a client can see it being tried again.
///
/// A closed channel is a runner that ended without deciding: a task that panicked, or one whose
/// first `Starting` would not persist. Both are in `daemon.log` already, and neither is a state a
/// client could render — which is what [`Start::Failed`]'s [`None`] says.
async fn settled(readiness: &mut watch::Receiver<Readiness>) -> Start {
    loop {
        // Taken by value rather than matched in place, so no borrow of the channel is held across the
        // await below. Marking it seen here is also what keeps `changed` from missing the next one.
        let decided = readiness.borrow_and_update().clone();

        if let Some(start) = decided_by(decided) {
            return start;
        }

        if readiness.changed().await.is_err() {
            return Start::Failed(None);
        }
    }
}

/// The same, for a runner that has just been **asked** to start — T19c.
///
/// What such a runner is publishing at the moment it is asked is the attempt *before* the request:
/// `Down` with the crash the backoff is being served for. Waiting on that would answer the caller
/// with the failure their own request is in the middle of correcting, which is the bug this task
/// exists for, so the first thing waited for here is the next thing the runner says. It will say
/// something: a runner released by a request moves to `Starting`, and one that cannot persist even
/// that ends and closes the channel.
///
/// **Unless it was already ending**, which is the one race worth spending a branch on: a runner in
/// its `Stopping`, or three statements from returning `Failed`, has no backoff left to be released
/// from and the request is simply dropped with it. Then `before` — what it last managed to say — is
/// still the truth about the service, and reporting it keeps the reason a client can render instead
/// of trading it for the [`None`] that means "the daemon's own problem".
async fn settled_after_asking(
    readiness: &mut watch::Receiver<Readiness>,
    before: Readiness,
) -> Start {
    if readiness.changed().await.is_err() {
        return decided_by(before).unwrap_or(Start::Failed(None));
    }

    settled(readiness).await
}

/// What a readiness answers a walk, or [`None`] while it answers nothing yet.
fn decided_by(readiness: Readiness) -> Option<Start> {
    match readiness {
        Readiness::Up => Some(Start::Ready),
        Readiness::Down(reason) => Some(Start::Failed(reason)),
        Readiness::Deciding => None,
    }
}

/// The daemon's clock, in the one shape everything below it takes.
fn now() -> Timestamp {
    Timestamp::from_system_time(SystemTime::now())
}

/// The running map, whether or not a task holding it panicked.
///
/// A poisoned lock here means a runner task died mid-tidy-up; the map itself is a `HashMap` of
/// handles and is no less valid for it, and refusing to supervise anything else would turn one
/// failed service into a daemon that cannot start another.
fn lock<T>(mutex: &Mutex<T>) -> MutexGuard<'_, T> {
    mutex.lock().unwrap_or_else(PoisonError::into_inner)
}

#[cfg(test)]
mod tests {
    use std::time::Duration;

    use mixengine_proto::{Backoff, Millis, ReadyCheck, RestartPolicy};
    use mixengine_testkit::FakeService;

    use super::fixture::{Declared, EVENTUALLY, Unavailable, arguments, home, service, spec};
    use super::*;

    /// How long a test listens to prove that nothing happened.
    ///
    /// Only ever meaningful against something far longer: what it is weighed against is a thirty
    /// second backoff, and what would break the silence would break it within a millisecond.
    const SILENCE: Duration = Duration::from_secs(2);

    /// A service that dies before it is ever ready, under a policy that never gives up.
    ///
    /// The backoff is far longer than either test that uses this needs, deliberately: what both
    /// assert about is the gap between two attempts, and a short wait would race them into asserting
    /// against a service that had moved on to the next one.
    fn crash_looping(id: &str) -> ServiceSpec {
        let broken = FakeService::new().never_ready().exit_after(50).exit_code(3);

        spec(id)
            .args(arguments(&broken))
            .restart(RestartPolicy::Always {
                backoff: Backoff {
                    initial: Millis::from_secs(30),
                    max: Millis::from_secs(30),
                    ..Backoff::default()
                },
            })
            .build()
            .expect("a usable spec")
    }

    /// A service that comes up, stays up for a moment and then dies, under the same never-give-up
    /// policy and the same long backoff.
    ///
    /// **The slow ready is the point.** Between the registration and `Running` lies the one window in
    /// which [`Registry::begin`] can ask a runner that is not in a backoff, and what a test needs to
    /// be able to do is land a second walk inside it. A second is far longer than the two database
    /// writes a walk takes to get there, and the exit is late enough to be unambiguously after it.
    fn crashes_after_coming_up(id: &str) -> ServiceSpec {
        let brittle = FakeService::new()
            .ready_after(1_000)
            .exit_after(3_000)
            .exit_code(3);

        spec(id)
            .args(arguments(&brittle))
            .restart(RestartPolicy::Always {
                backoff: Backoff {
                    initial: Millis::from_secs(30),
                    max: Millis::from_secs(30),
                    ..Backoff::default()
                },
            })
            .build()
            .expect("a usable spec")
    }

    fn registry(paths: &Paths, store: &Store, specs: Arc<dyn SpecSource>) -> Registry {
        Registry::new(
            paths,
            store,
            Arc::new(mixengine_platform::mock::Host::with_home(paths.root())),
            Events::new(),
            specs,
            CancellationToken::new(),
        )
    }

    /// A `fakeservice` running, and a row that says the *last* daemon started it — T18's subject.
    ///
    /// The row is written the way the runner writes one, through `core`, so what recovery meets is
    /// what a killed daemon really leaves: a supervised state, a pid, and the moment that process
    /// began. Nothing here is supervising it, which is the point.
    async fn left_running(
        store: &Store,
        id: &ServiceId,
        state: ServiceState,
    ) -> mixengine_testkit::service::Running {
        left_running_with(store, id, state, FakeService::new()).await
    }

    /// [`left_running`], for a test that needs the survivor to behave in a particular way.
    async fn left_running_with(
        store: &Store,
        id: &ServiceId,
        state: ServiceState,
        fake: FakeService,
    ) -> mixengine_testkit::service::Running {
        let service = fake.spawn();

        // Waited for, because a spawn returns before the process has parsed its arguments — and a
        // start time read in that window would be right for the wrong reason.
        assert!(
            service.wait_for_stdout(mixengine_testkit::service::READY_LINE, EVENTUALLY),
            "the survivor did not start"
        );

        let pid = service.id();
        let started = mixengine_platform::process::started_at(pid)
            .expect("this system can be asked when a process began")
            .expect("the survivor is running");

        services::transition(
            store,
            id,
            ServiceState::Starting,
            StateReason::Requested,
            now(),
        )
        .await
        .expect("a stopped service can start");

        if state != ServiceState::Starting {
            services::transition(store, id, state, StateReason::Ready, now())
                .await
                .expect("a starting service can reach the state this fixture asks for");
        }

        services::started(store, id, pid, Some(started.stored()), now())
            .await
            .expect("the row takes the process the last daemon started");

        service
    }

    /// Wait for a survivor to have gone. `false` if it had not within [`EVENTUALLY`].
    ///
    /// Polled rather than asked once: recovery stops a process it cannot supervise and does not wait
    /// for the kernel to get round to it, which is the same thing every other stop in this crate is
    /// polled for.
    async fn gone(service: &mut mixengine_testkit::service::Running) -> bool {
        let deadline = tokio::time::Instant::now() + EVENTUALLY;

        while service.still_running() {
            if tokio::time::Instant::now() >= deadline {
                return false;
            }

            tokio::time::sleep(Duration::from_millis(25)).await;
        }

        true
    }

    /// The state, pid and last exit code of one row.
    async fn row(store: &Store, id: &ServiceId) -> (ServiceState, Option<i64>) {
        let state = services::state(store, id).await.expect("the row");
        let pid: Option<i64> = sqlx::query_scalar("SELECT pid FROM services WHERE id = ?")
            .bind(id.as_str())
            .fetch_one(store.pool())
            .await
            .expect("the row");

        (state, pid)
    }

    #[tokio::test]
    async fn a_service_starts_runs_and_stops() {
        let (_home, paths, store) = home(&["caddy"]).await;
        let declared = Declared(vec![spec("caddy").build().expect("a usable spec")]);
        let registry = registry(&paths, &store, Arc::new(declared));

        let graph = registry.graph().await.expect("one declared service");
        let plan = graph.start_plan([&service("caddy")]).expect("a plan");

        let walk = registry.start(&graph, &plan).await;

        assert_eq!(walk.reached, vec![service("caddy")], "{walk:?}");
        assert!(walk.failed.is_none(), "{walk:?}");

        let (state, pid) = row(&store, &service("caddy")).await;
        assert_eq!(state, ServiceState::Running);
        assert!(pid.is_some(), "the pid of a running service is recorded");

        // The log file is the supervisor's, and this is the one assertion that it is being written
        // for a service the *registry* started rather than one a supervisor test spawned.
        let log = paths
            .service_logs(&service("caddy"))
            .join(mixengine_supervisor::logs::CURRENT_LOG_FILE_NAME);
        assert!(log.is_file(), "{} was not written", log.display());

        let stopping = graph.stop_plan([&service("caddy")]).expect("a plan");
        let stopped = registry.stop(&stopping).await;

        assert_eq!(stopped.reached, vec![service("caddy")], "{stopped:?}");
        assert!(stopped.failed.is_none(), "{stopped:?}");

        let (state, pid) = row(&store, &service("caddy")).await;
        assert_eq!(state, ServiceState::Stopped);
        assert_eq!(pid, None, "a stopped service names no process");
    }

    /// **A stop that did not take the service down does not report that it did**, which since T18
    /// is a thing that can happen: a survivor this daemon adopted and could not kill keeps its row
    /// in a supervised state on purpose — `Runner::stop_adopted` — because writing `stopped` for a
    /// process still holding the port is the lie crash recovery exists to prevent. The runner's task
    /// ends either way, so a walk that read *that* would report the stop nobody got.
    ///
    /// Arranged through the row rather than through a process that refuses to die, which is not
    /// something a test can stage on three operating systems. It is the row `stop_one` answers from,
    /// and a supervised one with nothing running it is exactly what a refused stop leaves behind —
    /// as well as being what the *second* `mix service stop` after a refused first one meets.
    #[tokio::test]
    async fn a_stop_that_left_the_service_supervised_is_not_reported_as_reached() {
        let (_home, paths, store) = home(&["caddy"]).await;
        let declared = Declared(vec![spec("caddy").build().expect("a usable spec")]);
        let registry = registry(&paths, &store, Arc::new(declared));

        // Through the machine rather than into the column, so that what this leaves is a row the
        // daemon could really have written.
        for state in [
            ServiceState::Starting,
            ServiceState::Running,
            ServiceState::Stopping,
        ] {
            services::transition(
                &store,
                &service("caddy"),
                state,
                StateReason::Requested,
                now(),
            )
            .await
            .expect("a row can be moved to where a refused stop leaves it");
        }

        let graph = registry.graph().await.expect("one declared service");
        let plan = graph.stop_plan([&service("caddy")]).expect("a plan");

        let walk = registry.stop(&plan).await;

        assert!(
            walk.reached.is_empty(),
            "a service still claiming a supervisor has not been stopped: {walk:?}"
        );
        assert!(
            matches!(&walk.failed, Some((id, None)) if id == &service("caddy")),
            "the walk names the service it could not take down, with nothing to quote as a \
             reason: {walk:?}"
        );
    }

    #[tokio::test]
    async fn a_service_that_never_becomes_ready_fails_rather_than_waiting_forever() {
        let (_home, paths, store) = home(&["slow"]).await;

        let fake = FakeService::new().never_ready();
        let declared = Declared(vec![
            spec("slow")
                .args(arguments(&fake))
                .ready(ReadyCheck::LogPattern {
                    regex: mixengine_testkit::service::READY_LINE.to_owned(),
                    timeout: Millis(750),
                })
                .build()
                .expect("a usable spec"),
        ]);
        let registry = registry(&paths, &store, Arc::new(declared));

        let graph = registry.graph().await.expect("one declared service");
        let plan = graph.start_plan([&service("slow")]).expect("a plan");

        let walk = registry.start(&graph, &plan).await;

        assert!(
            matches!(
                &walk.failed,
                Some((id, Some(StateReason::ReadyTimeout { after })))
                    if id == &service("slow") && *after == Millis(750)
            ),
            "a ready timeout says how long it waited: {walk:?}"
        );

        let (state, pid) = row(&store, &service("slow")).await;
        assert_eq!(state, ServiceState::Failed);
        assert_eq!(
            pid, None,
            "a service that was killed for never becoming ready names no process"
        );
    }

    /// The invariant behind the check-and-register being one step: whatever else two starts do,
    /// there is never a second process for a service that already has one.
    #[tokio::test]
    async fn starting_a_service_that_is_already_up_does_not_spawn_a_second_one() {
        let (_home, paths, store) = home(&["caddy"]).await;
        let declared = Declared(vec![spec("caddy").build().expect("a usable spec")]);
        let registry = registry(&paths, &store, Arc::new(declared));

        let graph = registry.graph().await.expect("one declared service");
        let plan = graph.start_plan([&service("caddy")]).expect("a plan");

        registry.start(&graph, &plan).await;
        let (_, first) = row(&store, &service("caddy")).await;
        assert!(first.is_some(), "the first start recorded a process");

        let again = registry.start(&graph, &plan).await;

        assert_eq!(again.reached, vec![service("caddy")], "{again:?}");
        assert!(again.failed.is_none(), "{again:?}");
        assert_eq!(
            row(&store, &service("caddy")).await.1,
            first,
            "the process being supervised is still the one the first start spawned"
        );
        assert_eq!(
            lock(&registry.running).len(),
            1,
            "a second runner would be one the registry can no longer name"
        );

        let stopping = graph.stop_plan([&service("caddy")]).expect("a plan");
        registry.stop(&stopping).await;
    }

    /// A stop that arrives mid-start must not be held by the ready check it interrupts.
    #[tokio::test]
    async fn a_stop_during_a_ready_check_does_not_wait_the_check_out() {
        let (_home, paths, store) = home(&["slow"]).await;

        let fake = FakeService::new().never_ready();
        let declared = Declared(vec![
            spec("slow")
                .args(arguments(&fake))
                .ready(ReadyCheck::LogPattern {
                    regex: mixengine_testkit::service::READY_LINE.to_owned(),
                    // Far longer than this test may take, on purpose: a runner that only looked at
                    // its token after `ready::wait` returned would be reported by the timeout below
                    // rather than by an assertion that has to guess at a threshold.
                    timeout: Millis::from_secs(600),
                })
                .build()
                .expect("a usable spec"),
        ]);
        let registry = Arc::new(registry(&paths, &store, Arc::new(declared)));

        let graph = registry.graph().await.expect("one declared service");
        let plan = graph.start_plan([&service("slow")]).expect("a plan");

        let walking = {
            let registry = Arc::clone(&registry);
            tokio::spawn(async move { registry.start(&graph, &plan).await })
        };

        // A recorded pid is the runner having spawned the process and entered the ready check,
        // which is the only moment this test is about.
        let deadline = tokio::time::Instant::now() + EVENTUALLY;
        while row(&store, &service("slow")).await.1.is_none() {
            assert!(
                tokio::time::Instant::now() < deadline,
                "the service never spawned"
            );

            tokio::time::sleep(Duration::from_millis(25)).await;
        }

        registry.shutdown.cancel();
        tokio::time::timeout(EVENTUALLY, registry.shut_down())
            .await
            .expect("the stop was not held by the ready check it interrupted");

        let walk = tokio::time::timeout(EVENTUALLY, walking)
            .await
            .expect("the walk was told how the start ended")
            .expect("the walk did not panic");

        assert!(
            matches!(
                &walk.failed,
                Some((id, Some(StateReason::Requested))) if id == &service("slow")
            ),
            "a service stopped before it was ready did not come up, and says why: {walk:?}"
        );

        let (state, pid) = row(&store, &service("slow")).await;
        assert_eq!(state, ServiceState::Stopped);
        assert_eq!(pid, None);
    }

    #[tokio::test]
    async fn what_depends_on_a_failure_is_never_spawned() {
        let (_home, paths, store) = home(&["db", "web"]).await;

        // Dies without ever announcing itself. **`never_ready` is not decoration**: a service that
        // printed its ready line and then died would be racing the ready check against the exit,
        // which is a real behaviour `ready::wait` biases towards the exit for — but it is not what
        // this test is about, and a test whose subject is decided by a race is a test that passes
        // most of the time.
        let broken = FakeService::new().never_ready().exit_after(50).exit_code(3);

        let declared = Declared(vec![
            spec("db")
                .args(arguments(&broken))
                .build()
                .expect("a usable spec"),
            spec("web")
                .depends_on(service("db"))
                .build()
                .expect("a usable spec"),
        ]);
        let registry = registry(&paths, &store, Arc::new(declared));

        let graph = registry.graph().await.expect("two declared services");
        let plan = graph.start_plan([&service("web")]).expect("a plan");

        let walk = registry.start(&graph, &plan).await;

        assert!(
            matches!(&walk.failed, Some((id, _)) if id == &service("db")),
            "the walk stops at the service that failed: {walk:?}"
        );
        assert_eq!(walk.blocked, vec![service("web")], "{walk:?}");

        assert_eq!(row(&store, &service("db")).await.0, ServiceState::Failed);
        assert_eq!(row(&store, &service("web")).await.0, ServiceState::Failed);
    }

    /// A walk waits for the attempt in flight and **not** for the policy to give up, because a policy
    /// is allowed never to: `RestartPolicy::Always` has no ceiling, so nothing about a service under
    /// it will ever reach `Failed`, and a walk that waited for that would never come back at all.
    #[tokio::test]
    async fn a_service_whose_policy_never_gives_up_does_not_hold_the_walk_for_ever() {
        let (_home, paths, store) = home(&["db"]).await;
        let registry = registry(
            &paths,
            &store,
            Arc::new(Declared(vec![crash_looping("db")])),
        );

        let graph = registry.graph().await.expect("one declared service");
        let plan = graph.start_plan([&service("db")]).expect("a plan");

        let walk = tokio::time::timeout(EVENTUALLY, registry.start(&graph, &plan))
            .await
            .expect("the walk was answered after the first attempt");

        assert!(
            matches!(
                &walk.failed,
                Some((id, Some(StateReason::Exited { code: Some(3) })))
                    if id == &service("db")
            ),
            "the walk says how the attempt ended rather than that the policy ran out: {walk:?}"
        );
        assert_eq!(
            row(&store, &service("db")).await.0,
            ServiceState::Restarting,
            "the runner is still putting it back, which is what its policy asks for"
        );

        registry.shutdown.cancel();
        tokio::time::timeout(EVENTUALLY, registry.shut_down())
            .await
            .expect("every runner finished");
    }

    /// The other half of that rule, and the half the default policy lives on: a ceiling is something
    /// a walk *can* wait for, so it does. `OnFailure`'s first crash is a transient the runner
    /// recovers from a backoff later, and a walk that took it for an answer would leave the tier
    /// below `Failed` and unsupervised beside a service that went on to come up by itself.
    #[tokio::test]
    async fn a_walk_waits_out_the_retries_a_bounded_policy_is_allowed() {
        let (_home, paths, store) = home(&["db", "web"]).await;
        let broken = FakeService::new().never_ready().exit_after(50).exit_code(3);

        let declared = Declared(vec![
            spec("db")
                .args(arguments(&broken))
                // One retry, and a backoff short enough that what the test spends its time on is
                // the wait being *taken* rather than the wait itself.
                .restart(RestartPolicy::OnFailure {
                    max_retries: 1,
                    window: Millis::from_secs(300),
                    backoff: Backoff {
                        initial: Millis(50),
                        max: Millis(50),
                        ..Backoff::default()
                    },
                })
                .build()
                .expect("a usable spec"),
            spec("web")
                .depends_on(service("db"))
                .build()
                .expect("a usable spec"),
        ]);
        let registry = registry(&paths, &store, Arc::new(declared));

        let graph = registry.graph().await.expect("two declared services");
        let plan = graph.start_plan([&service("web")]).expect("a plan");

        let walk = tokio::time::timeout(EVENTUALLY, registry.start(&graph, &plan))
            .await
            .expect("the walk was answered once the policy ran out");

        // Two attempts and the crash loop that ended them, not the first `Exited`: that reason here
        // would be a walk that gave up while the runner was still going to try again.
        assert!(
            matches!(
                &walk.failed,
                Some((id, Some(StateReason::CrashLoop { attempts: 2, .. })))
                    if id == &service("db")
            ),
            "the walk is answered by the policy running out, not by one crash: {walk:?}"
        );
        assert_eq!(walk.blocked, vec![service("web")], "{walk:?}");
        assert_eq!(row(&store, &service("db")).await.0, ServiceState::Failed);
        assert_eq!(
            row(&store, &service("web")).await.1,
            None,
            "`web` was never spawned"
        );
    }

    /// **The invariant behind reading readiness rather than a task's liveness.** A runner stays alive
    /// for as long as it keeps putting a service back, and a service in the gap between two attempts
    /// is not somewhere a dependent can be started against.
    #[tokio::test]
    async fn a_service_in_a_restart_backoff_is_not_reported_as_up() {
        let (_home, paths, store) = home(&["db", "web"]).await;
        let declared = Declared(vec![
            crash_looping("db"),
            spec("web")
                .depends_on(service("db"))
                .build()
                .expect("a usable spec"),
        ]);
        let registry = registry(&paths, &store, Arc::new(declared));

        let graph = registry.graph().await.expect("two declared services");
        let plan = graph.start_plan([&service("web")]).expect("a plan");

        let first = tokio::time::timeout(EVENTUALLY, registry.start(&graph, &plan))
            .await
            .expect("the first walk was answered");

        assert_eq!(first.blocked, vec![service("web")], "{first:?}");

        // What the second walk arrives to: a runner that is alive, and a service that is not up.
        assert_eq!(
            row(&store, &service("db")).await.0,
            ServiceState::Restarting
        );

        let again = tokio::time::timeout(EVENTUALLY, registry.start(&graph, &plan))
            .await
            .expect("the second walk was answered");

        assert!(
            matches!(&again.failed, Some((id, _)) if id == &service("db")),
            "a supervised service that is not up has to stop the walk: {again:?}"
        );
        assert!(
            again.reached.is_empty(),
            "nothing in this plan is up, so nothing was reached: {again:?}"
        );
        assert_eq!(
            lock(&registry.running).len(),
            1,
            "the second walk spawned a second runner for a service that already has one"
        );
        assert_eq!(
            row(&store, &service("web")).await.1,
            None,
            "`web` was never spawned against a database that is between crashes"
        );

        registry.shutdown.cancel();
        tokio::time::timeout(EVENTUALLY, registry.shut_down())
            .await
            .expect("every runner finished");
    }

    /// **The other half of that rule, and T19c.** Reading a crash-looping runner is right about the
    /// service not being up and useless as an answer to somebody who has just *asked* for it to
    /// start: nothing in that path could shorten the backoff the runner is sitting in, so every
    /// attempt re-walked the tier, emitted two more events and spawned nothing. A start now reaches
    /// the runner.
    #[tokio::test]
    async fn an_explicit_start_cuts_short_the_backoff_a_crash_loop_is_sitting_in() {
        let (_home, paths, store) = home(&["db"]).await;
        let registry = registry(
            &paths,
            &store,
            Arc::new(Declared(vec![crash_looping("db")])),
        );

        let graph = registry.graph().await.expect("one declared service");
        let plan = graph.start_plan([&service("db")]).expect("a plan");

        let first = tokio::time::timeout(EVENTUALLY, registry.start(&graph, &plan))
            .await
            .expect("the first walk was answered");

        assert!(
            matches!(&first.failed, Some((id, _)) if id == &service("db")),
            "{first:?}"
        );
        assert_eq!(
            row(&store, &service("db")).await.0,
            ServiceState::Restarting,
            "the runner is in the backoff this test is about"
        );

        // Subscribed while that backoff is being served, so the only events on this stream are the
        // ones the second walk causes. Nothing else is running and a 30 second wait is silent.
        let mut watching = registry.events.subscribe();

        // **The timeout is the assertion.** `crash_looping`'s backoff is longer than `EVENTUALLY`, and
        // this walk is not answered until the attempt the request causes has been decided — so a
        // request that did not reach the runner could not be answered here at all.
        let again = tokio::time::timeout(EVENTUALLY, registry.start(&graph, &plan))
            .await
            .expect("the request cut the backoff short rather than waiting it out");

        assert!(
            matches!(
                &again.failed,
                Some((id, Some(StateReason::Exited { code: Some(3) })))
                    if id == &service("db")
            ),
            "the walk is answered by the attempt the request caused: {again:?}"
        );
        assert_eq!(
            lock(&registry.running).len(),
            1,
            "the service was asked to start again, not given a second runner"
        );

        // And it went back as a *request* and not as the policy coming round, which is the difference
        // `Restarts::recovered` records and the reason somebody reading the log needs.
        let frame = tokio::time::timeout(EVENTUALLY, watching.next())
            .await
            .expect("the stream is not silent")
            .expect("the stream is still open");

        let crate::api::events::Frame::Event(DaemonEvent::ServiceStateChanged(change)) = frame
        else {
            panic!("the first thing the second walk published was not a state change: {frame:?}");
        };

        assert_eq!(change.to, ServiceState::Starting);
        assert_eq!(change.reason, StateReason::Requested);

        registry.shutdown.cancel();
        tokio::time::timeout(EVENTUALLY, registry.shut_down())
            .await
            .expect("every runner finished");
    }

    /// **The limit of that, and the half a request must not outlive.** A runner is only listening for
    /// one while it is sitting out a backoff, so a request that arrives mid-start is *kept* — and if
    /// that start succeeds, nothing consumes it for as long as the service stays up. The crash after
    /// that would then be released the instant it entered its backoff, its ladder reset and its move
    /// published as `Requested`, on behalf of somebody who asked an hour earlier and got what they
    /// asked for. The start that answers a request is what takes it.
    #[tokio::test]
    async fn a_start_asked_for_mid_start_is_taken_by_the_start_that_answers_it() {
        let (_home, paths, store) = home(&["db"]).await;
        let registry = registry(
            &paths,
            &store,
            Arc::new(Declared(vec![crashes_after_coming_up("db")])),
        );

        let graph = registry.graph().await.expect("one declared service");
        let plan = graph.start_plan([&service("db")]).expect("a plan");

        // The two walks a shared dependency produces, and the only way to reach the window this
        // test is about: one of them registers the runner, the other finds it `Deciding` a second
        // short of ready and asks it to start — which is a request no `wait_out` is going to take.
        let (first, second) = tokio::time::timeout(EVENTUALLY, async {
            tokio::join!(registry.start(&graph, &plan), registry.start(&graph, &plan))
        })
        .await
        .expect("both walks were answered");

        assert!(first.failed.is_none(), "{first:?}");
        assert!(second.failed.is_none(), "{second:?}");
        assert_eq!(
            lock(&registry.running).len(),
            1,
            "the second walk asked the first walk's runner rather than spawning another"
        );

        // Subscribed once the service is up, so the only events on this stream are the ones its
        // crash causes — and the request, if it survived, is the only thing that could cause more.
        let mut watching = registry.events.subscribe();

        loop {
            let frame = tokio::time::timeout(EVENTUALLY, watching.next())
                .await
                .expect("the service crashed as the fixture says it does")
                .expect("the stream is still open");

            let crate::api::events::Frame::Event(DaemonEvent::ServiceStateChanged(change)) = frame
            else {
                continue;
            };

            if change.to == ServiceState::Restarting {
                assert_eq!(
                    change.reason,
                    StateReason::Exited { code: Some(3) },
                    "the crash is the fixture's, and nobody asked for it"
                );

                break;
            }
        }

        // **The silence is the assertion.** The backoff this runner has just entered is thirty
        // seconds and nothing has asked for anything since the start that succeeded; a request left
        // over from that start would end it here, within a millisecond, as a `Starting` reading
        // `Requested`.
        let next = tokio::time::timeout(SILENCE, watching.next()).await;

        assert!(
            next.is_err(),
            "the runner left a backoff nobody asked it to leave: {next:?}"
        );

        registry.shutdown.cancel();
        tokio::time::timeout(EVENTUALLY, registry.shut_down())
            .await
            .expect("every runner finished");
    }

    /// **T18, and the case M1 is about.** A process that outlived the daemon which started it is
    /// supervised again by the one that finds it — not restarted, not left running with nothing
    /// watching it, and above all not reported as something it is not.
    ///
    /// The survivor here is this test's own child rather than one a daemon left behind, which is the
    /// only way to produce one on Windows at all: a supervised child there dies with its daemon by
    /// kernel guarantee (ADR 0007), so the process that reaches a real `recover` is the one from the
    /// window that ADR accepts. Nothing in the code under test can tell the difference — it is
    /// handed a row, and it asks the OS about the pid in it.
    #[tokio::test]
    async fn a_service_that_outlived_the_last_daemon_is_supervised_again() {
        let (_home, paths, store) = home(&["caddy"]).await;
        let declared = Declared(vec![spec("caddy").build().expect("a usable spec")]);
        let registry = registry(&paths, &store, Arc::new(declared));

        let mut survivor = left_running(&store, &service("caddy"), ServiceState::Running).await;

        // Subscribed before the reconciliation, because half of what is asserted here is a silence:
        // nothing happened to this service, so nothing may be announced about it.
        let mut watching = registry.events.subscribe();

        let recovered = registry.recover().await;

        assert_eq!(recovered.adopted, vec![service("caddy")], "{recovered:?}");
        assert!(recovered.stopped.is_empty(), "{recovered:?}");
        assert!(recovered.cleared.is_empty(), "{recovered:?}");

        assert!(
            registry.supervised().contains(&service("caddy")),
            "the adopted service has nothing supervising it"
        );
        assert_eq!(
            row(&store, &service("caddy")).await.0,
            ServiceState::Running,
            "an adopted service is where it was; adoption is not a start"
        );
        assert!(
            survivor.still_running(),
            "the process was stopped by the daemon that was supposed to take it over"
        );

        let announced = tokio::time::timeout(SILENCE, watching.next()).await;
        assert!(
            announced.is_err(),
            "adopting a service announced a state change, so a client was told a service somebody \
             has been using all along had just moved: {announced:?}"
        );

        // And the other half of being supervised again: a stop reaches it. This is the assertion
        // that separates adoption from a registry entry that merely looks right.
        let graph = registry.graph().await.expect("one declared service");
        let stopping = graph.stop_plan([&service("caddy")]).expect("a plan");
        tokio::time::timeout(EVENTUALLY, registry.stop(&stopping))
            .await
            .expect("the adopted service was stopped");

        let (state, pid) = row(&store, &service("caddy")).await;
        assert_eq!(state, ServiceState::Stopped);
        assert_eq!(pid, None);
        assert!(
            !survivor.still_running(),
            "the adopted process outlived the stop of the service it belongs to"
        );
    }

    /// The other half of M1: what did *not* survive is cleaned, and cleaning it signals nothing.
    ///
    /// **The pid in the row is this test process's own**, which is the strongest way to assert the
    /// second half: if the identity check were skipped — if a pid alone were taken for a service —
    /// this test would not fail, it would be killed. What makes the pair not match is a start time
    /// one tick from the real one, which is exactly what a recycled pid looks like from in here.
    #[tokio::test]
    async fn a_row_whose_process_did_not_survive_is_cleared_and_nothing_is_signalled() {
        let (_home, paths, store) = home(&["caddy"]).await;
        let declared = Declared(vec![spec("caddy").build().expect("a usable spec")]);
        let registry = registry(&paths, &store, Arc::new(declared));

        let ours = std::process::id();
        let mistaken = mixengine_platform::process::started_at(ours)
            .expect("this system can be asked when a process began")
            .expect("this process is running")
            .stored()
            + 1;

        services::transition(
            &store,
            &service("caddy"),
            ServiceState::Starting,
            StateReason::Requested,
            now(),
        )
        .await
        .expect("a stopped service can start");
        services::transition(
            &store,
            &service("caddy"),
            ServiceState::Running,
            StateReason::Ready,
            now(),
        )
        .await
        .expect("a starting service can be running");
        services::started(&store, &service("caddy"), ours, Some(mistaken), now())
            .await
            .expect("the row takes a process");

        let mut watching = registry.events.subscribe();

        let recovered = registry.recover().await;

        assert_eq!(recovered.cleared, vec![service("caddy")], "{recovered:?}");
        assert!(recovered.adopted.is_empty(), "{recovered:?}");

        let (state, pid) = row(&store, &service("caddy")).await;
        assert_eq!(state, ServiceState::Stopped);
        assert_eq!(
            pid, None,
            "a row that kept a pid would be adopted by the next daemon, and by then it is somebody \
             else's"
        );

        // Through `Stopping`, which is the only edge into `Stopped`, and with the reason a person
        // reading the service list needs to understand why a service they left running is not.
        let mut seen = Vec::new();
        while seen.len() < 2 {
            let frame = tokio::time::timeout(EVENTUALLY, watching.next())
                .await
                .expect("the stream is not silent")
                .expect("the stream is still open");

            if let crate::api::events::Frame::Event(DaemonEvent::ServiceStateChanged(change)) =
                frame
            {
                seen.push(change);
            }
        }

        assert_eq!(seen[0].to, ServiceState::Stopping);
        assert_eq!(seen[1].to, ServiceState::Stopped);
        assert_eq!(seen[1].reason, StateReason::Vanished);
    }

    /// A survivor nothing declares cannot be supervised, and is not left holding the port either.
    ///
    /// The state a service is left in is what a user is told, so it says which of the two things
    /// went wrong — the process was there, and this daemon had no declaration to run it against.
    #[tokio::test]
    async fn a_survivor_nothing_declares_any_more_is_stopped() {
        let (_home, paths, store) = home(&["caddy"]).await;
        let registry = registry(&paths, &store, Arc::new(Declared(Vec::new())));

        let mut survivor = left_running(&store, &service("caddy"), ServiceState::Running).await;

        let recovered = registry.recover().await;

        assert_eq!(recovered.stopped, vec![service("caddy")], "{recovered:?}");
        assert!(recovered.adopted.is_empty(), "{recovered:?}");

        assert!(
            gone(&mut survivor).await,
            "a service nothing declares was left running with nothing supervising it, which is the \
             process the next start collides with"
        );

        let (state, pid) = row(&store, &service("caddy")).await;
        assert_eq!(state, ServiceState::Stopped);
        assert_eq!(pid, None);
        assert!(
            registry.supervised().is_empty(),
            "nothing can be supervising a service that has no declaration"
        );
    }

    /// A daemon that cannot be *told* what is declared stops the same survivors — and must not tell
    /// their owner they were undeclared.
    ///
    /// The two look identical from inside the reconciliation and are opposite problems outside it:
    /// one service was removed and its process goes with it, the other is declared perfectly well by
    /// a source this daemon could not read. `mix service list` is where a person meets the
    /// difference, and a row saying "nothing declares this" would send them looking for a
    /// declaration that is sitting right there.
    #[tokio::test]
    async fn a_survivor_stopped_because_the_declarations_could_not_be_read_says_so() {
        let (_home, paths, store) = home(&["caddy"]).await;
        let registry = registry(&paths, &store, Arc::new(Unavailable));

        let mut survivor = left_running(&store, &service("caddy"), ServiceState::Running).await;

        let mut watching = registry.events.subscribe();

        let recovered = registry.recover().await;

        assert_eq!(recovered.stopped, vec![service("caddy")], "{recovered:?}");
        assert!(
            gone(&mut survivor).await,
            "a survivor was left running by a daemon that could not tell whether it was declared"
        );

        let stopped = loop {
            let frame = tokio::time::timeout(EVENTUALLY, watching.next())
                .await
                .expect("the stream is not silent")
                .expect("the stream is still open");

            if let crate::api::events::Frame::Event(DaemonEvent::ServiceStateChanged(change)) =
                frame
                && change.to == ServiceState::Stopped
            {
                break change;
            }
        };

        let StateReason::Unadopted { reason } = &stopped.reason else {
            panic!("a survivor this daemon stopped is not {}", stopped.reason);
        };

        assert!(
            reason.contains("could not read which services are declared"),
            "the reason does not say why this daemon had nothing to supervise the process \
             against: {reason}"
        );
        assert!(
            !reason.contains("nothing declares"),
            "a service whose declarations could not be read was reported as undeclared: {reason}"
        );
    }

    /// A reconciliation that could not finish is not a quiet boot.
    ///
    /// The one outcome that leaves the machine as it found it — a survivor that would not go, whose
    /// row still names it — has to reach the summary `mixengined` writes, or the line a person opens
    /// `daemon.log` for says nothing happened in exactly the boot where something did. Asserted on
    /// the value rather than through a process, because a process that survives `SIGKILL` and
    /// `TerminateProcess` is not something a test can arrange on three operating systems.
    #[test]
    fn a_survivor_that_would_not_stop_is_not_reported_as_nothing_to_do() {
        let refused = Recovery {
            refused: vec![service("caddy")],
            ..Recovery::default()
        };

        assert!(!refused.is_empty(), "{refused:?}");
        assert!(Recovery::default().is_empty());
    }

    /// A process is not enough: a service left mid-start cannot be resumed, so it is stopped.
    ///
    /// Its readiness was never decided and cannot be decided now — the ready check most specs use
    /// matches a log pattern, and the pipes it would read went with the daemon that died. Adopting
    /// it as though it were up would route traffic to a service nothing ever proved was listening.
    #[tokio::test]
    async fn a_service_left_mid_start_is_stopped_rather_than_taken_for_ready() {
        let (_home, paths, store) = home(&["caddy"]).await;
        let declared = Declared(vec![spec("caddy").build().expect("a usable spec")]);
        let registry = registry(&paths, &store, Arc::new(declared));

        let mut survivor = left_running(&store, &service("caddy"), ServiceState::Starting).await;

        let recovered = registry.recover().await;

        assert_eq!(recovered.stopped, vec![service("caddy")], "{recovered:?}");
        assert!(
            gone(&mut survivor).await,
            "a service that was still starting was adopted rather than stopped"
        );
        assert_eq!(
            row(&store, &service("caddy")).await.0,
            ServiceState::Stopped
        );
    }

    /// Adoption ends where an ordinary life begins: the moment the survivor exits, its policy has
    /// it, and what the policy starts is a child of *this* daemon — pipes, group and log capture
    /// restored.
    #[tokio::test]
    async fn an_adopted_service_that_ends_is_put_back_as_this_daemon_s_own() {
        let (_home, paths, store) = home(&["caddy"]).await;
        let declared = Declared(vec![
            spec("caddy")
                .restart(RestartPolicy::Always {
                    backoff: Backoff {
                        initial: Millis(50),
                        max: Millis(50),
                        ..Backoff::default()
                    },
                })
                .build()
                .expect("a usable spec"),
        ]);
        let registry = registry(&paths, &store, Arc::new(declared));

        // A survivor that is going to end by itself shortly after being adopted.
        let mut survivor = left_running_with(
            &store,
            &service("caddy"),
            ServiceState::Running,
            FakeService::new().exit_after(750),
        )
        .await;
        let first = survivor.id();

        registry.recover().await;

        // The row naming a *different* process is the whole assertion: the exit was noticed, the
        // policy was asked, and what it started was spawned here rather than adopted.
        let deadline = tokio::time::Instant::now() + EVENTUALLY;
        loop {
            let (state, pid) = row(&store, &service("caddy")).await;

            if state == ServiceState::Running && pid.is_some_and(|pid| pid != i64::from(first)) {
                break;
            }

            assert!(
                tokio::time::Instant::now() < deadline,
                "the adopted service was not put back by its policy: it is {state} with pid {pid:?}"
            );

            tokio::time::sleep(Duration::from_millis(25)).await;
        }

        assert!(
            !survivor.still_running(),
            "the process that was adopted is somehow still running"
        );

        registry.shutdown.cancel();
        tokio::time::timeout(EVENTUALLY, registry.shut_down())
            .await
            .expect("every runner finished");
    }

    #[tokio::test]
    async fn every_state_change_is_published_as_it_is_persisted() {
        let (_home, paths, store) = home(&["caddy"]).await;
        let declared = Declared(vec![spec("caddy").build().expect("a usable spec")]);
        let registry = registry(&paths, &store, Arc::new(declared));

        let mut watching = registry.events.subscribe();

        let graph = registry.graph().await.expect("one declared service");
        let plan = graph.start_plan([&service("caddy")]).expect("a plan");
        registry.start(&graph, &plan).await;

        let mut seen = Vec::new();
        while seen.len() < 2 {
            let frame = tokio::time::timeout(EVENTUALLY, watching.next())
                .await
                .expect("the stream is not silent")
                .expect("the stream is still open");

            if let crate::api::events::Frame::Event(DaemonEvent::ServiceStateChanged(change)) =
                frame
            {
                seen.push(change);
            }
        }

        assert_eq!(seen[0].to, ServiceState::Starting);
        assert_eq!(seen[0].reason, StateReason::Requested);
        assert_eq!(seen[1].to, ServiceState::Running);
        assert_eq!(seen[1].reason, StateReason::Ready);
    }

    #[tokio::test]
    async fn a_daemon_on_its_way_out_waits_for_what_it_supervises() {
        let (_home, paths, store) = home(&["caddy"]).await;
        let declared = Declared(vec![spec("caddy").build().expect("a usable spec")]);
        let registry = registry(&paths, &store, Arc::new(declared));

        let graph = registry.graph().await.expect("one declared service");
        let plan = graph.start_plan([&service("caddy")]).expect("a plan");
        registry.start(&graph, &plan).await;

        // What a signal does: the root token every runner hangs off is cancelled, and then the
        // daemon waits rather than dropping the tasks on the floor.
        registry.shutdown.cancel();
        tokio::time::timeout(EVENTUALLY, registry.shut_down())
            .await
            .expect("every runner finished");

        let (state, pid) = row(&store, &service("caddy")).await;
        assert_eq!(state, ServiceState::Stopped);
        assert_eq!(pid, None);
        assert!(lock(&registry.running).is_empty());
    }

    #[tokio::test]
    async fn a_source_that_cannot_answer_is_kept_apart_from_a_declaration_that_is_wrong() {
        let (_home, paths, store) = home(&[]).await;
        let refusing = registry(&paths, &store, Arc::new(Unavailable));

        let error = refusing.graph().await.expect_err("the source refused");

        let Undeclarable::Unavailable(why) = &error else {
            panic!("a source that failed is the daemon's problem, not the user's: {error:?}");
        };
        assert!(
            // The source's own sentence, kept rather than replaced: this crate does not know what
            // T30 will fail at, so it must not write the failure down in its own words.
            why.to_string().contains("is not installed"),
            "{why}"
        );

        // And the other half: a set that is not a graph is what the user declared.
        let cycle = Declared(vec![
            spec("a")
                .depends_on(service("b"))
                .build()
                .expect("a usable spec"),
            spec("b")
                .depends_on(service("a"))
                .build()
                .expect("a usable spec"),
        ]);
        let declaring_a_cycle = registry(&paths, &store, Arc::new(cycle));

        let error = declaring_a_cycle
            .graph()
            .await
            .expect_err("two services in a cycle");

        assert!(
            matches!(
                error,
                Undeclarable::Invalid(mixengine_core::Error::Graph(_))
            ),
            "{error:?}"
        );
    }
}
