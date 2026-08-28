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

#[cfg(debug_assertions)]
mod fakeservice;
mod first_run;
#[cfg(test)]
pub(crate) mod fixture;
pub(crate) mod idle;
pub(crate) mod limits;
pub(crate) mod logs;
mod ports;
mod runner;
mod spec;

pub(crate) use spec::generator;

use std::collections::{BTreeSet, HashMap};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Mutex, MutexGuard, PoisonError};
use std::time::{Duration, SystemTime};

use mixengine_core::generate::Generated;
use mixengine_core::services::{self, Plan, ServiceGraph};
use mixengine_core::{Paths, Store};
use mixengine_platform::Host;
use mixengine_platform::process::{Adopted, StartTime};
use mixengine_proto::{DaemonEvent, ServiceId, ServiceSpec, ServiceState, StateReason, Timestamp};
use tokio::sync::{Notify, watch};
use tokio::task::JoinHandle;
use tokio::time::Instant;
use tokio_util::sync::CancellationToken;

use crate::api::Events;
use logs::Logs;
use runner::{Readiness, Runner, gone};

pub(crate) use spec::{SpecSource, catalogue, declared};

/// The moment every stop now in flight has to be finished by — roadmap task **T9a**.
///
/// **One deadline for the whole daemon, not one per service, and that is the point.** A
/// [`ServiceSpec`] says how long *its* service needs in order to shut down cleanly, which is a
/// statement about MariaDB and stays true however many other services there are; what nothing owned
/// until this task is the sum. Eight of them each asking for ten seconds is eighty seconds a user is
/// sitting through after typing `mix daemon stop`, and a `StopBehaviour::Command` that really runs a
/// program is what turned that from arithmetic into something that happens (see T15a).
///
/// So a shutdown sets this once and every runner reads it as it stops: each service gets what its
/// spec asks for or what is left, whichever is less. The rule is the one T15a applied a level down,
/// inside a single grace period — whatever is allowed, minus what has already been spent.
///
/// [`None`] is the ordinary state of the daemon and means *no ceiling but the spec's*: a
/// `mix service stop mariadb` on a running machine is not a shutdown and has no total to divide.
///
/// Shared rather than passed, because the two things that need to agree about it are at opposite
/// ends of the process — the API handler that grants the budget, and a runner task started an hour
/// earlier that will spend it. A `std` mutex holding a `Copy` value: nothing awaits while it is held.
///
/// [`ServiceSpec`]: mixengine_proto::ServiceSpec
#[derive(Debug, Clone, Default)]
pub(crate) struct Budget(Arc<Mutex<Option<Deadlines>>>);

/// The two moments a shutdown keeps: when every stop has to be finished, and the one after it.
///
/// The second exists for [`Budget::reprieve`] and is [`CONFIRMATION_REPRIEVE`](crate::CONFIRMATION_REPRIEVE)
/// past the first. Held together rather than in two locks because they are narrowed as one decision.
#[derive(Debug, Clone, Copy)]
struct Deadlines {
    /// What every stop now in flight has to be finished by.
    stops: Instant,

    /// What the one wait a spent budget still permits has to be finished by.
    reprieve: Instant,
}

impl Budget {
    /// Give everything that stops from now on until `deadline`, and no longer.
    ///
    /// **Narrowing only.** A shutdown that arrives while another is running — a `daemon.shutdown`
    /// answered by a client, and the console event that closed the window a moment later — must not
    /// have its clock extended by the second one: the OS ceiling that motivated the tighter of the
    /// two is not something the daemon may grant itself more of. Both deadlines narrow together, so
    /// the reprieve of a shutdown that has been tightened is the tighter one's.
    fn narrow_to(&self, deadline: Instant) {
        let asked = Deadlines {
            stops: deadline,

            // Saturating in the only direction that matters: a deadline so far out that adding a
            // quarter second to it overflows is one nothing was ever going to reach anyway, and the
            // reprieve then grants exactly what the deadline itself does.
            reprieve: deadline
                .checked_add(crate::CONFIRMATION_REPRIEVE)
                .unwrap_or(deadline),
        };

        let mut held = lock(&self.0);

        *held = Some(held.map_or(asked, |existing| Deadlines {
            stops: existing.stops.min(asked.stops),
            reprieve: existing.reprieve.min(asked.reprieve),
        }));
    }

    /// How much of it is left, or [`None`] when nothing is counting.
    ///
    /// Zero once the deadline has passed, which is a real answer rather than a missing one: a
    /// service reached after the budget ran out is killed at once instead of starting a fresh grace
    /// period of its own.
    pub(super) fn remaining(&self) -> Option<Duration> {
        lock(&self.0).map(|held| held.stops.saturating_duration_since(Instant::now()))
    }

    /// How much is left of the window a wait may reach into once [`Budget::remaining`] is zero.
    ///
    /// **For the one wait that zero answers wrongly** — the poll that asks whether a killed survivor
    /// has left the process table, where no window at all reports a process that stopped correctly
    /// as one that would not go. See [`CONFIRMATION_REPRIEVE`](crate::CONFIRMATION_REPRIEVE) for why
    /// that question is not like the other waits the budget shortens, and
    /// [`Runner::seeing_it_go`](runner::Runner) for the only caller.
    ///
    /// **Shared across the walk**, because it is a moment and not an allowance: two survivors reached
    /// after the budget ran out look into the same window rather than into one each, which is what
    /// keeps this out of the ceiling arithmetic.
    ///
    /// [`None`] when nothing is counting, and the caller then has no reason to ask — there is no
    /// spent budget for it to be making up for.
    pub(super) fn reprieve(&self) -> Option<Duration> {
        lock(&self.0).map(|held| held.reprieve.saturating_duration_since(Instant::now()))
    }
}

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

    /// What a shutdown has left to spend, once one is under way — see [`Budget`].
    budget: Budget,

    /// Whether a shutdown has begun, which is what [`Registry::begin`] refuses new work on.
    ///
    /// **Separate from [`Registry::budget`] rather than derived from it**, which is the tempting
    /// shape and the wrong one: a budget is what a shutdown was *granted*, and a shutdown can begin
    /// without one being expressible — see [`Registry::stopping_within`], where a grace this
    /// machine's clock cannot name leaves the budget as it was. Deriving the question from the
    /// answer would make that exact shutdown the one that goes on starting services.
    shutting_down: AtomicBool,

    /// One entry per service with a task supervising it.
    ///
    /// A `std` mutex rather than tokio's: nothing awaits while holding it, and the alternative
    /// would make every reader of "what is running" an async function for no reason.
    running: Arc<Mutex<HashMap<ServiceId, Running>>>,

    /// What a client reads on `GET /logs/{id}` — roadmap task **T16b**.
    ///
    /// **Deliberately not part of [`Registry::running`]**, although every line in it comes from
    /// something that is: a `follow` has to survive the run it started in, and an entry that lived
    /// in the map above would be dropped by the crash it is most worth watching. See
    /// [`logs::Logs`], and
    /// `.claude/decisions/0009-logs-travel-on-their-own-stream.md` for why output is not an event.
    logs: Arc<Logs>,

    /// One entry per stop in flight, so a second caller waits for it instead of racing it — see
    /// [`Stopping`] and [`Registry::stop_one`].
    ///
    /// **Taken after [`Registry::running`] wherever both are held**, which is [`Registry::stop_one`]
    /// and [`Registry::shut_down`] and nowhere else. The order is what the pairing needs rather than
    /// merely what avoids a deadlock: claiming a stop and taking its entry out of the map have to be
    /// one decision, or a shutdown draining between the two leaves a claim nobody can wait on.
    stopping: Arc<Mutex<HashMap<ServiceId, watch::Receiver<bool>>>>,

    /// Hands out the generation below.
    generations: AtomicU64,

    /// The job registry, for the one thing a start may have to do that is too long to answer inline:
    /// a first-run ritual — roadmap task **T33**.
    jobs: Arc<crate::jobs::Jobs>,

    /// What each declared service still has to have done to it once, refreshed at every walk.
    ///
    /// **Held here rather than carried through [`Registry::start`]'s signature**, beside
    /// [`Registry::hand_over`] and for its reason: [`Registry::graph`] is the only place both halves
    /// are visible — the source knows what it generated, and this registry knows what it is about to
    /// begin. A [`ServiceGraph`] carries specs, and a ritual is not one.
    rituals: Arc<Mutex<HashMap<ServiceId, mixengine_core::generate::FirstRun>>>,
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

    /// Notify it to hand the running process the configuration that has just been rewritten under
    /// it — roadmap task **T31**, and see [`Registry::hand_over`].
    ///
    /// The sibling of [`Running::asked_to_start`] in mechanism and its opposite in what it means: a
    /// start is asked of a service that is *not* up, and a reload only ever of one that is.
    asked_to_reload: Arc<Notify>,

    /// Push a new set of ceilings at the runner, which writes them into the live process.
    ///
    /// **A `watch` rather than a [`Notify`] like its two neighbours**, and the difference is that
    /// this one carries a value: `asked_to_start` and `asked_to_reload` ask the runner to go and look
    /// at something that has already been written down, while a limit *is* the message. The same
    /// channel type [`Running::readiness`] uses, running the other way.
    limits: watch::Sender<mixengine_proto::ResourceLimits>,

    /// Tell the runner why the stop it is about to perform is happening — roadmap task **T69**.
    ///
    /// [`None`] is *somebody asked*, and it is what every stop but one is. Set by
    /// [`Registry::stopping_because`] immediately before the cancel, so the transition a client
    /// reads says `idle` rather than claiming a person asked for it.
    stopping_because: watch::Sender<Option<StateReason>>,

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

