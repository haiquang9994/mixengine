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
use mixengine_platform::process::{self, Adopted, CAN_ASK_TO_STOP, CAN_SIGNAL, Exit, Supervised};
use mixengine_proto::{
    EnvValue, Millis, ReloadBehaviour, ReloadSignal, RestartPolicy, ServiceId, ServiceSpec,
    ServiceState, StateReason, StopBehaviour,
};
use mixengine_supervisor::logs::Capture;
use mixengine_supervisor::{Decision, Health, Restarts, Surroundings, Verdict, ready};
use tokio::sync::{Notify, broadcast, watch};
use tokio::time::Instant;
use tokio_util::sync::CancellationToken;

use super::logs::ServiceLog;
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
///
/// **Generous is affordable only because a shutdown shortens it** — see [`Runner::within_budget`].
/// This is waited after the grace period rather than inside it, so an adopted survivor that took
/// its whole budget being asked politely and then five seconds more being watched would put the
/// walk five seconds past a total somebody was told. The constant is what a stop is allowed when
/// nothing is counting; the budget is what it gets when something is.
const GONE: Duration = Duration::from_secs(5);

/// The least of [`GONE`] a killed survivor is watched for, whatever a shutdown has left — roadmap
/// task **T9a**.
///
/// **Because zero is a wrong answer here and a right one everywhere else the budget reaches.** A
/// grace period of zero is a service killed at once and a log drain of zero is a tail nobody was
/// reading — both stated, both survivable. This poll is not a wait but a question, and one asked
/// with no window at all is asked microseconds after the kill: the kernel has not finished with the
/// process, [`Adopted::exited`] says `Ok(None)`, and a process that stopped exactly as it was told
/// to is reported as a survivor that will not go. What follows from that report is the whole of the
/// defect — the row keeps its `stopping`, [`Registry::stop_one`](super::Registry) reads it and says
/// the stop failed, and the walk stops there on the ordering rule, leaving every service after it in
/// the plan running.
///
/// Two polls of [`POLL`], which is what a delivered kill needs in the ordinary case and no more than
/// that: the floor exists to make the question answerable, not to wait out a process that is
/// genuinely stuck. One that is still there after it is reported as still there, exactly as before.
///
/// Drawn from [`Budget::reprieve`](super::Budget) rather than granted outright, so that a walk with
/// eight survivors in it costs the OS ceiling what a walk with one costs — see
/// [`Runner::seeing_it_go`].
const GONE_FLOOR: Duration = Duration::from_millis(100);

/// How long a service's environment is waited for before whatever needed it goes on without it.
///
/// A deadline rather than a patience, and the reason is the store it reads: a `Keyring` value goes
/// through the OS credential store, which on Linux is a D-Bus round trip to a daemon that may be
/// *prompting the user*. A locked keyring answers when somebody types a password, or never. Without
/// a ceiling here a `mix stop` of an adopted MariaDB, or a whole daemon shutting down, waits for
/// that forever.
///
/// **Both paths that resolve one are bounded by this, and they differ only in what they do
/// afterwards.** A stop goes on with the entries the spec states outright
/// ([`Runner::where_commands_run`]), because a `mariadb-admin shutdown` short one entry still beats
/// a kill; a start refuses ([`Runner::environment`]), because carrying on would be the empty
/// password that function exists to prevent.
///
/// The start path is the one that could hang a whole daemon, and did until it was bounded here.
/// The read happens on a blocking task and holds no cancellation point, so a service sitting in
/// [`Runner::attempt`] when a shutdown arrives never reaches its token: the cancellation releases
/// nothing, [`Registry::stop_one`](super::Registry) waits on a task that is not coming back, and
/// the answer to `daemon.shutdown` — with the root token and the daemon behind it — waits with it.
/// A `--detach`ed daemon on Windows has no console event to fall back on either.
///
/// Generous against an unlocked store, which answers in milliseconds, and short against a person who
/// is not at the machine.
///
/// **And shortened again on the stop path, because three seconds is not always three seconds this
/// daemon has** — see [`Runner::within_budget`]. Adopted services resolve this *while stopping*, one
/// read each, after which the walk still has to ask them to stop; a Windows shutdown is working
/// inside two and a half seconds altogether. So the constant is what the read is allowed when
/// nothing is counting, and the budget is what it gets when something is — the same arrangement
/// [`GONE`] and [`FLUSH`] are in, and for the same reason.
const ENVIRONMENT: Duration = Duration::from_secs(3);

/// How long the last lines of a stopped service are waited for.
///
/// Bounded because end of file is not the process exiting but the *last holder of the pipe*
/// exiting — see [`Capture::finish`], which explains why an unbounded wait here would hang the
/// supervisor at the one moment it has something to report.
///
/// **Really paid, and paid per service after that service's grace period is spent** — which is what
/// makes it a shutdown's business and not only a stop's. Killing the group is not killing a
/// grandchild that left it, and a leftover holding a copy of the service's stdout keeps the pipe
/// open for the whole of this; eight services doing that is sixteen seconds after the last grace
/// period. So a shutdown shortens it too — see [`Runner::within_budget`].
const FLUSH: Duration = Duration::from_secs(2);

/// What a walk of the spec's environment came back with: every entry that resolved, and the error of
/// each that did not.
///
/// Both halves, because the two callers of [`Runner::walk_environment`] disagree about what a
/// failure means — a start refuses anything less than all of it, a stop command runs with what there
/// is.
///
/// **`anyhow::Error` here on purpose, unlike [`SpecSource`](super::SpecSource)** — R8 changed one of
/// them and deliberately not the other. Nothing downcasts this or maps it to a code: an entry that
/// will not resolve is logged and the service is given `StateReason::SpawnFailed`, so what this type
/// has to carry is a sentence for `daemon.log`. Three unrelated things produce one — a
/// `mixengine_platform` keyring failure, a credential the store simply does not hold, and the
/// blocking task not finishing — and the `.context("the environment entry …")` that names which
/// variable it was is the part a reader needs. A typed enum here would be three variants that exist
/// to be `Display`ed and one place where a `From` has to be written.
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

/// What the memory watchdog last concluded about one service — roadmap task **T71a**.
///
/// Carried as [`None`] where the service is under its ceiling *or* was not measured at all: the two
/// are the same fact to a reader of state, and the difference between them is spent inside
/// `services::watchdog` where it decides a count.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct Over {
    /// What it was measured holding, in bytes: the finished minute's average.
    pub(crate) rss_bytes: u64,

    /// The ceiling it was judged against, in megabytes.
    pub(crate) limit_mb: u32,
}

/// One state from two independent readings — roadmap task **T71a**.
///
/// **The runner is the only writer of the `Running`/`Degraded` edge, and this is why.** Health and
/// size are measured by different things at different rates: a health verdict comes from a probe
/// this loop makes, and a memory verdict from a watchdog reading minutes the sampler finished. Two
/// writers would overwrite each other in both directions — the next healthy probe clearing a warning
/// about a service still over its ceiling, and a watchdog seeing memory drop erasing a genuine
/// `Unhealthy`.
///
/// **Illness is reported ahead of size when both hold.** A service failing its probe needs attention
/// whatever it weighs, and telling somebody that their database is *large* while it is refusing
/// connections would send them to the wrong problem. It is still restarted for its size at the third
/// minute, with `OverMemory` on that transition — the one place this design says two things about
/// one episode, and it is deliberate: a leak is most likely to be the cause exactly when a service
/// has also stopped answering.
fn fold(healthy: bool, over: Option<Over>) -> (ServiceState, StateReason) {
    match (healthy, over) {
        (false, _) => (ServiceState::Degraded, StateReason::Unhealthy),

        (true, Some(over)) => (
            ServiceState::Degraded,
            StateReason::OverMemory {
                rss_bytes: over.rss_bytes,
                limit_mb: over.limit_mb,
            },
        ),

        (true, None) => (ServiceState::Running, StateReason::Healthy),
    }
}

