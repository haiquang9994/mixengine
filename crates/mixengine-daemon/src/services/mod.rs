//! The registry of running services: what the daemon is supervising, and how a plan is walked.
//!
//! **This is where the timing lives.** `mixengine-supervisor` has the mechanisms and no loop;
//! `mixengine-core` has the graph, the state machine and the row; the daemon is what holds a task
//! per service, the [`CancellationToken`] it stops on, and the clock both of those are measured by.
//! Roadmap task **T19**.
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
use mixengine_proto::{DaemonEvent, ServiceId, ServiceSpec, ServiceState, StateReason, Timestamp};
use tokio::sync::{Notify, watch};
use tokio::task::JoinHandle;
use tokio_util::sync::CancellationToken;

use crate::api::Events;
use runner::{Readiness, Runner};

pub(crate) use spec::{SpecSource, Undeclared};

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
    /// [`None`] as the reason when the failure was the daemon's own — see [`Start::Failed`].
    pub(crate) failed: Option<(ServiceId, Option<StateReason>)>,

    /// Services never tried, because something they depend on failed.
    pub(crate) blocked: Vec<ServiceId>,
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
    /// A service that is not running is already where the caller wants it, which is why this cannot
    /// fail: there is no state "stopped" fails to reach.
    pub(crate) async fn stop(&self, plan: &Plan) -> Walk {
        let mut walk = Walk::default();

        for id in plan.flat() {
            self.stop_one(id).await;
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
    /// never writes one behind `core`'s back. It answers the other question: a row that says
    /// `running` with nothing in here is what a daemon that was killed left behind, and until T18
    /// adopts or clears those, `service.list` saying so is the only place the difference is visible.
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
                    runner.run().await;

                    let mut running = lock(&deregister);
                    if running
                        .get(&named)
                        .is_some_and(|entry| entry.generation == generation)
                    {
                        running.remove(&named);
                    }
                });

                running.insert(
                    id.clone(),
                    Running {
                        cancel,
                        asked_to_start,
                        task,
                        generation,
                        readiness: readiness.clone(),
                    },
                );

                (readiness, None)
            }
        };

        match asked {
            Some(before) => settled_after_asking(&mut readiness, before).await,
            None => settled(&mut readiness).await,
        }
    }

    /// Cancel one service and wait for its runner to finish.
    async fn stop_one(&self, id: &ServiceId) {
        let Some(entry) = lock(&self.running).remove(id) else {
            return;
        };

        entry.cancel.cancel();

        if let Err(error) = entry.task.await {
            tracing::warn!(
                service = id.as_str(),
                %error,
                "the task supervising this service did not finish cleanly"
            );
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
        registry.stop(&stopping).await;

        let (state, pid) = row(&store, &service("caddy")).await;
        assert_eq!(state, ServiceState::Stopped);
        assert_eq!(pid, None, "a stopped service names no process");
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