/// One stop of one service, held for exactly as long as that stop is in flight.
///
/// **A guard rather than two statements, because a stop does not only end by returning.** An
/// `api/rpc.rs` handler is a `tokio::spawn` nothing joins, so the future performing a stop can be
/// dropped where it stands when its connection goes — and a runner task that panicked mid-tidy-up
/// takes its caller's frame with it. Whatever ends the stop, the entry has to leave the map and
/// everybody waiting on it has to be released: a marker left behind would make every later stop of
/// that service wait for a stop that is not happening, and then report it as done.
///
/// The value published is *"no longer in flight"* and never *"it worked"*. Which is a distinction
/// with a reason: the answer to a stop comes from the row and from nothing else — see
/// [`Registry::stop_one`] — so what a second caller needs from this is the moment at which reading
/// that row is honest, not a second opinion travelling beside it.
#[derive(Debug)]
struct Stopping {
    /// Which service's stop this is.
    service: ServiceId,

    /// The map to take it out of, shared with the registry that handed it out.
    stopping: Arc<Mutex<HashMap<ServiceId, watch::Receiver<bool>>>>,

    /// What releases whoever is waiting for this stop.
    finished: watch::Sender<bool>,
}

impl Drop for Stopping {
    fn drop(&mut self) {
        lock(&self.stopping).remove(&self.service);

        // Ignored, and the two ways it fails are both ordinary: nobody is waiting, or everybody who
        // was has gone. Neither is news, and a stop that has ended cannot do anything about either.
        let _ = self.finished.send(true);
    }
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
/// **Both halves carry the same type, and the variant is still the whole point.** What differs is
/// where the failure came from and therefore who has to act on it, not what it is made of: a source
/// failure and a bad graph are both [`mixengine_core::Error`], because that crate is the one with
/// the vocabulary. `Unavailable` used to hold an `anyhow::Error` instead, and the wire mapping had
/// to downcast back to this type to keep the code it had thrown away.
///
/// Not an [`std::error::Error`] itself: nothing wraps it, and the one caller matches it and hands
/// each half to the mapping that already exists for it — [`crate::error::ToWire`], where the
/// `service.*` handlers meet it.
#[derive(Debug)]
pub(crate) enum Undeclarable {
    /// The source could not produce them: a package that is not installed, a template that does not
    /// render, a database that cannot be read.
    Unavailable(mixengine_core::Error),

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
        jobs: Arc<crate::jobs::Jobs>,
    ) -> Self {
        Self {
            paths: paths.clone(),
            store: store.clone(),
            host,
            events,
            specs,
            shutdown,
            budget: Budget::default(),
            shutting_down: AtomicBool::new(false),
            running: Arc::new(Mutex::new(HashMap::new())),
            logs: Arc::new(Logs::new()),
            stopping: Arc::new(Mutex::new(HashMap::new())),
            generations: AtomicU64::new(0),
            jobs,
            rituals: Arc::new(Mutex::new(HashMap::new())),
        }
    }

    /// Everything that stops from now on has until `now + grace`, however long its own spec asks
    /// for — roadmap task **T9a**.
    ///
    /// Called by both ways a daemon stops and by nothing else: `daemon.shutdown` before it walks the
    /// stop plan, and the accept loop before it cancels the root token on a signal. The two differ
    /// only in the number they pass, which is where "one budget, two ceilings" lives — see
    /// [`Budget`] for why a second call can only ever shorten it.
    ///
    /// **A `grace` this clock cannot name leaves the budget where it was, and that is arithmetic
    /// rather than a fallback.** `Instant + Duration` panics on overflow, and this is the one path
    /// where a panic costs more than the request it is in: the signal half runs on the main task, so
    /// the unwinding goes straight past `Store::close` and leaves the write-ahead log
    /// uncheckpointed — a `-wal` sidecar holding the newest commits, for a number somebody typed
    /// into `config.toml`. `mixengine-core` clamps `shutdown_grace_seconds` on the way in and this
    /// deliberately does not rest on that: one clamp is a decision about a config file, and this is
    /// the shutdown path, which must be unable to panic on whatever reaches it.
    ///
    /// Skipping the call is what the arithmetic would have done anyway. A deadline centuries out
    /// loses every `min` in [`Budget::narrow_to`] and bounds nothing, which is exactly what
    /// [`Budget`]'s [`None`] already means — *no ceiling but the spec's* — and a grace that large is
    /// a request for precisely that. What it must not also mean is "no shutdown", which is why the
    /// flag below is set before the arithmetic is attempted rather than after it succeeds.
    pub(crate) fn stopping_within(&self, grace: Duration) {
        self.shutting_down.store(true, Ordering::Relaxed);

        let Some(deadline) = Instant::now().checked_add(grace) else {
            tracing::warn!(
                ?grace,
                "this shutdown was granted more time than this machine's clock can name; every \
                 service gets the grace period its own spec asks for and nothing bounds the sum"
            );

            return;
        };

        self.budget.narrow_to(deadline);
    }

    /// Whether this daemon has begun going away, by either of the two routes it has.
    ///
    /// **Two, because the flag and the token are set at different moments and neither is early
    /// enough on its own.** `daemon.shutdown` grants a budget and *then* walks the stop plan in
    /// dependency order, cancelling the root token only once that walk is done — so for the whole
    /// of the walk, the token says nothing has happened. A signal cancels the token, and a test or
    /// a future caller may cancel it without granting anything. Either one is this daemon on its
    /// way out, and [`Registry::begin`] has to refuse from the first of them.
    ///
    /// Also read by `daemon.shutdown`, which is what makes it `pub(crate)`: a request to stop that
    /// arrives when one of these is already true is somebody asking a second time, and what that
    /// means is [`Registry::stopping_within`] with nothing left in it.
    pub(crate) fn is_shutting_down(&self) -> bool {
        self.shutting_down.load(Ordering::Relaxed) || self.shutdown.is_cancelled()
    }

    /// The declared services, checked and ordered.
    ///
    /// Asked afresh rather than cached, because the set is a *rendering* of state that changes —
    /// a service created, a port edited, a package upgraded — and a graph held from startup would
    /// answer for a home that no longer exists.
    ///
    /// **It is also where a running service learns its configuration moved**, which is roadmap task
    /// **T31** and is here because this is the only place both halves are visible: the source knows
    /// what it rewrote, and this map knows what is running. Asking anywhere else would mean either a
    /// generator that reaches into the registry or a walk that re-reads files it has already
    /// overwritten. See [`Registry::hand_over`].
    ///
    /// # Errors
    ///
    /// [`Undeclarable`], which keeps the two apart on purpose: a source that failed is the daemon's
    /// problem, and a set that is not a graph is the user's declaration and belongs in
    /// `invalid_argument`.
    /// Bring every service's configuration up to date, and hand any change to whatever is running.
    ///
    /// [`graph`](Self::graph) without the graph, for the caller that changed something a *front end*
    /// renders rather than something about a service: a site written, updated, deleted, started or
    /// stopped. The rendering is the whole of what it wants — `mix site create` has no use for a
    /// start order.
    ///
    /// **A failure fails the caller**, which is deliberate and is not the hosts queue's behaviour.
    /// A hosts entry that has not been granted yet is a want with a person on the other end of it; a
    /// configuration the server refused is a defect, and a `site.create` that returned success while
    /// the site was unreachable would send whoever typed it to look in the wrong place. Nothing was
    /// installed either way — `document::install` stages first.
    ///
    /// # Errors
    ///
    /// [`Undeclarable`], exactly as [`graph`](Self::graph) reports it.
    pub(crate) async fn reconfigure(&self) -> Result<(), Undeclarable> {
        self.graph().await.map(drop)
    }