/// Whether a fold is a change worth writing down.
///
/// **The state, and deliberately not the reason.** [`ServiceState::can_become`] has no self-loops, so
/// a move to the state a service is already in is an `IllegalTransition` — which `record` logs an
/// `error!` for, once per minute, for as long as the service stays over its ceiling.
///
/// The cost is a reason that can lag: a service that recovers its health while still over its
/// ceiling goes on reading `degraded — unhealthy`, although what is now wrong with it is its size.
/// The alternative is publishing a `Running` it never was, to every client watching, in order to
/// correct one word.
fn worth_recording(current: ServiceState, next: ServiceState) -> bool {
    current != next
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

    /// Where this service's output is put for clients to read — roadmap task **T16b**.
    ///
    /// **Outlives every [`Capture`] this runner makes**, which is the whole reason it is a field
    /// here rather than something built beside each one: a capture belongs to one run of the
    /// process and dies with it, and a `mix service logs -f` open across a crash, a backoff and a
    /// restart must not end three times. Each attempt attaches its capture to this — see
    /// [`Runner::relay`] — and what a connected client holds is a subscription to *this*.
    pub(super) log: Arc<ServiceLog>,

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

    /// Notified when this service's generated configuration has been rewritten under it — roadmap
    /// task **T31**, and the second edge from the registry into a runner.
    ///
    /// A [`Notify`] for [`Runner::asked_to_start`]'s reasons and one of its own: a walk that renders
    /// while this runner is inside a health probe leaves a permit rather than a message, so the
    /// reload happens at the top of the next turn — and two walks that both find a change before it
    /// gets there collapse into the one reload that was needed, because what a reload delivers is
    /// the file as it is now and not the edit that produced it.
    pub(super) asked_to_reload: Arc<Notify>,

    /// A new set of ceilings, pushed by [`Registry::set_limits`](super::Registry::set_limits).
    ///
    /// Carries the value rather than only waking this task, unlike its two neighbours — see the
    /// sender's own documentation for why a limit is the message and not a pointer to one.
    pub(super) limits_asked: watch::Receiver<mixengine_proto::ResourceLimits>,

    /// Why the stop this runner is about to perform is happening — roadmap task **T69**.
    ///
    /// [`None`], which is nearly always, means somebody asked: `service.stop`, a walk, a shutdown.
    /// The idle sweeper sets it before it cancels, so a service that stopped because nothing was
    /// using it says so on the transition rather than claiming a person asked for it.
    ///
    /// A `watch` beside [`Runner::limits_asked`] and for its reason — the value *is* the message —
    /// running the same way. Read at the moment the stop begins rather than subscribed to, because
    /// there is exactly one moment it matters.
    pub(super) stopping_because: watch::Receiver<Option<StateReason>>,

    /// What the memory watchdog last concluded about this service — roadmap task **T71a**.
    ///
    /// Read by `supervise`, folded with its own health verdict, and never acted on alone. See
    /// [`fold`].
    pub(super) over_memory: watch::Receiver<Option<Over>>,

    /// What is left of the whole daemon's shutdown, when one is under way — roadmap task **T9a**.
    ///
    /// Read at every wait a stop is about to make, and it can only shorten them: the seconds a
    /// service needs to flush are a fact about the service, and the total a user waits for a daemon
    /// they told to stop is a different fact that nothing else owns. See [`Budget`](super::Budget).
    ///
    /// **Every wait and not only the grace period**, which is what makes it a bound on the total.
    /// The grace period is where the spec has something to say, so it goes through
    /// [`Runner::grace_for`]; the kill's log drain, the watch for an adopted survivor and the
    /// environment an adopted service's stop command has to be run in are constants this module
    /// chose, and they go through [`Runner::within_budget`]. A stop is the sum of the four, and a
    /// budget that clamped only the first was a promise the walk did not keep — as was one that
    /// clamped the two that come after the grace period and not the one that comes before it.
    pub(super) budget: super::Budget,

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

    /// The environment read a previous start stopped waiting for, while it is still going.
    ///
    /// **Kept so that the next start joins it instead of beginning a second one.** Giving up on the
    /// read is [`ENVIRONMENT`]'s whole purpose and it cannot abort what it gave up on: the walk runs
    /// on a blocking task, `spawn_blocking` has no cancellation, and dropping the handle only stops
    /// *this* task waiting — the thread stays parked in the keyring until the OS answers. One of
    /// those is the price the bound was worth paying; one per attempt is not, and a start is
    /// attempted again by every restart the policy grants and by every `service.start` a client
    /// sends. Against a keyring that never answers, that filled tokio's blocking pool — at which
    /// point [`Runner::kill`], which is also a blocking task, could no longer run, and a daemon that
    /// had bounded a hanging read to keep stopping services could not stop any.
    ///
    /// So the leak is bounded at one per runner, which is the same bound the stop path already has,
    /// and a keyring that unlocks half an hour later is answered by the attempt that finds this
    /// finished rather than by one that starts the read over.
    ///
    /// [`None`] whenever no read is outstanding, which is every start that got its answer.
    pub(super) reading: Option<tokio::task::JoinHandle<Resolved>>,

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
    /// Put everything this capture reads onto the log clients are connected to — roadmap task
    /// **T16b**.
    ///
    /// **A relay and not a fourth sink inside the capture**, deliberately. The reader threads run
    /// outside the runtime and their one obligation is to drain a pipe the service blocks on; making
    /// them also reach into a daemon-side structure would put the daemon's locks on the path of
    /// every line a service prints, where a moment's contention stalls the process itself. What this
    /// task does instead costs the reader threads nothing — the send they already make has one more
    /// subscriber — and everything after that happens on the runtime, where being slow is only slow.
    ///
    /// **It ends on its own**, when the last sender inside the capture is dropped: that is the
    /// capture going away with this run of the process, or the last reader thread reaching end of
    /// file. Neither needs the runner to remember it, which is what keeps this out of every stop
    /// path — a restart makes a new capture and a new relay, and the log they both write into is the
    /// same one the client has been reading since before either existed.
    ///
    /// A relay that falls behind is a gap in the client's stream and says so. It should not happen:
    /// this loop does nothing but move a line from one channel to another, so falling behind means
    /// the runtime itself is starved — but a hole nobody mentions is exactly what
    /// [`LogFrame::Gap`](mixengine_proto::LogFrame::Gap) exists to prevent, and the alternative is a
    /// log panel quietly missing the lines that explain a failure.
    ///
    /// **What the capture already holds is taken here, and not inside the task** — roadmap task
    /// **T16c**. `Capture::start` puts the reader threads on the pipes before it returns, and the
    /// task below is spawned rather than run, so a service can print before the runtime first polls
    /// it. A subscription delivers nothing that was sent before it existed, so every line in that
    /// window used to reach `current.log` and never the ring — permanently, not until something
    /// caught up, and those are a start's first lines. [`Capture::read`] hands over the tail and the
    /// subscription under one lock, so what is taken here is complete and holds no line twice; both
    /// are moved into the task, which records the tail before it pumps anything.
    fn relay(&self, capture: &Capture) {
        let (already_said, mut lines) = capture.read();
        let log = Arc::clone(&self.log);
        let service = self.spec.id().clone();

        tokio::spawn(async move {
            for line in already_said {
                log.record(line);
            }

            loop {
                match lines.recv().await {
                    Ok(line) => log.record(line),

                    Err(broadcast::error::RecvError::Lagged(missed)) => {
                        tracing::warn!(
                            service = service.as_str(),
                            missed,
                            "this daemon fell behind a service's output; the lines it lost are \
                             reported as a gap to everything reading its log"
                        );

                        log.missed(missed);
                    }

                    // Every sender is gone: this run of the process is over and its pipes have been
                    // read to the end.
                    Err(broadcast::error::RecvError::Closed) => return,
                }
            }
        });
    }

    async fn attempt(&mut self, restarts: &mut Restarts) -> After {
        let env = match self.environment().await {
            Ok(env) => env,

            // A credential the spec names and the keyring does not hold, or one it will not answer
            // for inside `ENVIRONMENT`. The process was never started, which is exactly what
            // `SpawnFailed` says — and the entry is named in `daemon.log` and never in the event,
            // because the event is rendered in a GUI.
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

        let mut supervised = match process::spawn_supervised(
            self.spec.program(),
            &args,
            self.spec.cwd(),
            &env,
            &super::limits::from_proto(self.spec.limits()),
        ) {
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

        // Cloned rather than borrowed out of `self`, because the wait below also holds `&mut
        // supervised` and the two would be a borrow of `self` and a borrow inside it at once. Two
        // owned fields, cloned once per start, against a ready check that may run for a minute.
        let place = self
            .surroundings
            .clone()
            .expect("the surroundings were just set");

        // Before anything waits on the process: a pipe nobody drains stops the service writing to
        // it, and a ready check that matches a log pattern has nothing to match against until this
        // exists.
        let capture = Capture::start(
            &mut supervised,
            self.spec.id().as_str(),
            self.spec.logs(),
            Some(&self.directory),
        );

        self.relay(&capture);

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

            outcome = ready::wait(self.spec.ready(), &mut supervised, &capture, &place) => Some(outcome),
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
                // Asked before the process is reaped, and only about a failure: an exit of zero is
                // a service that did what it was told, and nothing about a port could improve on
                // that sentence.
                let conflict = if exit.is_success() {
                    None
                } else {
                    self.port_conflict(Some(supervised.pid())).await
                };

                let capture = self.kill(supervised, capture).await;
                let decision = restarts.ended(&exit, std::time::Instant::now(), &capture);

                self.after_exit(decision, exit.code(), conflict).await
            }

            // Running, and never going to be usable. Killed rather than left: the next attempt would
            // collide with the port and the data directory this one is holding.
            Ok(ready::Ready::TimedOut) => {
                let after = self.spec.ready().timeout();
                // Asked while the process is still up, so that its own pid is there to be
                // recognised: a service that bound its port and then failed its ready check is
                // holding that port itself, and is not in conflict with anybody.
                let conflict = self.port_conflict(Some(supervised.pid())).await;

                self.kill(supervised, capture).await;
                self.record_exit(None).await;

                self.give_up(conflict.unwrap_or(StateReason::ReadyTimeout { after }))
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

        // **The two halves of the state this loop owns** — roadmap task T71a. `healthy` starts true
        // because a service with no health check is one nothing has ever said is ill, and `state`
        // starts `Running` because `supervise` is entered from the `Running`/`Ready` transition
        // immediately above it.
        let mut healthy = true;
        let mut state = ServiceState::Running;

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

                // Ahead of the timer and behind the stop. A configuration that has already been
                // written is one the next liveness poll can wait for, and a daemon on its way out
                // should not spend a reload's patience on a service it is about to stop.
                () = self.asked_to_reload.notified() => {
                    self.reload(&place, &supervised).await;

                    continue;
                }

                // Beside the reload and for its reason: something has been written down and the
                // running process has not been told yet. Cheaper than a reload — no patience, no
                // signal, no command — because both mechanisms accept a rewrite with processes
                // already inside them.
                Ok(()) = self.limits_asked.changed() => {
                    let asked = *self.limits_asked.borrow_and_update();

                    if let Err(error) = supervised.set_limits(&super::limits::from_proto(asked)) {
                        // **Logged, and the service is left running.** A mechanism that refused a
                        // value is not a service that has stopped working, and a daemon that killed
                        // a healthy database because a cap could not be written would be doing more
                        // damage than the uncapped service ever could.
                        tracing::warn!(
                            service = self.spec.id().as_str(),
                            error = %error,
                            "cannot apply this service's new limits to the running process"
                        );
                    }

                    continue;
                }

                // **Nothing is done here, deliberately.** This arm exists to wake the loop; what
                // to do about the new value is the same decision a health probe reaches, and it is
                // made once, below, from both inputs.
                Ok(()) = self.over_memory.changed() => {}

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

                    // No port diagnosis here, and the difference from a failed *start* is the whole
                    // reason: this process had the port. Whatever ended it, nobody took it away —
                    // and if something claims it before the restart, that start is where it will be
                    // met and named.
                    return self.after_exit(decision, exit.code(), None).await;
                }

                Ok(None) => {}

                // The OS will not say. Nothing to decide on, and the next tick asks again.
                Err(error) => tracing::warn!(
                    service = self.spec.id().as_str(),
                    error = %error,
                    "cannot tell whether this service is still running"
                ),
            }

            // **Folded before the health guard below, not inside it** — roadmap task T71a. A
            // service whose recipe declares no `HealthCheck` never reaches that branch, and a fold
            // living there would watch such a service's ceiling and never say a word about it.
            let over = *self.over_memory.borrow_and_update();
            let folded = fold(healthy, over);

            if worth_recording(state, folded.0) && self.move_to(folded.0, folded.1).await {
                state = folded.0;
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
                // **Recorded, not applied** — roadmap task T71a. The verdict changes what this loop
                // believes about the service's health; what state that adds up to is the fold's, at
                // the top of the next turn, because size has a say in it too.
                Ok(Some(Verdict::Degraded)) => {
                    healthy = false;
                }

                Ok(Some(Verdict::Recovered)) => {
                    // The backoff, not the failure history: a service that recovers between crashes
                    // is still crashing, and `Restarts` is what remembers that.
                    restarts.recovered();
                    healthy = true;
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

    /// Hand the running process the configuration that was rewritten under it — roadmap task
    /// **T31**.
    ///
    /// `caddy reload`, and later `nginx -s reload`: a program shipped with the service, run in the
    /// service's own surroundings, that tells the process listening to re-read its file. Everything
    /// that decides *whether* to do this is the registry's — see
    /// [`Registry::hand_over`](super::Registry::hand_over) — and everything about *how* is the
    /// spec's, which is why this function is as short as it is.
    ///
    /// **Nothing here changes the service's state.** A reload that worked is not news: the process
    /// was running before it and is running after it, and a state change would announce a restart
    /// that did not happen. A reload that failed is not news about the *service* either — it is
    /// still up, still serving, still on the configuration it had — so it is a line in `daemon.log`
    /// rather than a degradation. What a user has then is a file on disk that the running process is
    /// not using, which the next start resolves and which `mix doctor` owes a sentence (T47).
    async fn reload(&self, place: &Surroundings, supervised: &Supervised) {
        match self.spec.reload() {
            Some(ReloadBehaviour::Command {
                program,
                args,
                patience,
            }) => {
                self.reload_by_command(place, program, args, *patience)
                    .await
            }

            Some(ReloadBehaviour::Signal { signal, patience }) => {
                self.reload_by_signal(supervised, *signal, *patience).await;
            }

            // Asked of a service that has no way to be asked. Said at `warn` because the
            // alternative is a person editing an override, watching the daemon accept it, and
            // finding the old value still in force with nothing anywhere saying why.
            _ => tracing::warn!(
                service = self.spec.id().as_str(),
                "this service's configuration changed and it has no reload, so the running process \
                 is still using the previous one; it will be read at the next start"
            ),
        }
    }

    /// The [`ReloadBehaviour::Command`] half, which is what T31 wrote — unchanged, moved.
    async fn reload_by_command(
        &self,
        place: &Surroundings,
        program: &std::path::Path,
        args: &[String],
        patience: Millis,
    ) {
        match place.run(program, args, patience.as_duration()).await {
            Ok(ran) if ran.succeeded() => tracing::info!(
                service = self.spec.id().as_str(),
                "this service re-read its configuration without being restarted"
            ),

            // The two ways it did not work, kept apart because they send a reader to different
            // places: the program ran and refused, or it could not be run at all.
            Ok(ran) => tracing::warn!(
                service = self.spec.id().as_str(),
                program = %program.display(),
                detail = ran.complaint().unwrap_or("it failed without saying why"),
                "this service refused the configuration it was asked to re-read; the process is \
                 still running the previous one"
            ),

            Err(error) => tracing::warn!(
                service = self.spec.id().as_str(),
                program = %program.display(),
                %error,
                "the command that hands this service its configuration could not be run; the \
                 process is still running the previous one"
            ),
        }
    }

    /// The [`ReloadBehaviour::Signal`] half — roadmap task **T32**.
    ///
    /// **[`CAN_SIGNAL`] is read before anything is waited for**, which is [`CAN_ASK_TO_STOP`]'s
    /// lesson applied one method along: a system with no signals should say so at the moment it is
    /// asked, not after a patience spent on a delivery nobody attempted. A recipe on such a system
    /// returns no reload at all, so this arm is the belt to that braces.
    ///
    /// **The patience is a wait and not a check.** A signal has no exit status: the daemon cannot
    /// learn from the OS whether php-fpm liked the file it was told to re-read, only that the signal
    /// was delivered. What the wait buys is that the next configuration change does not arrive on
    /// top of a pool that is still cycling its workers — so it is spent *after* the delivery has
    /// been reported, not before.
    async fn reload_by_signal(
        &self,
        supervised: &Supervised,
        signal: ReloadSignal,
        patience: Millis,
    ) {
        if !CAN_SIGNAL {
            tracing::warn!(
                service = self.spec.id().as_str(),
                "this service is reloaded by signal and this system has none, so the running \
                 process is still using its previous configuration; it will be read at the next \
                 start"
            );

            return;
        }

        let which = match signal {
            ReloadSignal::Hup => process::Signal::Hup,
            ReloadSignal::Usr1 => process::Signal::Usr1,
            ReloadSignal::Usr2 => process::Signal::Usr2,

            // The wire enum is `#[non_exhaustive]` and this crate is downstream of it, so a variant
            // added there without a mapping here is reported rather than silently dropped.
            other => {
                tracing::warn!(
                    service = self.spec.id().as_str(),
                    signal = ?other,
                    "this build cannot send the signal this service is reloaded with"
                );

                return;
            }
        };

        match supervised.signal(which) {
            Ok(()) => {
                // Said before the wait and not after it. The signal is delivered by the time
                // `signal` returns, so the news is true now; logging it on the far side of the
                // patience would stamp `daemon.log` ten seconds late and keep a reader waiting for
                // an answer the daemon already has.
                tracing::info!(
                    service = self.spec.id().as_str(),
                    signal = ?signal,
                    "this service was signalled to re-read its configuration"
                );

                tokio::time::sleep(patience.as_duration()).await;
            }

            Err(error) => tracing::warn!(
                service = self.spec.id().as_str(),
                signal = ?signal,
                %error,
                "this service could not be signalled to re-read its configuration; the process is \
                 still running the previous one"
            ),
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

                    // As the supervised loop above: a service that was up held its own port.
                    return self.after_exit(decision, exit.code(), None).await;
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
        let reason = self.stopping_because();

        self.move_to(ServiceState::Stopping, reason.clone()).await;

        // `GONE` and not `FLUSH`: what this path does after the request is watch a process it is not
        // the parent of leave the table, and that is the tail its grace period has to leave room for.
        self.ask_to_stop(&mut adopted, GONE).await;

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

        // **Capped here and not inside [`gone`], which is the one place the budget does not reach.**
        // That function is shared with `Registry::discard`, which polls a survivor during crash
        // recovery — before the first client is served, and so before any shutdown exists to have a
        // budget — so a parameter threaded through it would be one that caller has nothing honest to
        // pass. Dropping the poll says exactly what its own deadline passing says: it is not known to
        // have gone, so the row keeps its `stopping` and its pid for the next daemon. The inner
        // `GONE` still bounds the caller that has no budget, and `seeing_it_go` is what stops a whole
        // walk being five seconds longer than it was allowed for one survivor that will not go —
        // without shortening the question to nothing, which would answer it wrongly.
        let went = tokio::time::timeout(self.seeing_it_go(), gone(self.spec.id(), &adopted))
            .await
            .unwrap_or(false);

        if !went {
            tracing::error!(
                service = self.spec.id().as_str(),
                pid = adopted.pid(),
                "this adopted process did not go when it was stopped; its row is left saying so, \
                 for the next daemon to stop it again"
            );

            return After::Done;
        }

        self.record_exit(None).await;

        self.move_to(ServiceState::Stopped, reason).await;

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
        let reason = self.stopping_because();

        self.move_to(ServiceState::Stopping, reason.clone()).await;

        // `FLUSH` is what the kill below still needs after this returns, and saying so here is what
        // keeps the whole of this stop inside a shutdown's budget rather than the polite half of it.
        let exit = self.ask_to_stop(&mut supervised, FLUSH).await;

        // Whatever the polite half achieved, the group is killed afterwards: the leader exiting is
        // not the workers exiting, and a php-fpm pool left holding the port is what the next start
        // collides with.
        self.kill(supervised, capture).await;
        self.record_exit(exit.and_then(|exit| exit.code())).await;

        self.move_to(ServiceState::Stopped, reason).await;

        After::Done
    }

    /// Ask the group to leave on its own, and wait as long as the spec says. `None` if it did not.
    ///
    /// Written against [`Stoppable`] rather than against [`Supervised`] because a service this
    /// daemon adopted is stopped by the same `StopBehaviour` as one it started: the spec is the
    /// user's statement about what the *service* needs in order to shut down cleanly, and it does
    /// not become less true because the daemon that spawned the process was killed. What differs
    /// between the two is only what the request travels on, which is the trait's whole surface.
    ///
    /// `tail` is how long the caller's own kill still needs after this returns — [`FLUSH`] for the
    /// supervised path, [`GONE`] for the adopted one. It is a parameter and not a constant here
    /// because those two really are different amounts of work, and reserving the larger of them for
    /// both would take five seconds off every supervised stop to pay for a drain that costs two. See
    /// [`Runner::grace_for`], which is where it is spent.
    async fn ask_to_stop(&self, process: &mut dyn Stoppable, tail: Duration) -> Option<Exit> {
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

                self.grace_for(*grace, tail)
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

                // Clamped here rather than above for the same reason the clock starts here: what a
                // whole-daemon shutdown has left is smaller after a three-second keyring read than
                // it was before one, and the command is what should be given the difference.
                let allowed = self.grace_for(*grace, tail);

                // **Zero is a kill, not a command run with no patience.** `run` would spawn the
                // program, find the deadline already past on the next poll and kill it — a process
                // started, two pipes made and a `mariadb-admin` given no chance to say anything, at
                // the one moment the daemon has run out of the time it was given. Nothing is left to
                // hear an answer in, so the question is not asked; `ended_meanwhile` is still read,
                // because a service that went by itself must not be recorded as one that was killed.
                if allowed.is_zero() {
                    tracing::info!(
                        service = self.spec.id().as_str(),
                        program = %program.display(),
                        "nothing is left of the shutdown budget; this service's stop command is not \
                         run and it is killed at once, which may leave it to recover on its next \
                         start"
                    );

                    return self.ended_meanwhile(process);
                }

                match place.run(program, args, allowed).await {
                    Ok(ran) if ran.succeeded() => allowed,

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

        let deadline = began + grace;

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
                    grace = ?grace,
                    "this service did not stop when asked; killing it"
                );

                return None;
            }

            tokio::time::sleep(POLL).await;
        }
    }

    /// Why the stop about to happen is happening.
    ///
    /// [`StateReason::Requested`] unless somebody set [`Runner::stopping_because`] first, which is
    /// the idle sweeper and nothing else. Read rather than subscribed to: there is one moment this
    /// matters, and it is the moment a stop begins.
    fn stopping_because(&self) -> StateReason {
        self.stopping_because
            .borrow()
            .clone()
            .unwrap_or(StateReason::Requested)
    }

    /// How long this service may actually take to stop: what its spec asks for, or what a shutdown
    /// has left once the kill after it is paid for, whichever is less — roadmap task **T9a**.
    ///
    /// **The two numbers answer different questions and neither is wrong.** A spec's grace period is
    /// what *this* service needs in order to shut down cleanly, and it stays true whether the daemon
    /// is stopping one service or twelve; the budget is what somebody who typed `mix daemon stop`
    /// is waiting for, and until this task nothing owned it — twelve services each allowed ten
    /// seconds was two minutes nobody had agreed to.
    ///
    /// **`tail` is what makes the budget a bound on the total rather than on the polite half of it.**
    /// Asking a service to stop is not the last thing a stop does: the kill after it drains the
    /// service's pipes ([`FLUSH`]), or watches a survivor leave the process table ([`GONE`]), and
    /// both are per service and both come *after* the grace period. A grace period clamped to the
    /// whole of what is left therefore overran the budget by that tail once per service — three
    /// services on Windows was a budget of two and a half seconds and a walk of eight and a half,
    /// against an OS ceiling of five. So what is kept back here is exactly
    /// [`CEILING_RESERVE`](crate::CEILING_RESERVE) one level down: a shutdown subtracts from an OS
    /// ceiling what it still has to do after the last service stops, and a service subtracts from
    /// the budget what it still has to do after its own request to stop.
    ///
    /// Outside a shutdown there is no total to divide and the spec is the whole answer, which is what
    /// [`Budget::remaining`](super::Budget::remaining)'s [`None`] means — **`tail` is subtracted from
    /// nothing there**, because a `mix service stop mariadb` on a running machine has no total for
    /// the kill to have to fit inside. Inside one, a service reached after the budget is spent gets
    /// zero and is killed at once — stated in its row and in the log line above, because for a
    /// database that is a recovery on its next start rather than a clean stop, and the honest report
    /// is the whole of what makes a shorter budget somebody's choice instead of a surprise.
    fn grace_for(&self, asked: mixengine_proto::Millis, tail: Duration) -> Duration {
        let asked = asked.as_duration();

        let Some(left) = self.budget.remaining() else {
            return asked;
        };

        let allowed = left.saturating_sub(tail);

        if allowed >= asked {
            return asked;
        }

        tracing::info!(
            service = self.spec.id().as_str(),
            asked = ?asked,
            left = ?left,
            tail = ?tail,
            allowed = ?allowed,
            "this service is being stopped inside a shutdown with less time left than its spec asks \
             for; it gets what is left, less what killing it afterwards still needs"
        );

        allowed
    }

    /// What is left of a shutdown's budget for something that wants `wants` — roadmap task **T9a**.
    ///
    /// The other half of [`Runner::grace_for`], for the parts of a stop that are not a grace period
    /// at all: the log drain in [`Runner::kill`], the watch in [`Runner::stop_adopted`], and the
    /// environment read in [`Runner::where_commands_run`] that an adopted service's stop command
    /// waits on before any of it begins. Those are constants rather than anything the spec asks for,
    /// and a constant charged outside the budget is a constant the budget does not bound — which is
    /// the whole of what `.claude/architecture/daemon-and-ipc.md` promises when it says the budget
    /// bounds the total.
    ///
    /// [`None`] is the ordinary non-shutdown state and means the constant entire: nothing is
    /// counting, and there is no total for it to have to fit inside.
    fn within_budget(&self, wants: Duration) -> Duration {
        self.budget
            .remaining()
            .map_or(wants, |left| wants.min(left))
    }

    /// How long the poll after the kill in [`Runner::stop_adopted`] gets — roadmap task **T9a**.
    ///
    /// [`Runner::within_budget`] with a floor under it, and the floor is the whole of what this adds:
    /// see [`GONE_FLOOR`] for why zero is the one answer this particular wait cannot be given, and
    /// [`Budget::reprieve`](super::Budget) for where the floor is paid from. Everything above the
    /// floor is the budget's answer unchanged — a shutdown with seconds left still watches for
    /// [`GONE`], and one with fifty milliseconds left still watches for fifty rather than for a
    /// hundred, because a budget that has not run out is a budget that is still being kept.
    ///
    /// **What the reprieve running out means is what a spent budget always means here**: the poll is
    /// short, the survivor is not known to have gone, and the row says so. The floor makes the
    /// question answerable in the ordinary case; it does not make the answer up.
    fn seeing_it_go(&self) -> Duration {
        let allowed = self.within_budget(GONE);

        // `None` is the ordinary non-shutdown state, where `allowed` is `GONE` entire and this `max`
        // changes nothing. Inside a shutdown it is what is left of the one window the whole walk
        // shares, so the floor is granted to as many survivors as fit inside it and no more.
        let floor = self
            .budget
            .reprieve()
            .map_or(GONE_FLOOR, |left| GONE_FLOOR.min(left));

        allowed.max(floor)
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
    /// Who else is on a port this service was going to listen on — roadmap task **T38**.
    ///
    /// On a blocking task because every implementation of the capability is a synchronous read of
    /// the machine, and one of them starts a process to do it (see `mixengine_platform`'s macOS
    /// module). A join that fails is treated as no diagnosis, which is the same answer every other
    /// failure to ask gets.
    async fn port_conflict(&self, ours: Option<u32>) -> Option<StateReason> {
        if self.spec.ports().is_empty() {
            return None;
        }

        let host = Arc::clone(&self.host);
        let ports = self.spec.ports().to_vec();

        tokio::task::spawn_blocking(move || super::ports::conflict(host.as_ref(), &ports, ours))
            .await
            .ok()
            .flatten()
    }

    async fn after_exit(
        &self,
        decision: Decision,
        code: Option<i32>,
        conflict: Option<StateReason>,
    ) -> After {
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

            // **A conflict replaces an exit and never a crash loop.** `StateReason::Exited` says
            // only what the OS said, and a port held by somebody else is the same fact with the
            // useful half attached; `StateReason::CrashLoop` carries its own count and the lines
            // the service printed, which is more than this could add.
            Decision::GiveUp { reason } => {
                let reason = match (reason, conflict) {
                    (StateReason::Exited { .. }, Some(conflict)) => conflict,
                    (reason, _) => reason,
                };

                self.give_up(reason).await
            }

            // The `Restarting` is what answers a walk that is waiting on this service, which is why
            // a failure to persist it ends the task: a runner that went on restarting from a row
            // nobody could read would leave that walk with nothing to wait for.
            Decision::Restart { after, attempt } => {
                if !self
                    .move_to(
                        ServiceState::Restarting,
                        conflict.unwrap_or(StateReason::Exited { code }),
                    )
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
                let reason = self.stopping_because();

                self.move_to(ServiceState::Stopping, reason.clone()).await;
                self.move_to(ServiceState::Stopped, reason).await;

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
    ///
    /// **The drain is inside a shutdown's budget and not after it** — see [`Runner::within_budget`].
    /// This runs once per service and after that service's grace period is spent, and it is really
    /// reached: killing the group does not reach a grandchild that left it, and one of those still
    /// holding a copy of the service's stdout keeps the pipe open for the whole of [`FLUSH`]. Left
    /// uncapped, a walk of three services would spend six seconds past a total somebody was told —
    /// which on Windows is the daemon being terminated in the middle of `Store::close`, the one
    /// outcome [`CEILING_RESERVE`](crate::CEILING_RESERVE) exists to prevent.
    async fn kill(&self, supervised: Supervised, mut capture: Capture) -> Capture {
        let service = self.spec.id().clone();
        let flush = self.within_budget(FLUSH);

        tokio::task::spawn_blocking(move || {
            // Its `Drop` is the kill: the group goes, whether or not the leader had already exited.
            drop(supervised);

            if !capture.finish(flush) {
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
    ///
    /// **Bounded by whichever of that and the shutdown's remainder is smaller**, because "only ever
    /// taken while a service is being stopped" is the whole reason: this is a wait a stop makes, it
    /// is per service, and it is spent *before* the grace period the budget was already dividing. A
    /// daemon stopping three adopted services against a locked keyring spent nine seconds here that
    /// no budget had agreed to, which on Windows is the process being terminated in the middle of
    /// the WAL checkpoint it stopped its services in order to make. Zero is a real answer and means
    /// the literals, which is what the arm below already does with a read that did not finish.
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

        // **Inside the shutdown's budget and not beside it** — see `Runner::within_budget`, which is
        // why this is not `ENVIRONMENT` outright. It is the one wait a stop makes *before* its grace
        // period, and it is paid by every service this daemon adopted: those are the runners a spawn
        // never filled `surroundings` in for, so a whole-daemon shutdown resolves their environment
        // here, once each, three seconds at a time against a Windows budget of two and a half.
        let env = match tokio::time::timeout(
            self.within_budget(ENVIRONMENT),
            self.walk_environment(OnFailure::Record),
        )
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
    ///
    /// **An environment that will not *arrive* is an environment that is not there**, which is why
    /// [`ENVIRONMENT`] bounds this exactly as it bounds the stop path's read. The answer is the same
    /// one a missing credential gets and for the same reason: what the spec asked for is not
    /// available, so the process is not started. It reaches the user as
    /// [`StateReason::SpawnFailed`], which says precisely that, and the reason it did not arrive is
    /// named in `daemon.log` where a locked keyring can be acted on.
    ///
    /// Giving up on the read leaves the blocking task where it is, still waiting on the store — see
    /// [`Runner::where_commands_run`], which pays the same price for the same reason. What it does
    /// not do is make a whole daemon's shutdown wait with it.
    ///
    /// **And the next attempt joins that task rather than starting another**, which is the whole of
    /// why this takes `&mut self` — see [`Runner::reading`]. A start is retried, so a price the stop
    /// path pays once per service was being paid once per attempt here, and against a keyring that
    /// never answers that is tokio's blocking pool filling up with reads nobody is waiting for.
    async fn environment(&mut self) -> anyhow::Result<BTreeMap<String, String>> {
        // Taken rather than borrowed: whichever arm below runs, this handle is either finished with
        // or put back, and a `take` is what stops a `?` in between leaving a live read behind.
        let mut reading = match self.reading.take() {
            Some(reading) => reading,
            None => self.spawn_environment(OnFailure::Stop),
        };

        // `&mut` so that the timeout drops the borrow and not the handle — the read this gives up on
        // is the one the next attempt is going to wait for.
        let walked = tokio::time::timeout(ENVIRONMENT, &mut reading).await;

        let (env, failed) = match walked {
            Ok(joined) => joined?,

            // Worded to stand on its own: the caller logs this with `%error`, which prints the top
            // of the chain and not the chain.
            Err(_) => {
                self.reading = Some(reading);

                anyhow::bail!(
                    "no answer within {ENVIRONMENT:?}; a locked OS keyring is the usual reason"
                )
            }
        };

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
    ///
    /// The awaiting half of [`Runner::spawn_environment`], for the caller that has nowhere to keep a
    /// read it gave up on: [`Runner::where_commands_run`] is reached once per stop of one service,
    /// where the start path is reached once per attempt.
    async fn walk_environment(&self, on_failure: OnFailure) -> anyhow::Result<Resolved> {
        Ok(self.spawn_environment(on_failure).await?)
    }

    /// Start the walk, without waiting for it.
    ///
    /// Split out from [`Runner::walk_environment`] for the one caller that has to hold the handle
    /// across a timeout it may not survive — see [`Runner::reading`]. The task itself is the same
    /// either way, and so is what abandoning it costs.
    fn spawn_environment(&self, on_failure: OnFailure) -> tokio::task::JoinHandle<Resolved> {
        let named: Vec<(String, EnvValue)> = self
            .spec
            .env()
            .iter()
            .map(|(name, value)| (name.clone(), value.clone()))
            .collect();

        let host = Arc::clone(&self.host);

        tokio::task::spawn_blocking(move || {
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
/// **The ceiling here is the one a caller with no shutdown to answer to gets**, which is crash
/// recovery's case exactly: `Registry::discard` runs before the first client is served, so there is
/// no budget for it to consult and [`GONE`] entire is the right answer. The runner's own call is
/// shortened by whatever a shutdown has left — done at that call site rather than by a parameter
/// here, for the reason given there.
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

#[cfg(test)]
mod tests {
    use mixengine_core::Paths;
    use mixengine_proto::{LogSubject, Millis};
    use mixengine_testkit::FakeService;

    use super::super::Budget;
    use super::super::fixture::{arguments, home, spec};
    use super::*;

    /// A verdict about a service holding more than it was allowed.
    fn over() -> Over {
        Over {
            rss_bytes: 600 * 1024 * 1024,
            limit_mb: 512,
        }
    }

    /// The four rows of the T71a design's D4 table, as a table.
    #[test]
    fn health_and_memory_fold_into_one_state() {
        assert_eq!(
            fold(true, None),
            (ServiceState::Running, StateReason::Healthy)
        );

        assert_eq!(
            fold(true, Some(over())),
            (
                ServiceState::Degraded,
                StateReason::OverMemory {
                    rss_bytes: 600 * 1024 * 1024,
                    limit_mb: 512
                }
            )
        );

        assert_eq!(
            fold(false, None),
            (ServiceState::Degraded, StateReason::Unhealthy)
        );

        assert_eq!(
            fold(false, Some(over())),
            (ServiceState::Degraded, StateReason::Unhealthy),
            "illness is the more urgent sentence to put in front of a person"
        );
    }

    /// The bug this design was rewritten to avoid.
    ///
    /// A healthy probe must not clear a memory warning: the service is still over its ceiling, and
    /// the runner owns this edge alone precisely so that two inputs cannot overwrite each other.
    #[test]
    fn a_healthy_probe_does_not_clear_an_over_memory_warning() {
        assert_eq!(
            fold(true, Some(over())).0,
            ServiceState::Degraded,
            "health recovering says nothing about size"
        );
    }

    /// Nothing is written when the fold reaches the state the service is already in.
    ///
    /// `can_become` has no self-loops, so a second identical move is an `IllegalTransition` and one
    /// `error!` per minute in `daemon.log`.
    #[test]
    fn an_unchanged_state_produces_no_transition() {
        assert!(!worth_recording(
            ServiceState::Degraded,
            fold(false, None).0
        ));

        assert!(worth_recording(ServiceState::Degraded, fold(true, None).0));
    }

    /// A service that recovers its health while still over its ceiling stays where it is.
    ///
    /// **The state is right and the reason is stale**, and that is the deliberate trade: reaching
    /// `Degraded`/`OverMemory` from `Degraded`/`Unhealthy` is not a legal transition, and the only
    /// route to it would be a `Running` this service never was. Named here so that the day somebody
    /// reads `unhealthy` on a service whose probe is passing, this test says why.
    #[test]
    fn a_reason_may_lag_while_the_state_does_not_change() {
        assert!(!worth_recording(
            ServiceState::Degraded,
            fold(true, Some(over())).0
        ));
    }

    /// The margin every wall-clock assertion here allows itself.
    ///
    /// Only ever weighed against gaps of whole seconds — the one that matters most below is a
    /// second against six — so nothing but the clamp under test failing can close one. Generous
    /// because every number here contains a process spawn, and starting one on a loaded Windows
    /// runner is measured in hundreds of milliseconds.
    const SLACK: Duration = Duration::from_secs(2);

    /// How long a test waits before reading a file a stop command would have left behind.
    ///
    /// Far longer than a `fakeservice --touch` needs — it writes the file before it does anything
    /// else at all — so a run that got as far as starting has had every chance to leave its trace.
    const SETTLE: Duration = Duration::from_millis(500);

    /// The budget these tests hand a runner that is being shut down.
    ///
    /// Bigger than [`GONE`] by a second, which is the whole arrangement: the tail these stops
    /// reserve is `GONE`, so a grace period clamped correctly is one second and one clamped to the
    /// whole of what is left is six. There is no third answer for a slow runner to land on.
    const BUDGET: Duration = Duration::from_secs(6);

    /// A process that takes every request to stop and acts on none of them.
    ///
    /// The whole of what these tests need from a [`Stoppable`]: what is under test is how long the
    /// runner is *willing* to wait, and a process that ended would answer the wait instead of
    /// letting it reach its deadline. Deliberately not a real one — the thing being timed is a
    /// budget, and a second child would put its own spawn inside every measurement.
    struct NeverStops;

    impl Stoppable for NeverStops {
        fn ask_to_stop(&self) -> mixengine_platform::Result<()> {
            Ok(())
        }

        fn exited(&mut self) -> mixengine_platform::Result<Option<Exit>> {
            Ok(None)
        }
    }

    /// A spec whose stop command is a `fakeservice` that runs until something kills it.
    ///
    /// **`Command` and not `Signal`, for the reason T9a's own budget test gives**: Windows sends no
    /// request to stop at all (ADR 0008), so a `Signal` spec spends no grace period there and every
    /// assertion below would pass without the clamp existing — green on the one system whose
    /// console clock motivated the task.
    fn stopped_by_a_command(id: &str, grace: Millis, command: &FakeService) -> ServiceSpec {
        spec(id)
            .stop(StopBehaviour::Command {
                program: FakeService::program(),
                args: arguments(command),
                grace,
            })
            .build()
            .expect("a usable spec")
    }

    /// A runner over `spec`, supervising nothing.
    ///
    /// Enough for [`Runner::ask_to_stop`], which reaches neither a process nor the row: what it
    /// touches is the spec, the budget and the place its commands run.
    ///
    /// `surroundings` is set, and that is what keeps these tests about the arithmetic. Left [`None`]
    /// it would send [`Runner::where_commands_run`] through a keyring walk on a blocking task, and
    /// what the clock would then be measuring is a mock host rather than a clamp.
    fn runner(spec: ServiceSpec, paths: &Paths, store: &Store) -> Runner {
        let place = Surroundings::new(spec.cwd(), BTreeMap::new());
        let directory = paths.service_logs(spec.id());
        let (readiness, _) = watch::channel(Readiness::Deciding);

        Runner {
            spec,
            store: store.clone(),
            directory,
            log: crate::services::logs::Logs::new().reading(&LogSubject::Service {
                id: ServiceId::parse("fake").expect("a usable id"),
            }),
            host: Arc::new(mixengine_platform::mock::Host::with_home(paths.root())),
            events: Events::new(),
            cancel: CancellationToken::new(),
            asked_to_start: Arc::new(Notify::new()),
            asked_to_reload: Arc::new(Notify::new()),
            limits_asked: watch::channel(mixengine_proto::ResourceLimits::default()).1,
            stopping_because: watch::channel(None).1,
            over_memory: watch::channel(None).1,
            budget: Budget::default(),
            surroundings: Some(place),
            reading: None,
            readiness,
        }
    }

    /// A runner whose commands have nowhere to run *yet*, over a keyring that will not answer.
    ///
    /// **The adopted-service case, and the only one that pays for this read on the stop path.** Every
    /// runner is built with `surroundings: None` and only a spawn fills it in, so a service this
    /// daemon adopted from a previous one — never spawned here — resolves its environment when it is
    /// asked to stop, which is inside a shutdown.
    fn adopted_runner(
        spec: ServiceSpec,
        paths: &Paths,
        store: &Store,
        keyring_takes: Duration,
    ) -> Runner {
        let directory = paths.service_logs(spec.id());
        let (readiness, _) = watch::channel(Readiness::Deciding);

        Runner {
            spec,
            store: store.clone(),
            directory,
            log: crate::services::logs::Logs::new().reading(&LogSubject::Service {
                id: ServiceId::parse("fake").expect("a usable id"),
            }),
            host: Arc::new(mixengine_platform::mock::Host::stalling_on_the_keyring(
                paths.root(),
                keyring_takes,
            )),
            events: Events::new(),
            cancel: CancellationToken::new(),
            asked_to_start: Arc::new(Notify::new()),
            asked_to_reload: Arc::new(Notify::new()),
            limits_asked: watch::channel(mixengine_proto::ResourceLimits::default()).1,
            stopping_because: watch::channel(None).1,
            over_memory: watch::channel(None).1,
            budget: Budget::default(),
            surroundings: None,
            reading: None,
            readiness,
        }
    }

    /// **The environment a stop reads is inside the shutdown's budget, like every other wait a stop
    /// makes** — the third of them, and the one the first implementation left out.
    ///
    /// `FLUSH` and `GONE` are clamped by [`Runner::within_budget`] because a constant charged after
    /// the budget is a constant the budget does not bound. This read is the same thing and was not:
    /// it is paid per service, on the stop path, by every adopted service, and `ENVIRONMENT` is
    /// three seconds — so on Windows, where the whole signalled budget is two and a half and the
    /// slack over the WAL checkpoint is one, a single locked keyring put the daemon past the clock
    /// the OS is running and the checkpoint is what it lost.
    ///
    /// A locked store rather than a missing one, because they are different failures: a missing
    /// store answers at once and no deadline is ever reached.
    #[tokio::test]
    async fn an_environment_a_stop_waits_for_is_bounded_by_what_the_shutdown_has_left() {
        let (_home, paths, store) = home(&["mariadb"]).await;

        // Longer than `ENVIRONMENT` so that what ends this read is always a deadline and never the
        // store — whichever deadline it turns out to be is then the whole of what is measured — and
        // no longer than it has to be, because dropping a runtime waits for the blocking task this
        // sleep is on and every second past the deadline is a second added to the test.
        let never = ENVIRONMENT + Duration::from_secs(1);
        let left = Duration::from_millis(500);

        let runner = adopted_runner(
            spec("mariadb")
                .env("MARIADB_DATA", "/var/lib/mysql")
                .env_from_keyring("MARIADB_ROOT_PASSWORD", "mixengine", "mariadb@main/root")
                .build()
                .expect("a usable spec"),
            &paths,
            &store,
            never,
        );

        runner.budget.narrow_to(Instant::now() + left);

        let began = Instant::now();
        let place = runner.where_commands_run().await;
        let took = began.elapsed();

        assert!(
            took < Duration::from_secs(2),
            "the read was given {took:?} by a shutdown with {left:?} left to spend; unclamped it is \
             {ENVIRONMENT:?}, once per service, after which the budget is already over"
        );

        // What it does about giving up is unchanged and is the point of giving up at all: the stop
        // goes on with the entries the spec states outright, because a `mariadb-admin shutdown`
        // short one entry still beats a kill. `Surroundings` redacts its values and prints its keys,
        // which is exactly as much as this needs to say.
        let resolved = format!("{place:?}");
        assert!(
            resolved.contains("MARIADB_DATA"),
            "the entries the spec states outright are what a stop carries on with: {resolved}"
        );
        assert!(
            !resolved.contains("MARIADB_ROOT_PASSWORD"),
            "the keyring never answered, so there is nothing under this name to have carried: \
             {resolved}"
        );
    }

    /// The same read, outside a shutdown: nothing is counting, so it gets its whole deadline.
    ///
    /// The mistake this guards against is a clamp applied unconditionally, which would quietly take
    /// a shutdown's arithmetic into a machine that is not shutting down — a health probe's
    /// environment, resolved once per life of a process, cut short by a budget that does not exist.
    #[tokio::test]
    async fn without_a_budget_the_environment_read_keeps_its_own_deadline() {
        let (_home, paths, store) = home(&["mariadb"]).await;

        let runner = adopted_runner(
            spec("mariadb")
                .env_from_keyring("MARIADB_ROOT_PASSWORD", "mixengine", "mariadb@main/root")
                .build()
                .expect("a usable spec"),
            &paths,
            &store,
            ENVIRONMENT + Duration::from_secs(1),
        );

        assert_eq!(
            runner.budget.remaining(),
            None,
            "no shutdown is under way, which is what this test is about"
        );

        let began = Instant::now();
        let _ = runner.where_commands_run().await;
        let took = began.elapsed();

        assert!(
            took >= ENVIRONMENT,
            "the read was cut off after {took:?} with nothing counting against it"
        );
        assert!(
            took < ENVIRONMENT + SLACK,
            "the read ran past its own deadline: {took:?}"
        );
    }

    /// **A grace period the budget clamps still leaves room for the kill after it** — the half of
    /// T9a's promise the first implementation did not keep.
    ///
    /// The budget is a deadline for the whole walk, and asking a service to stop is not the whole of
    /// stopping one: the kill afterwards drains the service's pipes, or watches a survivor leave the
    /// process table, per service and *after* the grace period. A clamp to the whole of what is left
    /// therefore overran the total by that tail every time — which on Windows is the daemon being
    /// terminated in the middle of the WAL checkpoint it stopped its services in order to make.
    ///
    /// So the assertion is on the sum and not on the wait: what the ask took plus what its caller
    /// still has to spend has to fit in the budget. The stop command never returns, which is what a
    /// `mariadb-admin shutdown` against a server that has stopped answering looks like, so the ask
    /// spends exactly what it was allowed and the number is the clamp itself.
    #[tokio::test]
    async fn a_grace_the_budget_clamps_leaves_room_for_the_kill_that_follows_it() {
        let (_home, paths, store) = home(&["mariadb"]).await;

        // Asks, and never comes back. Nothing here creates a file it would exit for.
        let unanswering = FakeService::new();
        let runner = runner(
            stopped_by_a_command("mariadb", Millis::from_secs(60), &unanswering),
            &paths,
            &store,
        );

        runner.budget.narrow_to(Instant::now() + BUDGET);

        let began = Instant::now();
        let exit = runner.ask_to_stop(&mut NeverStops, GONE).await;
        let took = began.elapsed();

        assert!(
            exit.is_none(),
            "the service ignored the request, so the caller is told to kill it"
        );

        assert!(
            took + GONE <= BUDGET + SLACK,
            "the ask took {took:?} and the kill after it still needs {GONE:?}, which is more than \
             the {BUDGET:?} the whole stop was given: the grace period was clamped to what was \
             left instead of to what was left minus the tail"
        );

        // The other direction, and the one an over-eager reserve would get wrong: what is left
        // *minus* the tail is still a second of asking, and a stop that skipped it would be a
        // database killed with a shutdown command available and unused.
        assert!(
            took >= Duration::from_millis(500),
            "the stop command was given {took:?}, which is not the second the budget had room for"
        );
    }

    /// **A budget with nothing left kills at once rather than starting a fresh anything.**
    ///
    /// Zero is a real answer and not a missing one — see [`Budget::remaining`] — and what it has to
    /// mean is the kill. A grace period that began again from the spec's number here would be a
    /// service handed sixty more seconds by the very arithmetic that exists to take them away, at
    /// the moment the daemon has none left to give.
    ///
    /// **What is asserted is the decision, and the reason that is not also the spawn is worth
    /// writing down.** The code skips running the command as well, because a `run` with no patience
    /// would start a process and two pipes and kill them on the next poll for an answer nothing can
    /// wait to hear — but no test can watch that happen: the child is dropped microseconds after it
    /// is created, long before the program can load, read its arguments and write anything down, so
    /// the file below is missing under the spawn every bit as reliably as under the skip. It is
    /// still watched, because it can only ever accuse truthfully, and it is the assertions above it
    /// that carry the test.
    #[tokio::test]
    async fn a_budget_that_is_already_spent_kills_at_once_and_starts_no_fresh_grace_period() {
        let (home, paths, store) = home(&["mariadb"]).await;

        let marker = home.path().join("the-stop-command-ran");
        let announcing = FakeService::new().touch(&marker);
        let runner = runner(
            stopped_by_a_command("mariadb", Millis::from_secs(60), &announcing),
            &paths,
            &store,
        );

        // Already elapsed: the walk reached this service with the whole of its budget spent on the
        // ones before it, which is the case T9a's budget exists to bound.
        runner.budget.narrow_to(Instant::now());

        assert_eq!(
            runner.grace_for(Millis::from_secs(60), FLUSH),
            Duration::ZERO,
            "a service reached after the budget ran out gets nothing, and nothing is the kill"
        );

        let began = Instant::now();
        let exit = runner.ask_to_stop(&mut NeverStops, FLUSH).await;
        let took = began.elapsed();

        assert!(exit.is_none(), "nothing was asked, so nothing stopped");
        assert!(
            took < SLACK,
            "a stop with nothing left to spend spent {took:?}; it is meant to fall straight through \
             to the kill"
        );

        tokio::time::sleep(SETTLE).await;

        assert!(
            !marker.exists(),
            "{} was created, so the stop command was run with a grace period of zero",
            marker.display()
        );
    }

    /// **The ordinary state of a daemon is unchanged: no total to divide, and the spec is the whole
    /// answer.**
    ///
    /// A `mix service stop mariadb` on a running machine is not a shutdown. Nothing is counting
    /// against it, so there is no deadline for the kill afterwards to have to fit inside and the
    /// tail is subtracted from nothing — the mistake this guards against is a reserve applied
    /// unconditionally, which would quietly take five seconds off every grace period on the machine
    /// and leave a database being killed mid-flush outside a shutdown entirely.
    #[tokio::test]
    async fn without_a_budget_a_specs_grace_period_is_honoured_in_full() {
        let (_home, paths, store) = home(&["mariadb"]).await;

        let grace = Millis::from_secs(1);
        let unanswering = FakeService::new();
        let runner = runner(
            stopped_by_a_command("mariadb", grace, &unanswering),
            &paths,
            &store,
        );

        assert_eq!(
            runner.budget.remaining(),
            None,
            "no shutdown is under way, which is what this test is about"
        );
        assert_eq!(
            runner.grace_for(grace, GONE),
            grace.as_duration(),
            "the tail is subtracted from a total, and outside a shutdown there is no total"
        );

        let began = Instant::now();
        let exit = runner.ask_to_stop(&mut NeverStops, GONE).await;
        let took = began.elapsed();

        assert!(exit.is_none(), "the service ignored the request");
        assert!(
            took >= grace.as_duration(),
            "the spec asked for {grace} and the stop command was given {took:?}"
        );
        assert!(
            took <= grace.as_duration() + SLACK,
            "the spec asked for {grace} and the stop took {took:?}"
        );
    }

    /// **A spent budget still leaves long enough to see a killed survivor go** — the one wait for
    /// which zero is a wrong answer rather than a short one.
    ///
    /// Every other clamp in this file is honest at zero: a grace period of nothing is a service
    /// killed at once, a log drain of nothing is a tail nobody was reading. The poll after the kill
    /// in [`Runner::stop_adopted`] is a *question*, and asked with no window at all it is asked
    /// microseconds after the kill — the kernel has not finished with the process, so a survivor
    /// that stopped exactly as it was told to is written down as one that would not go. What follows
    /// is the defect this pins: the row keeps its `stopping`,
    /// [`Registry::stop_one`](super::super::Registry) reads it and reports the stop as failed, and
    /// the walk stops there on the ordering rule with every service after it still running.
    ///
    /// **And the floor is a window the walk shares, not an allowance per service**, which is the
    /// half that keeps it inside the OS ceiling — asserted here by letting the window pass, after
    /// which a second survivor gets what a spent budget always gave.
    #[tokio::test]
    async fn a_spent_budget_still_leaves_long_enough_to_see_a_killed_survivor_go() {
        let (_home, paths, store) = home(&["mariadb", "redis"]).await;

        let first = runner(
            spec("mariadb").build().expect("a usable spec"),
            &paths,
            &store,
        );
        let mut second = runner(
            spec("redis").build().expect("a usable spec"),
            &paths,
            &store,
        );

        // One shutdown, two services in its plan — which is what makes the window below a shared
        // one rather than two.
        second.budget = first.budget.clone();

        assert_eq!(
            first.seeing_it_go(),
            GONE,
            "nothing is counting yet, so the constant is the whole answer"
        );

        // Already elapsed: the walk reached these two with the whole of its budget spent on the ones
        // before them.
        first.budget.narrow_to(Instant::now());

        assert!(
            first.within_budget(GONE).is_zero(),
            "the budget is spent, which is the case this test is about"
        );
        assert_eq!(
            first.seeing_it_go(),
            GONE_FLOOR,
            "a killed survivor was given no time at all to leave the process table, so the poll \
             after the kill can only answer that it had not"
        );
        assert_eq!(
            second.seeing_it_go(),
            GONE_FLOOR,
            "both survivors are inside the one window the shutdown opened"
        );

        tokio::time::sleep(crate::CONFIRMATION_REPRIEVE + SETTLE).await;

        assert_eq!(
            second.seeing_it_go(),
            Duration::ZERO,
            "the reprieve is a moment the whole walk shares and it has passed; a floor granted \
             again here would be one per service, which is the unbounded term the OS ceiling \
             cannot contain"
        );
    }

    /// **A start that gave up on a keyring read joins it next time instead of starting another.**
    ///
    /// [`ENVIRONMENT`] bounds the read and cannot abort it: the walk is a blocking task,
    /// `spawn_blocking` has no cancellation, and dropping the handle only stops *this* task waiting.
    /// One parked thread is what the bound was worth paying; one per attempt is not — a start is
    /// retried by every restart the policy grants and by every `service.start` a client sends, so a
    /// keyring that never answers filled tokio's blocking pool, at which point [`Runner::kill`] is
    /// also a blocking task and the daemon could no longer stop anything.
    ///
    /// **Asserted by the clock rather than by counting threads**, which is what the mock keyring can
    /// support: the store answers a second after the first attempt has given up, so an attempt that
    /// joined the read in flight hears it a second later and one that started a fresh read waits the
    /// whole of `ENVIRONMENT` again. The gap between those two is the test.
    #[tokio::test]
    async fn a_start_that_gave_up_on_a_keyring_read_does_not_start_a_second_one() {
        let (_home, paths, store) = home(&["mariadb"]).await;

        // Longer than `ENVIRONMENT`, so the first attempt always ends at its deadline; and only just,
        // so the answer lands while the second attempt is waiting for it.
        let answers_after = ENVIRONMENT + Duration::from_secs(1);

        let mut runner = adopted_runner(
            spec("mariadb")
                .env_from_keyring("MARIADB_ROOT_PASSWORD", "mixengine", "mariadb@main/root")
                .build()
                .expect("a usable spec"),
            &paths,
            &store,
            answers_after,
        );

        let first = runner
            .environment()
            .await
            .expect_err("the keyring has not answered yet");

        assert!(
            format!("{first}").contains("no answer within"),
            "the first attempt ended at its own deadline, which is what leaves a read behind: \
             {first}"
        );
        assert!(
            runner.reading.is_some(),
            "the read this attempt gave up on is the one the next attempt has to join"
        );

        let began = Instant::now();
        let second = runner
            .environment()
            .await
            .expect_err("the mock keyring holds no credential under that name");
        let took = began.elapsed();

        assert!(
            took < ENVIRONMENT,
            "the second attempt waited {took:?}, which is a read of its own: the one already in \
             flight answers {:?} after the first attempt gave up",
            answers_after - ENVIRONMENT
        );
        assert!(
            format!("{second}").contains("MARIADB_ROOT_PASSWORD"),
            "what came back is the store's answer about the entry, not a second deadline: {second}"
        );
        assert!(
            runner.reading.is_none(),
            "the read finished, so there is nothing left for a third attempt to join"
        );
    }
}