    pub(crate) async fn graph(&self) -> Result<ServiceGraph, Undeclarable> {
        let generated = self
            .specs
            .declared()
            .await
            .map_err(Undeclarable::Unavailable)?;

        self.hand_over(&generated);
        self.remember_rituals(&generated);

        ServiceGraph::new(
            generated
                .into_iter()
                .map(|one| one.spec)
                .collect::<Vec<_>>(),
        )
        .map_err(|error| Undeclarable::Invalid(mixengine_core::Error::Graph(error)))
    }

    /// The absolute path of the program this home's front end runs, or [`None`].
    ///
    /// **By what a package is for, not by what it is called** — [`services::front_end::held_by`]
    /// answers that, by role rather than by name, so a third front end added later is found without
    /// a string being added to a list here.
    ///
    /// Rendering the graph is what turns a row into a program path, and [`recover`](Self::recover)
    /// has already done it a moment before this is called at start-up: a second rendering identical
    /// to the first writes nothing and reloads nothing.
    ///
    /// [`None`] for every failure as well as for a home with no front end. The caller is a start-up
    /// step that must not fail a start, and "we could not find out" and "there is nothing to find"
    /// lead to the same next move.
    pub(crate) async fn front_end_program(&self) -> Option<std::path::PathBuf> {
        let id = services::front_end::held_by(&self.store, &catalogue())
            .await
            .inspect_err(|error| {
                tracing::warn!(%error, "cannot tell which service is this home's front end");
            })
            .ok()
            .flatten()?;

        let id = ServiceId::parse(&id).ok()?;

        let graph = self
            .graph()
            .await
            .inspect_err(|error| {
                tracing::warn!(
                    ?error,
                    "cannot render this home's services, so the front end has no path"
                );
            })
            .ok()?;

        Some(graph.spec(&id)?.program().to_path_buf())
    }

    /// The port this home's front end serves TLS on, or [`None`] — roadmap task **T53**.
    ///
    /// **From the settings a rendering would be made with**, which is the only source that cannot
    /// come to disagree with the configuration on disk. The other two are both refused: a second
    /// `Settings::merge` here could drift from the generator's, and reading the number out of the
    /// rendered file is what `.claude/CLAUDE.md` forbids outright — generated configuration is
    /// disposable and is never parsed back into state.
    ///
    /// **`settings` and never `generate`.** [`generate`](mixengine_core::generate::Generator::generate)
    /// installs, so reaching the port that way would have `mix cert status` — a command whose whole
    /// guarantee is that it writes nothing — rewrite this home's configuration and possibly reload
    /// a running server as a side effect of being asked a question.
    ///
    /// [`None`] for a home with no front end and for every failure alike, on
    /// [`front_end_program`](Self::front_end_program)'s reasoning: the caller reports "nothing is
    /// serving this", and "we could not find out" leads to the same sentence.
    pub(crate) async fn front_end_tls_port(&self) -> Option<u16> {
        let id = services::front_end::held_by(&self.store, &catalogue())
            .await
            .inspect_err(|error| {
                tracing::warn!(%error, "cannot tell which service is this home's front end");
            })
            .ok()
            .flatten()?;

        let id = ServiceId::parse(&id).ok()?;

        let settings = self
            .specs
            .settings(&id)
            .await
            .inspect_err(|error| {
                tracing::warn!(
                    %error,
                    "cannot read this home's front end settings, so it has no TLS port"
                );
            })
            .ok()
            .flatten()?;

        u16::try_from(settings.number("https_port"))
            .ok()
            .filter(|port| *port != 0)
    }

    /// Tell every service that is running and whose configuration just changed to re-read it.
    ///
    /// **A notification and not a command**, which is the whole design: what runs the reload is the
    /// runner, in the surroundings the service itself was started with, exactly as its health probe
    /// and its stop command are run. Doing it from here would mean a second, lesser copy of
    /// [`Surroundings`](mixengine_supervisor::Surroundings) — and it would put a subprocess with a
    /// thirty-second patience inside `service.list`, which is a call a GUI makes on a timer.
    ///
    /// **Only what is up.** A service that is stopped will read the new file when it is started, and
    /// one in a restart backoff will read it on the attempt after this. Neither is a reload, and
    /// asking for one would be a command sent to an address nothing is listening on.
    ///
    /// A permit left with [`Notify`] rather than a message is what makes this safe to do on every
    /// walk: a runner busy in a health probe collects it at the top of its next turn, and two walks
    /// that both find a change while it is busy collapse into the one reload that was needed. The
    /// service is left alone if it has no [`ReloadBehaviour`](mixengine_proto::ReloadBehaviour) —
    /// the runner says so once, because a configuration that changed and reached nothing is worth a
    /// line in `daemon.log`.
    /// Keep what each declared service still has to have done to it once — roadmap task **T33**.
    ///
    /// **Replaced rather than merged**, so a service that stopped declaring a ritual — an override
    /// pointed at a different data directory, a recipe that no longer has one — stops carrying it.
    /// The whole map is at most one entry per declared service, and each holds a cloned
    /// [`Context`](mixengine_core::generate::Context).
    fn remember_rituals(&self, generated: &[Generated]) {
        let remembered: HashMap<_, _> = generated
            .iter()
            .filter_map(|one| {
                one.first_run
                    .clone()
                    .map(|ritual| (one.spec.id().clone(), ritual))
            })
            .collect();

        *lock(&self.rituals) = remembered;
    }

    /// The ritual this service still has to have performed, if it declared one.
    fn ritual_for(&self, id: &ServiceId) -> Option<mixengine_core::generate::FirstRun> {
        lock(&self.rituals).get(id).cloned()
    }

    /// Ask one running service to re-read its configuration, and say whether there was one to ask.
    ///
    /// [`Registry::hand_over`]'s single-service half, for the one caller whose change is not a file
    /// this registry generated: a runtime's ini set is rewritten by `runtime.set_extension`, and the
    /// pool reading it is supervised here.
    /// Write a new set of ceilings into a service that is running now — roadmap task **T68**.
    ///
    /// `false` when nothing is supervising that id, which is not a failure: a stopped service's
    /// limits live in its row and are read by the next spawn. The caller has already written the row.
    pub(crate) fn set_limits(
        &self,
        id: &ServiceId,
        limits: mixengine_proto::ResourceLimits,
    ) -> bool {
        let running = lock(&self.running);

        let Some(entry) = running.get(id) else {
            return false;
        };

        entry.limits.send_replace(limits);
        true
    }

    /// Say why the next stop of this service is happening, before asking for it — task **T69**.
    ///
    /// **Set, then cancel, in that order**: the runner reads this at the moment it enters
    /// `Stopping`, so a value written after the cancel arrives too late to explain the stop it was
    /// meant for.
    ///
    /// **And cleared again if that stop does not happen**, with `None`, which is what the argument
    /// is an [`Option`] for. A reason left behind by a stop that failed would be read by whatever
    /// stopped the service next — a person asking, most likely — and told them their own
    /// `service.stop` was an idle timeout. The idle sweeper is the only caller and does both.
    ///
    /// [`false`] when nothing is supervising this service, which is a service that is not running
    /// and therefore not one anything is about to stop.
    pub(crate) fn stopping_because(&self, id: &ServiceId, reason: Option<StateReason>) -> bool {
        let running = lock(&self.running);

        let Some(entry) = running.get(id) else {
            return false;
        };

        entry.stopping_because.send_replace(reason);
        true
    }

    pub(crate) fn ask_to_reload(&self, id: &ServiceId) -> bool {
        let running = lock(&self.running);

        let Some(entry) = running.get(id) else {
            return false;
        };

        entry.asked_to_reload.notify_one();
        true
    }

    fn hand_over(&self, generated: &[Generated]) {
        let running = lock(&self.running);

        for one in generated.iter().filter(|one| one.changed()) {
            let Some(entry) = running.get(one.spec.id()) else {
                continue;
            };

            tracing::debug!(
                service = one.spec.id().as_str(),
                "this service's configuration changed while it is running; asking it to re-read it"
            );

            entry.asked_to_reload.notify_one();
        }
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
    /// same cost a stop command that fails carries — see [`Runner::ask_to_stop`](runner) — and it is
    /// stated in the row rather than hidden.
    ///
    /// Adoption writes **no transition** for the service it takes over: nothing happened to it. Its
    /// row said `running` before this process existed and says `running` still.
    pub(crate) async fn recover(&self) -> Recovery {
        let mut recovery = Recovery::default();

        // **First, and before any row is read** — roadmap task **T68**. On Linux a supervised group
        // is a *directory*, so a daemon that was killed leaves one behind for every service it was
        // capping. Nothing on the other two systems, where a group is a kernel object that goes when
        // the last handle to it closes.
        //
        // Safe to run before adoption rather than after, and that is the point: an empty cgroup is
        // removable and a non-empty one is not, so this takes exactly the ones whose processes are
        // already gone and leaves the ones belonging to survivors this reconciliation is about to
        // adopt. It needs no list of what to expect because the kernel is the list.
        mixengine_platform::process::sweep_stale_groups();

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

    /// Reconcile only the rows this registry holds no runner for — roadmap task **T47b**.
    ///
    /// **[`recover`](Self::recover) with the set narrowed, and narrowed for a reason that is not
    /// tidiness.** That function walks *every* row, which is right at a boot where nothing is
    /// supervised yet and wrong on a running daemon: it would adopt or stop services this process is
    /// already watching. The rows a live daemon may decide anything about are the ones it holds no
    /// runner for.
    ///
    /// The decision per row is [`reconcile`](Self::reconcile)'s and is not restated here — adopt when
    /// the pid *and* the recorded start time both match, stop what cannot be resumed or is no longer
    /// declared, and never signal on a pid alone.
    pub(crate) async fn reconcile_stranded(&self) -> Recovery {
        let mut recovery = Recovery::default();

        let records = match services::records(&self.store).await {
            Ok(records) => records,

            Err(error) => {
                tracing::error!(
                    %error,
                    "cannot read the services table; nothing stranded can be reconciled"
                );

                return recovery;
            }
        };

        let declared = match self.graph().await {
            Ok(graph) => Some(graph),

            Err(error) => {
                let reason: &dyn std::fmt::Display = match &error {
                    Undeclarable::Unavailable(why) => why,
                    Undeclarable::Invalid(why) => why,
                };

                tracing::error!(
                    %reason,
                    "cannot tell which services are declared; anything stranded will be stopped \
                     rather than adopted"
                );

                None
            }
        };

        let held = self.supervised();

        for (stored, record) in records {
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

            // The whole of what makes this safe on a running daemon.
            if held.contains(&service) {
                continue;
            }

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

        // **Nothing is marked when the daemon is on its way out**, which is the one case where the
        // sentence this would persist is not true. A walk that `begin` refused did not meet a
        // service that failed, so writing `DependencyFailed` under its name would leave every
        // dependent recorded as broken by a database that was stopping perfectly — and leave it that
        // way on disk, for whoever opens `mix service list` after the next boot.
        if let Some((failed, _)) = &walk.failed
            && !self.is_shutting_down()
        {
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
    ///
    /// **The two things this takes besides the entries are what keep it from colliding with a walk
    /// that is still going.** A signal arriving mid-`daemon.shutdown` gets here while the handler is
    /// still stepping through the stop plan, and both halves of that were a bug the shape of a false
    /// report: a `begin` that had passed its check would register a runner into a map this has
    /// already emptied, and the walk's next [`Registry::stop_one`] would find its entry gone, read a
    /// row still saying `Stopping`, and call a service that is stopping perfectly a stop that
    /// failed. So the flag and every stop in flight are claimed in the same critical section as the
    /// drain — see [`Registry::stopping`] for why the order of the two locks is the pairing rather
    /// than only the deadlock.
    pub(crate) async fn shut_down(&self) {
        let running: Vec<(ServiceId, Running, Option<Stopping>)> = {
            let mut running = lock(&self.running);

            self.shutting_down.store(true, Ordering::Relaxed);

            running
                .drain()
                .map(|(id, entry)| {
                    // `None` is a stop somebody else claimed before this drain reached it, and it is
                    // theirs to release: that caller took its entry out of the map under this same
                    // lock, so it is not one of the entries here.
                    let claimed = self.claim_stop(&id).ok();

                    (id, entry, claimed)
                })
                .collect()
        };

        if running.is_empty() {
            return;
        }

        tracing::info!(
            services = running.len(),
            "waiting for supervised services to stop"
        );

        // Each claim is dropped as its service's turn ends, which is what releases a walk waiting on
        // that one while this is still working through the rest.
        for (id, entry, _stopping) in running {
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

    /// The machine this registry supervises on.
    ///
    /// Handed out for the one caller that has to ask the OS a question of its own before a row
    /// exists: `service.create` allocates a port, and what makes a port free is the machine rather
    /// than the table — see [`mixengine_core::services::ports`]. Reached through here rather than
    /// through `mixengine_platform::host()` so that a test driving the API against a mock host is
    /// answered by that mock and not by whatever is listening on the runner.
    pub(crate) fn host(&self) -> &dyn Host {
        self.host.as_ref()
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

    /// What a client reads on `GET /logs/{id}` — roadmap task **T16b**.
    ///
    /// A read-only handle for the endpoint, which is what the layering asks for: everything that
    /// *writes* a line into one of these is a runner this registry started, and a request handler
    /// only ever asks a service's log what it has and what comes next.
    pub(crate) fn logs(&self) -> &Logs {
        &self.logs
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
    ///
    /// **A daemon on its way out starts nothing, and that answer is taken under the registration
    /// lock like the other two.** `api/rpc.rs` runs every handler in a `tokio::spawn` nothing joins,
    /// so a `service.start` outlives the connection that asked for it: one landing beside a
    /// `mix daemon stop` used to walk past the place the shutdown had already been, register a fresh
    /// runner into a map [`Registry::shut_down`] had already drained, and spawn a process nothing
    /// was left to wait for — killed a moment later by
    /// [`Supervised`](mixengine_platform::process::Supervised)'s destructor, leaving a row mid-write
    /// for the next boot's crash recovery to clean up. Reading the answer outside the lock would
    /// leave that same window open a few instructions narrower; reading it under the lock that
    /// [`Registry::shut_down`] drains under closes it, because then the two are one order.
    ///
    /// It is refused for a service that is already up as well, which is the one case that could
    /// arguably be answered [`Start::Ready`]. A shutdown that has begun is going to stop that
    /// service too, and answering a client that it is up — moments before the daemon takes it
    /// down — would be true for less time than it takes to render.
    async fn begin(&self, spec: &ServiceSpec) -> Start {
        let id = spec.id().clone();

        // **Before the spawn, and outside the lock** — roadmap task T33. A ritual is minutes of
        // work, and holding a `std` mutex across it would stop every other `service.*` call for the
        // whole of a bootstrap. Nothing is registered yet, so a second start arriving mid-ritual
        // finds nothing supervised and asks the same question again — which reads the markers, finds
        // our own in-progress one, and clears and redoes rather than colliding.
        //
        // The refusal below persists nothing, exactly as the shutting-down one does not: no row
        // moved and no event was published, and the durable account is the job row, which holds the
        // step that failed and what it printed.
        if let Some(plan) = self.ritual_for(&id)
            && let Err(error) = first_run::ensure(&self.host, &self.jobs, &plan).await
        {
            tracing::error!(
                service = id.as_str(),
                error = %error,
                "this service has never been started here and its first run did not finish"
            );

            return Start::Failed(Some(StateReason::FirstRunFailed {
                detail: error.message.clone(),
            }));
        }

        let (mut readiness, asked) = {
            // Held across the spawn as well, so that a runner which ends immediately cannot
            // deregister an entry that has not been made yet. Nothing awaits while it is held.
            let mut running = lock(&self.running);

            if self.is_shutting_down() {
                // `None`, because nothing was persisted about this and nothing should be: no row
                // moved, no event was published, and the service is where it was. What a client
                // renders is the walk naming the service it did not start, and the sentence saying
                // why is this line in `daemon.log`.
                tracing::warn!(
                    service = id.as_str(),
                    "refusing to start this service: the daemon is shutting down"
                );

                return Start::Failed(None);
            }

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
        let asked_to_reload = Arc::new(Notify::new());

        // Seeded with what the spec already says, so a runner that never hears from a client still
        // has the right value to apply — and so the first `changed()` is a real change.
        let (limits, limits_asked) = watch::channel(spec.limits());

        // Seeded with `None`: unless the sweeper says otherwise before it cancels, a stop is a stop
        // somebody asked for, which is what every stop before T69 was.
        let (stopping_because, because_asked) = watch::channel(None);

        let runner = Runner {
            spec: spec.clone(),
            store: self.store.clone(),
            directory: self.paths.service_logs(&id),
            // Sized here rather than inside the runner, because this is where a policy meets the
            // log that outlives the runner reading it: the same spec's `ring_lines`, applied to
            // both rings, so what a client is served and what a crash-loop quotes agree about how
            // much of a service is worth keeping.
            log: self.logs.feeding(&id, usize::from(spec.logs().ring_lines)),
            host: Arc::clone(&self.host),
            events: self.events.clone(),
            cancel: cancel.clone(),
            asked_to_start: Arc::clone(&asked_to_start),
            asked_to_reload: Arc::clone(&asked_to_reload),
            limits_asked,
            stopping_because: because_asked,
            budget: self.budget.clone(),
            // Built by the first spawn of this life, or on demand by a service this runner adopted.
            surroundings: None,
            reading: None,
            readiness: published,
        };

        let deregister = Arc::clone(&self.running);
        let logs = Arc::clone(&self.logs);
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

            // After the deregistration and under the same tidy-up, because they answer the same
            // question about the same moment: nothing is supervising this service any more. A
            // client that still has a `follow` open keeps the log — which is what lets its stream
            // carry on when the service is started again — and one that has gone leaves the daemon
            // holding a ring for a service nobody is watching.
            logs.forget_if_unwatched(&named);
        });

        running.insert(
            id,
            Running {
                cancel,
                asked_to_start,
                asked_to_reload,
                limits,
                stopping_because,
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
    /// **A second caller is given the first one's stop rather than a race against it.** Taking the
    /// entry out of the map is what used to make two of these two different stops: whoever arrived
    /// second found [`None`], skipped the wait that the answer depends on, and read a row mid-stop —
    /// `Stopping`, which [`ServiceState::is_supervised`] counts, so a service being shut down
    /// perfectly was reported as one that would not stop, and `mix daemon stop` exited non-zero
    /// pointing at a database that was fine. It happens two ways and neither is exotic: two clients,
    /// or a signal whose [`Registry::shut_down`] drains the map underneath a walk that is still
    /// going. So the stop is claimed before the entry is taken, in the same critical section, and a
    /// caller that finds it claimed waits for the claim to be released and then reads the same row
    /// this would have — see [`Stopping`], and [`Registry::stopping`] for the lock order that makes
    /// claim-and-take one decision rather than two.
    ///
    /// [`runner`]: runner::Runner
    async fn stop_one(&self, id: &ServiceId) -> bool {
        // Bound in a statement of its own so the guards are dropped before the awaits below: an
        // `if let` would hold them across, and a lock held over an await is one no other thread can
        // take for as long as this stop lasts.
        let claimed = {
            let mut running = lock(&self.running);

            self.claim_stop(id)
                .map(|stopping| (stopping, running.remove(id)))
        };

        // Held until the row has been read rather than until the task has ended, so that a third
        // caller arriving in between waits for this answer instead of racing the same read.
        let _stopping = match claimed {
            Ok((stopping, entry)) => {
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

                Some(stopping)
            }

            // Somebody else's stop, and what is waited for is the whole of it. The result is
            // discarded on purpose: a sender dropped without sending is a stop whose caller went
            // away, which is the same news as one that finished — it is no longer in flight, and
            // the row below is what says where it left the service either way.
            Err(mut over) => {
                let _ = over.wait_for(|finished| *finished).await;

                None
            }
        };

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

    /// Become the stop of this service, or be handed the one that already is.
    ///
    /// [`Err`] carries a receiver released when the stop in flight is over, however it ended — see
    /// [`Stopping`]. **The caller holds [`Registry::running`]**, which is what the argument-free
    /// signature cannot say and the two callers both do: a claim taken a statement away from the
    /// entry it belongs to is a claim a concurrent [`Registry::shut_down`] can drain out from under.
    fn claim_stop(&self, id: &ServiceId) -> Result<Stopping, watch::Receiver<bool>> {
        let mut stopping = lock(&self.stopping);

        if let Some(over) = stopping.get(id) {
            return Err(over.clone());
        }

        let (finished, over) = watch::channel(false);
        stopping.insert(id.clone(), over);

        Ok(Stopping {
            service: id.clone(),
            stopping: Arc::clone(&self.stopping),
            finished,
        })
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

    use mixengine_proto::{
        Backoff, Millis, ReadyCheck, ReloadBehaviour, RestartPolicy, StopBehaviour,
    };
    use mixengine_testkit::FakeService;

    use super::fixture::{
        Declared, EVENTUALLY, Rerendered, Unavailable, arguments, home, service, spec,
    };
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
        registry_on(
            paths,
            store,
            specs,
            Arc::new(mixengine_platform::mock::Host::with_home(paths.root())),
        )
    }

    /// [`registry`], for the one test whose subject is what the machine answers.
    fn registry_on(
        paths: &Paths,
        store: &Store,
        specs: Arc<dyn SpecSource>,
        host: Arc<dyn mixengine_platform::Host>,
    ) -> Registry {
        let events = Events::new();
        // A real one, because it is what a start reaches for when a service declares a first-run
        // ritual — and none of these fixtures does, so nothing here ever begins a job through it.
        let jobs = Arc::new(crate::jobs::Jobs::new(
            store,
            events.clone(),
            CancellationToken::new(),
        ));

        Registry::new(
            paths,
            store,
            host,
            events,
            specs,
            CancellationToken::new(),
            jobs,
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

    /// **A service whose spec stops it with a command of its own is stopped by running it** —
    /// roadmap task T15a, and the behaviour Phase 3's MariaDB rests on.
    ///
    /// The fixture is what makes this provable rather than merely plausible. The service ignores
    /// every request to stop, so neither a `SIGTERM` nor — on Windows, where there is no such
    /// request at all (ADR 0008) — anything else can end it politely; the only route left is the
    /// file it watches for, and the only thing that creates that file is the program the spec names
    /// as its stop command. So the two assertions are one claim from two sides: the file is there,
    /// and the row records a *clean* exit, which a kill could not have produced.
    #[tokio::test]
    async fn a_service_with_a_stop_command_is_stopped_by_running_it() {
        let (home, paths, store) = home(&["mariadb"]).await;

        let asked = home.path().join("shutdown-was-asked-for");
        let stubborn = FakeService::new().ignoring_stop().exit_when(&asked);
        let shutdown = FakeService::new().touch(&asked);

        let declared = Declared(vec![
            spec("mariadb")
                .args(arguments(&stubborn))
                .stop(StopBehaviour::Command {
                    program: FakeService::program(),
                    args: arguments(&shutdown),
                    grace: Millis::from_secs(10),
                })
                .build()
                .expect("a usable spec"),
        ]);
        let registry = registry(&paths, &store, Arc::new(declared));

        let graph = registry.graph().await.expect("one declared service");
        let plan = graph.start_plan([&service("mariadb")]).expect("a plan");
        let walk = registry.start(&graph, &plan).await;

        assert_eq!(walk.reached, vec![service("mariadb")], "{walk:?}");

        let stopping = graph.stop_plan([&service("mariadb")]).expect("a plan");
        let stopped = registry.stop(&stopping).await;

        assert_eq!(stopped.reached, vec![service("mariadb")], "{stopped:?}");
        assert!(
            asked.is_file(),
            "the stop command was never run: {} is not there",
            asked.display()
        );

        let (state, _) = row(&store, &service("mariadb")).await;
        assert_eq!(state, ServiceState::Stopped);

        let exit: Option<i64> =
            sqlx::query_scalar("SELECT last_exit_code FROM services WHERE id = ?")
                .bind("mariadb")
                .fetch_one(store.pool())
                .await
                .expect("the row");

        assert_eq!(
            exit,
            Some(0),
            "a service that left on its own inside the grace period exits cleanly; a killed one \
             records no code at all"
        );
    }

    /// **A configuration that changed under a running service reaches it without a restart** —
    /// roadmap task T31, and the mechanism every site on the machine depends on.
    ///
    /// The claim has two halves and the fixture proves both. The file the reload command creates is
    /// evidence that the command really ran — nothing else in this test writes it — and the pid is
    /// evidence of what the whole thing is *for*: it is the same process before and after, so the
    /// connections it was serving were never dropped.
    ///
    /// The first walk finds a changed rendering too, and is deliberately left in: nothing is running
    /// then, and a reload asked of a service that is down would be a command sent to an address
    /// nothing is listening on.
    #[tokio::test]
    async fn a_rendering_that_changed_reaches_the_process_already_running() {
        let (home, paths, store) = home(&["caddy"]).await;

        let reread = home.path().join("the-configuration-was-read-again");
        let reload = FakeService::new().touch(&reread);

        let declared = Rerendered(vec![
            spec("caddy")
                .reload(ReloadBehaviour::Command {
                    program: FakeService::program(),
                    args: arguments(&reload),
                    patience: Millis::from_secs(20),
                })
                .build()
                .expect("a usable spec"),
        ]);
        let registry = registry(&paths, &store, Arc::new(declared));

        let graph = registry.graph().await.expect("one declared service");
        let plan = graph.start_plan([&service("caddy")]).expect("a plan");
        let walk = registry.start(&graph, &plan).await;
        assert!(walk.failed.is_none(), "{walk:?}");

        let (_, before) = row(&store, &service("caddy")).await;
        assert!(
            !reread.exists(),
            "starting a service is not the same as reloading one"
        );

        // The walk that finds the rendering changed under a service that is up. What it does about
        // it is a permit left with the runner, so the wait below is for the runner's next turn.
        registry.graph().await.expect("a second walk");

        let deadline = std::time::Instant::now() + EVENTUALLY;
        while !reread.exists() && std::time::Instant::now() < deadline {
            tokio::time::sleep(Duration::from_millis(25)).await;
        }

        assert!(
            reread.is_file(),
            "the reload command was never run: {} is not there",
            reread.display()
        );

        let (state, after) = row(&store, &service("caddy")).await;
        assert_eq!(state, ServiceState::Running, "a reload is not a restart");
        assert_eq!(
            after, before,
            "the process was replaced rather than reloaded"
        );

        let stopping = graph.stop_plan([&service("caddy")]).expect("a plan");
        assert!(registry.stop(&stopping).await.failed.is_none());
    }

    /// **The budget bounds the sum, which is what nothing owned before T9a.**
    ///
    /// Two services each asking for a minute to stop in, and neither of them able to use it: the
    /// stop command runs and never returns, which is what a `mariadb-admin shutdown` against a
    /// server that has stopped answering looks like. Unbudgeted that is two minutes; the whole point
    /// is that somebody who typed `mix daemon stop` waits for what they were told rather than for
    /// the sum of what the specs happen to say.
    ///
    /// **`Command` and not `Signal`, which is what makes this the same test on all three systems.**
    /// Windows sends no request to stop at all (ADR 0008), so a `Signal` spec spends no grace period
    /// there and the assertion would pass without the clamp existing — a green test proving nothing
    /// on the one system whose console clock motivated the task. A stop command really runs
    /// everywhere, and the budget is visible as the deadline it is given.
    ///
    /// **The elapsed time is the assertion and the timeout is its floor.** Either the clamp applies
    /// or the walk sits in the first service's minute; the margin between the two is two orders of
    /// magnitude, so there is no third outcome for a loaded runner to land in.
    #[tokio::test]
    async fn a_shutdown_budget_bounds_every_service_left_to_stop_and_not_each_one() {
        let (_home, paths, store) = home(&["db", "web"]).await;

        // Asks, and never comes back. Nothing here creates the file the service would exit for, so
        // what ends each service is the kill after its grace period — whenever that turns out to be.
        let unanswering = FakeService::new();

        let stubborn = |id| {
            spec(id)
                .args(arguments(&FakeService::new().ignoring_stop()))
                .stop(StopBehaviour::Command {
                    program: FakeService::program(),
                    args: arguments(&unanswering),
                    grace: Millis::from_secs(60),
                })
                .build()
                .expect("a usable spec")
        };

        let declared = Declared(vec![stubborn("db"), stubborn("web")]);
        let registry = registry(&paths, &store, Arc::new(declared));

        let graph = registry.graph().await.expect("two declared services");
        let started = registry.start(&graph, &graph.start_order()).await;
        assert_eq!(started.reached.len(), 2, "{started:?}");

        registry.stopping_within(Duration::from_millis(500));

        let began = Instant::now();
        let stopped = tokio::time::timeout(EVENTUALLY, registry.stop(&graph.stop_order()))
            .await
            .expect("a stop that is bounded by the budget rather than by the specs");
        let took = began.elapsed();

        assert_eq!(stopped.reached.len(), 2, "both stopped: {stopped:?}");
        assert!(
            took < Duration::from_secs(10),
            "the two services took {took:?}, which is the specs' sixty seconds each rather than \
             the budget's half-second for both"
        );
    }

    /// The half of [`Budget`] that is a rule rather than arithmetic.
    #[test]
    fn a_second_shutdown_can_shorten_a_budget_and_never_extend_one() {
        let budget = Budget::default();

        assert_eq!(
            budget.remaining(),
            None,
            "the ordinary state of a daemon: no total to divide, and a spec's grace is the whole \
             answer"
        );

        budget.narrow_to(Instant::now() + Duration::from_secs(30));
        budget.narrow_to(Instant::now() + Duration::from_secs(2));

        let left = budget.remaining().expect("a shutdown is under way");
        assert!(
            left <= Duration::from_secs(2),
            "a console event arriving during a `daemon.shutdown` brings an OS clock with it, and \
             the daemon may not grant itself more of it: {left:?}"
        );

        // And the other direction, which is the one an accidental `max` would get wrong.
        budget.narrow_to(Instant::now() + Duration::from_secs(30));
        assert!(budget.remaining().expect("still under way") <= Duration::from_secs(2));
    }

    /// **A grace no clock can name is a shutdown that bounds nothing, and not one that panics.**
    ///
    /// `Instant + Duration` panics on overflow, and the signal half of a shutdown runs on the
    /// daemon's own main task: the unwinding would go straight past `Store::close` and leave the
    /// write-ahead log uncheckpointed — the next boot opening a database without its newest
    /// commits, because of a number somebody put in `config.toml`. `mixengine-core`'s loader clamps
    /// the setting and this is the second answer to the same number, which is the right number of
    /// answers for a panic that costs a database.
    ///
    /// **The two halves are asserted separately because they used to be one value.** A deadline
    /// that cannot be expressed leaves the budget where it was, which is exactly `Budget`'s `None`
    /// — no ceiling but each spec's — and that must not also mean "no shutdown is happening", which
    /// is what the refused start below is the observable half of.
    #[tokio::test]
    async fn a_shutdown_grace_no_clock_can_hold_bounds_nothing_rather_than_panicking() {
        let (_home, paths, store) = home(&["caddy"]).await;
        let declared = Declared(vec![spec("caddy").build().expect("a usable spec")]);
        let registry = registry(&paths, &store, Arc::new(declared));

        registry.stopping_within(Duration::MAX);

        assert_eq!(
            registry.budget.remaining(),
            None,
            "a deadline further away than this machine's clock can hold bounds nothing, and a \
             grace that large is a request for precisely that"
        );

        let graph = registry.graph().await.expect("one declared service");
        let plan = graph.start_plan([&service("caddy")]).expect("a plan");
        let walk = registry.start(&graph, &plan).await;

        assert!(
            matches!(&walk.failed, Some((id, None)) if id == &service("caddy")),
            "a shutdown whose budget could not be expressed is still a shutdown, and it starts \
             nothing: {walk:?}"
        );
        assert_eq!(
            row(&store, &service("caddy")).await.1,
            None,
            "a process was spawned by a daemon that is shutting down"
        );
    }

    /// **A stop command that reported failure does not throw away a service that stopped anyway** —
    /// `Runner::ended_meanwhile`, and the one thing a failed stop request must not lose.
    ///
    /// Running the command is a whole grace period's worth of time, and a server that took the
    /// instruction and exited inside that window has stopped exactly as it was asked to — whatever
    /// the program carrying the instruction then returned. `mariadb-admin shutdown` really does
    /// this: it can return non-zero for a server that has already gone by the time it looks again.
    /// Answering `None` there records no exit code at all and writes an ERROR about a kill that
    /// never happened, on a database that shut down cleanly.
    ///
    /// The fixture stages the window rather than hoping for it. The stop command creates the file
    /// first and only exits `3` four hundred milliseconds later, so the service — which polls for
    /// that file every twenty-five — is reliably gone before the failure is reported. The row is the
    /// assertion: `Some(0)` is a service that left on its own, and `None` is what a kill records.
    #[tokio::test]
    async fn a_service_that_stopped_while_its_stop_command_failed_keeps_its_exit_code() {
        let (home, paths, store) = home(&["mariadb"]).await;

        let asked = home.path().join("shutdown-was-asked-for");
        let stubborn = FakeService::new().ignoring_stop().exit_when(&asked);

        // Asks properly and then reports failure, which is the whole point: the service is already
        // going by the time the runner is told the request did not work.
        let shutdown = FakeService::new()
            .touch(&asked)
            .exit_after(400)
            .exit_code(3);

        let declared = Declared(vec![
            spec("mariadb")
                .args(arguments(&stubborn))
                .stop(StopBehaviour::Command {
                    program: FakeService::program(),
                    args: arguments(&shutdown),
                    grace: Millis::from_secs(10),
                })
                .build()
                .expect("a usable spec"),
        ]);
        let registry = registry(&paths, &store, Arc::new(declared));

        let graph = registry.graph().await.expect("one declared service");
        let plan = graph.start_plan([&service("mariadb")]).expect("a plan");

        assert_eq!(
            registry.start(&graph, &plan).await.reached,
            vec![service("mariadb")]
        );

        let stopping = graph.stop_plan([&service("mariadb")]).expect("a plan");
        let stopped = registry.stop(&stopping).await;

        assert_eq!(stopped.reached, vec![service("mariadb")], "{stopped:?}");
        assert!(
            asked.is_file(),
            "the stop command was never run: {} is not there",
            asked.display()
        );

        let (state, _) = row(&store, &service("mariadb")).await;
        assert_eq!(state, ServiceState::Stopped);

        let exit: Option<i64> =
            sqlx::query_scalar("SELECT last_exit_code FROM services WHERE id = ?")
                .bind("mariadb")
                .fetch_one(store.pool())
                .await
                .expect("the row");

        assert_eq!(
            exit,
            Some(0),
            "the service shut down cleanly while its stop command was still running, and the \
             command's own non-zero status threw that exit code away"
        );
    }

    /// **A stop command that cannot be run does not become a service that cannot be stopped.**
    ///
    /// The failure is loud in `daemon.log` — for a database it means a recovery on its next start —
    /// but the service still has to go, because the alternative is a port and a data directory held
    /// by a process nobody is supervising. Staged with a program that is not there, which is the
    /// ordinary shape of it: a spec written against a package that has since been uninstalled.
    #[tokio::test]
    async fn a_stop_command_that_cannot_be_run_still_leaves_the_service_stopped() {
        let (_home, paths, store) = home(&["mariadb"]).await;

        let stubborn = FakeService::new().ignoring_stop();
        let declared = Declared(vec![
            spec("mariadb")
                .args(arguments(&stubborn))
                .stop(StopBehaviour::Command {
                    program: FakeService::program().with_file_name("mixengine-no-such-program"),
                    args: Vec::new(),
                    grace: Millis::from_secs(10),
                })
                .build()
                .expect("a usable spec"),
        ]);
        let registry = registry(&paths, &store, Arc::new(declared));

        let graph = registry.graph().await.expect("one declared service");
        let plan = graph.start_plan([&service("mariadb")]).expect("a plan");

        assert_eq!(
            registry.start(&graph, &plan).await.reached,
            vec![service("mariadb")]
        );

        let stopping = graph.stop_plan([&service("mariadb")]).expect("a plan");
        let stopped = registry.stop(&stopping).await;

        assert_eq!(stopped.reached, vec![service("mariadb")], "{stopped:?}");

        let (state, pid) = row(&store, &service("mariadb")).await;
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

    /// **Two stops of one service are one stop with two callers**, and the second of them is
    /// answered by the stop that actually happened rather than by a race it lost.
    ///
    /// The shape it arrives in is ordinary: a `mix daemon stop` beside a `mix service stop`, or a
    /// GUI button pressed twice. The caller that got there second used to find the entry already
    /// taken out of the map, skip the wait the answer depends on, and read a row mid-stop — which
    /// says `Running` or `Stopping`, both of which `is_supervised` counts. So a service that was
    /// shutting down perfectly was reported as one that would not stop, `mix daemon stop` exited
    /// non-zero, and somebody went looking for a database that was fine.
    ///
    /// **The stop is staged to take time, or there is no second caller to be wrong.** A stop command
    /// really runs on all three systems — a `Signal` spec spends no grace on Windows at all
    /// (ADR 0008), so the window would close there, on one of the two systems this race is easiest
    /// to hit. Seven hundred milliseconds is far longer than the two database reads the second walk
    /// needs to reach its wrong answer, and far shorter than the grace the spec allows.
    #[tokio::test]
    async fn two_stops_of_one_service_are_both_answered_by_the_stop_that_happened() {
        let (home, paths, store) = home(&["mariadb"]).await;

        let asked = home.path().join("shutdown-was-asked-for");
        let stubborn = FakeService::new().ignoring_stop().exit_when(&asked);

        // Asks properly and stays there, which is what holds the stop open: the file is created as
        // the command starts and the command itself only ends seven hundred milliseconds later.
        let shutdown = FakeService::new().touch(&asked).exit_after(700);

        let declared = Declared(vec![
            spec("mariadb")
                .args(arguments(&stubborn))
                .stop(StopBehaviour::Command {
                    program: FakeService::program(),
                    args: arguments(&shutdown),
                    grace: Millis::from_secs(10),
                })
                .build()
                .expect("a usable spec"),
        ]);
        let registry = registry(&paths, &store, Arc::new(declared));

        let graph = registry.graph().await.expect("one declared service");
        let plan = graph.start_plan([&service("mariadb")]).expect("a plan");

        assert_eq!(
            registry.start(&graph, &plan).await.reached,
            vec![service("mariadb")]
        );

        let stopping = graph.stop_plan([&service("mariadb")]).expect("a plan");

        let (first, second) = tokio::time::timeout(EVENTUALLY, async {
            tokio::join!(registry.stop(&stopping), registry.stop(&stopping))
        })
        .await
        .expect("both stops were answered");

        assert_eq!(first.reached, vec![service("mariadb")], "{first:?}");
        assert!(first.failed.is_none(), "{first:?}");
        assert_eq!(
            second.reached,
            vec![service("mariadb")],
            "the second caller was answered by the stop the first one was performing: {second:?}"
        );
        assert!(
            second.failed.is_none(),
            "a service that stopped exactly as it was asked to was reported as one that would \
             not: {second:?}"
        );

        let (state, pid) = row(&store, &service("mariadb")).await;
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

    /// Roadmap task **T38**: the failure a user can do something about, in place of the one they
    /// cannot.
    ///
    /// The same shape as the test above — a service that never becomes ready — with one difference:
    /// the machine says a program MixEngine does not manage is on the port this service declared.
    /// "not ready within 750ms" is true of both and sends the reader to the service's own log,
    /// where a fixture that never announces itself has nothing to say.
    #[tokio::test]
    async fn a_service_whose_port_is_held_by_another_program_says_which_one() {
        let (_home, paths, store) = home(&["slow"]).await;

        let fake = FakeService::new().never_ready();
        let declared = Declared(vec![
            spec("slow")
                .args(arguments(&fake))
                .ports([3306])
                .ready(ReadyCheck::LogPattern {
                    regex: mixengine_testkit::service::READY_LINE.to_owned(),
                    timeout: Millis(750),
                })
                .build()
                .expect("a usable spec"),
        ]);
        let squatter = mixengine_platform::mock::Host::with_a_port_held(
            paths.root(),
            3306,
            mixengine_platform::PortHolder {
                pid: Some(4242),
                name: Some("mysqld.exe".to_owned()),
            },
        );
        let registry = registry_on(&paths, &store, Arc::new(declared), Arc::new(squatter));

        let graph = registry.graph().await.expect("one declared service");
        let plan = graph.start_plan([&service("slow")]).expect("a plan");

        let walk = registry.start(&graph, &plan).await;

        assert!(
            matches!(
                &walk.failed,
                Some((id, Some(StateReason::PortInUse { port, program: Some(program), .. })))
                    if id == &service("slow") && *port == 3306 && program == "mysqld.exe"
            ),
            "the reason names the program on the port, not the symptom: {walk:?}"
        );

        let (state, _) = row(&store, &service("slow")).await;
        assert_eq!(state, ServiceState::Failed);
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

    /// **One credential the keyring cannot answer for does not take the rest of the environment
    /// with it.**
    ///
    /// The adopted path is the only one that resolves an environment at stop time — see
    /// `Runner::where_commands_run` — and it is the one where a keyring is most likely to refuse: a
    /// machine that has been rebooted since the daemon that started this process. Dropping the whole
    /// environment for that would run `mariadb-admin shutdown` without the `HOME` the spec declares
    /// outright, so the stop that was meant to survive a locked keyring would fail a second time for
    /// a reason nobody chose.
    ///
    /// The mock store holds no credential, which is the same answer a locked one eventually gives.
    #[tokio::test]
    async fn an_adopted_service_stops_with_the_environment_entries_that_did_resolve() {
        let (home, paths, store) = home(&["mariadb"]).await;

        let asked = home.path().join("shutdown-was-asked-for");
        let dumped = home.path().join("stop-command-environment");
        let shutdown = FakeService::new().dump_env(&dumped).touch(&asked);

        let declared = Declared(vec![
            spec("mariadb")
                .env("MIXENGINE_DECLARED", "outright")
                .env_from_keyring("MIXENGINE_SECRET", "mariadb", "root")
                .stop(StopBehaviour::Command {
                    program: FakeService::program(),
                    args: arguments(&shutdown),
                    grace: Millis::from_secs(10),
                })
                .build()
                .expect("a usable spec"),
        ]);
        let registry = registry(&paths, &store, Arc::new(declared));

        // Deaf to every signal, so the clean exit below is evidence the command ran and nothing
        // else.
        let mut survivor = left_running_with(
            &store,
            &service("mariadb"),
            ServiceState::Running,
            FakeService::new().ignoring_stop().exit_when(&asked),
        )
        .await;

        let recovered = registry.recover().await;
        assert_eq!(recovered.adopted, vec![service("mariadb")], "{recovered:?}");

        let graph = registry.graph().await.expect("one declared service");
        let stopping = graph.stop_plan([&service("mariadb")]).expect("a plan");
        let stopped = tokio::time::timeout(EVENTUALLY, registry.stop(&stopping))
            .await
            .expect("the adopted service was stopped");

        assert_eq!(stopped.reached, vec![service("mariadb")], "{stopped:?}");
        assert!(
            gone(&mut survivor).await,
            "the adopted process outlived the stop of the service it belongs to"
        );

        let environment =
            std::fs::read_to_string(&dumped).expect("the stop command recorded its environment");

        assert!(
            environment
                .lines()
                .any(|line| line == "MIXENGINE_DECLARED=outright"),
            "the entry the spec states outright was thrown away with the one the keyring refused, \
             so the stop command ran without the environment it needs:\n{environment}"
        );
        assert!(
            !environment.contains("MIXENGINE_SECRET"),
            "an entry that did not resolve was passed anyway, which for a credential means an \
             empty password:\n{environment}"
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

    /// **A daemon on its way out starts nothing** — the other half of the same collision, and the
    /// one that leaves a process behind rather than a wrong sentence.
    ///
    /// `api/rpc.rs` runs every handler in a `tokio::spawn` nothing joins, so a `service.start` that
    /// lands while `mix daemon stop` is walking outlives the connection that asked for it. It used
    /// to walk past the place the shutdown had already been, register a runner into a map
    /// `shut_down` had already drained, and spawn a process nothing was left to wait for: killed by
    /// `Supervised`'s destructor as the runtime came down, leaving a row mid-write for the next
    /// boot's crash recovery to sort out.
    ///
    /// **Both routes into it are asserted, because they begin at different moments.** Granting the
    /// budget is the first thing `daemon.shutdown` does and the token is cancelled only after its
    /// ordered walk, so for the whole of that walk the token says nothing has happened; a signal is
    /// the other way round and may grant no budget this registry can hold. The refusal is the same
    /// either way, and so is the evidence for it: no row moved, and no process was spawned.
    #[tokio::test]
    async fn a_start_that_arrives_once_a_shutdown_has_begun_is_refused() {
        let (_home, paths, store) = home(&["caddy"]).await;
        let declared = Declared(vec![spec("caddy").build().expect("a usable spec")]);
        let granted = registry(&paths, &store, Arc::new(declared));

        let graph = granted.graph().await.expect("one declared service");
        let plan = graph.start_plan([&service("caddy")]).expect("a plan");

        // What `daemon.shutdown` does before it walks anything, and the whole of the window this
        // test is about: from here the daemon is going away, and the root token says nothing yet.
        granted.stopping_within(Duration::from_secs(5));

        let walk = tokio::time::timeout(EVENTUALLY, granted.start(&graph, &plan))
            .await
            .expect("the start was refused rather than waited on");

        assert!(walk.reached.is_empty(), "{walk:?}");
        assert!(
            matches!(&walk.failed, Some((id, None)) if id == &service("caddy")),
            "the walk names the service it would not start, with nothing to quote as a reason: \
             {walk:?}"
        );

        let (state, pid) = row(&store, &service("caddy")).await;
        assert_eq!(
            state,
            ServiceState::Stopped,
            "a service that was refused is where it was; a refusal is not a start that failed"
        );
        assert_eq!(
            pid, None,
            "a process was spawned by a daemon that is shutting down, and nothing is left to wait \
             for it"
        );
        assert!(
            lock(&granted.running).is_empty(),
            "a runner was registered into a map the shutdown is about to drain"
        );

        // The other route, and the one a signal takes: the root token is cancelled with no budget
        // ever granted, which is still this daemon going away.
        let signalled = registry(
            &paths,
            &store,
            Arc::new(Declared(vec![
                spec("caddy").build().expect("a usable spec"),
            ])),
        );
        signalled.shutdown.cancel();

        let walk = tokio::time::timeout(EVENTUALLY, signalled.start(&graph, &plan))
            .await
            .expect("the start was refused rather than waited on");

        assert!(
            matches!(&walk.failed, Some((id, None)) if id == &service("caddy")),
            "a cancelled root token is a daemon that starts nothing: {walk:?}"
        );
        assert!(lock(&signalled.running).is_empty(), "{walk:?}");
    }

    /// **A signal arriving mid-walk does not turn the rest of that walk into failures.**
    ///
    /// `daemon.shutdown` stops services in dependency order and a Ctrl-C in the middle of it breaks
    /// the accept loop, which cancels the root token and calls `shut_down` — so the whole `running`
    /// map is drained underneath a walk that is still stepping through it. The walk's next service
    /// then had no entry to wait for and read a row while the shutdown was still waiting for that
    /// very runner, reporting a stop that was going perfectly as one that failed. The budget half of
    /// this case was handled from the start; the walk half was not.
    ///
    /// Staged so the overlap is not a coincidence: `db` is given a stop three times longer than
    /// `web`'s, so the walk reaches it while the shutdown is unambiguously still inside it.
    #[tokio::test]
    async fn a_shutdown_draining_the_map_under_a_walk_does_not_fail_the_rest_of_it() {
        let (home, paths, store) = home(&["db", "web"]).await;

        // One file per service: the stop command creates it as it starts, which is both what the
        // service is waiting to exit for and what this test watches to know where the walk is.
        let asked = |id: &str| home.path().join(format!("{id}-was-asked-to-stop"));

        let slow_to_stop = |id: &str, millis: u64| {
            let stop = FakeService::new().touch(asked(id)).exit_after(millis);

            spec(id)
                .args(arguments(
                    &FakeService::new().ignoring_stop().exit_when(asked(id)),
                ))
                .stop(StopBehaviour::Command {
                    program: FakeService::program(),
                    args: arguments(&stop),
                    grace: Millis::from_secs(30),
                })
        };

        let declared = Declared(vec![
            slow_to_stop("db", 1_500).build().expect("a usable spec"),
            slow_to_stop("web", 400)
                .depends_on(service("db"))
                .build()
                .expect("a usable spec"),
        ]);
        let registry = Arc::new(registry(&paths, &store, Arc::new(declared)));

        let graph = registry.graph().await.expect("two declared services");
        assert_eq!(
            registry.start(&graph, &graph.start_order()).await.reached,
            vec![service("db"), service("web")]
        );

        let walking = {
            let registry = Arc::clone(&registry);
            let order = graph.stop_order();

            tokio::spawn(async move { registry.stop(&order).await })
        };

        // `web`'s stop command having started is the walk being inside its first service, which is
        // the only moment a signal can arrive in for this test to be about anything.
        let deadline = tokio::time::Instant::now() + EVENTUALLY;
        while !asked("web").is_file() {
            assert!(
                tokio::time::Instant::now() < deadline,
                "the walk never reached the first service's stop"
            );

            tokio::time::sleep(Duration::from_millis(25)).await;
        }

        // The signal: everything released at once, and the map emptied under the walk.
        registry.shutdown.cancel();
        tokio::time::timeout(EVENTUALLY, registry.shut_down())
            .await
            .expect("every runner finished");

        let walk = tokio::time::timeout(EVENTUALLY, walking)
            .await
            .expect("the walk was answered")
            .expect("the walk did not panic");

        assert_eq!(
            walk.reached,
            vec![service("web"), service("db")],
            "a service the shutdown was stopping was reported as one that would not stop: {walk:?}"
        );
        assert!(walk.failed.is_none(), "{walk:?}");

        assert_eq!(row(&store, &service("db")).await.0, ServiceState::Stopped);
        assert_eq!(row(&store, &service("web")).await.0, ServiceState::Stopped);
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
            // T30 will fail at, so it must not write the failure down in its own words. It is a
            // `mixengine_core::Error` and not an `anyhow::Error` since R8, which is what lets the
            // wire mapping read the code off it instead of downcasting to find one.
            why.to_string().contains("no such package"),
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
