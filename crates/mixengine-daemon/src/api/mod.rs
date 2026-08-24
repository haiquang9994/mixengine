//! The API server: JSON-RPC over HTTP/1.1 on the local endpoint T7 opened.
//!
//! Three layers, one per file, and the split is the same one the architecture draws:
//!
//! - [`http`] is the transport. Routing, body limits, timeouts, spans, and the connection loop.
//! - [`rpc`] is the protocol. Batches, notifications, method dispatch, panic containment.
//! - [`events`] is the stream. `GET /events`, a bounded broadcast, and what a slow client is told.
//! - [`logs`] is the other stream. `GET /logs/{id}`, which carries one service's output and is
//!   separate from [`events`] on purpose — see
//!   `.claude/decisions/0009-logs-travel-on-their-own-stream.md`.
//!
//! Nothing in here is business logic — `.claude/CLAUDE.md` puts that in `mixengine-core` — and the
//! handlers are the proof: each one turns state the daemon already holds into a `mixengine-proto`
//! type and does nothing else.

// Reachable by name rather than only through the re-export below, because a `Frame` is what a
// subscriber receives and the registry's tests assert on the ones its transitions produce.
mod create;
pub(crate) mod events;
mod http;
mod logs;
mod rpc;

use std::sync::Arc;
use std::time::{Duration, Instant, SystemTime};

use mixengine_core::{Paths, Store};
use mixengine_platform::ipc;
use mixengine_proto::{ProtocolVersion, Timestamp};
use tokio_util::sync::CancellationToken;

use crate::services;

pub(crate) use events::Events;
pub(crate) use http::serve_connection;

/// Everything a request handler is allowed to see.
///
/// Constructed once at startup and shared by every connection, per the injected-dependencies rule in
/// `.claude/standards/rust.md` — no globals, and nothing below this point reads the environment or
/// resolves a path of its own.
#[derive(Debug)]
pub(crate) struct Api {
    /// The daemon's build version, read once from the binary that is running.
    version: &'static str,

    /// The API version this build speaks.
    protocol: ProtocolVersion,

    /// The process id, so `daemon.status` can hand a user something they can look up in a task
    /// manager.
    pid: u32,

    /// Where the home is, as a string for display. See [`mixengine_proto::DaemonStatus`] for why
    /// these are not `PathBuf`s.
    home: String,

    /// The socket path or pipe name this daemon is listening on.
    endpoint: String,

    /// The SQLite file that is open. Not derived from `home` — `[paths]` can move it.
    database: String,

    /// The home's directory tree, for the one route that reads a file rather than state.
    ///
    /// `GET /logs/{id}` and nothing else: a service whose output this daemon never captured has its
    /// last lines in `current.log` and nowhere else, and the path to it is `Paths`' to know. Every
    /// other handler answers from memory or from the database.
    paths: Paths,

    /// The state rows, for the handlers that answer a question about one.
    ///
    /// Cheap to clone — one pool behind it — and held rather than reached for through the registry,
    /// because what `service.list` composes is three separate readings: the declared set, the row
    /// each of them has, and which of them this daemon is supervising. Only the first and the third
    /// are the registry's.
    store: Store,

    /// What is being supervised, and the only thing that starts or stops a service.
    ///
    /// T19 deliberately left this in `serve`, where the shutdown wait needs it, rather than adding a
    /// field nothing read. `service.*` is what reads it — roadmap task T19a.
    services: Arc<services::Registry>,

    /// The long operations this daemon is running, and the only thing that starts or cancels one.
    ///
    /// Beside `services` rather than inside it, because the two supervise different things: a
    /// service is a process with a lifetime of its own, a job is work with an end. Its first and
    /// only producer is `runtime.install` — see [`crate::runtimes`].
    jobs: Arc<crate::jobs::Jobs>,

    /// The installed runtimes, the index that offers more, and the only thing that starts an
    /// install.
    runtimes: Arc<crate::runtimes::Runtimes>,

    /// What each installed runtime can load, and the only thing that turns one round.
    ///
    /// Built here rather than passed in [`Supervision`]: it holds nothing of its own that outlives a
    /// call — the paths, the store and the registry beside it are the whole of it — so a field in
    /// `main` would be a fifth thing to keep in step for no reading of it.
    extensions: Arc<crate::extensions::Extensions>,

    /// The installed service packages, and the only thing that starts one of those installs.
    packages: Arc<crate::packages::Packages>,

    /// The registered projects, and the only thing that writes one down.
    ///
    /// Built here rather than passed in [`Supervision`], on `extensions`' reasoning: it holds
    /// nothing of its own that outlives a call — the store beside it is the whole of it — so a
    /// field in `main` would be one more thing to keep in step for no reading of it.
    projects: Arc<crate::projects::Projects>,

    /// The declared sites, and the only thing that writes one down.
    ///
    /// Built here for `projects`' reason: it holds nothing of its own that outlives a call.
    pub(crate) sites: Arc<crate::sites::Sites>,

    /// `mix doctor`'s half — roadmap task **T47a**.
    ///
    /// Holds every other part rather than being held by them: it is the one handler whose answer is
    /// assembled *across* subsystems, and each is reached through the door that already owns it.
    pub(crate) doctor: Arc<crate::doctor::Doctor>,

    /// `mix doctor --repair`'s half — roadmap task **T47b**.
    ///
    /// Built beside `doctor` and holding it, so the two halves of one feature cannot be given
    /// different dependencies: what a repair acts on is what the report found, read at the top of
    /// every call.
    pub(crate) repairs: Arc<crate::repair::Repairs>,

    /// The `domain.*` half — roadmap task **T46**.
    ///
    /// Built here for `sites`' reason, and over the same object: both write a site, and two doors
    /// onto one table would be two places for a rule to live.
    pub(crate) domains: Arc<crate::domains::Domains>,

    /// `<root>/bin` and this user's PATH, and the only thing that writes either.
    shims: Arc<crate::shims::Shims>,

    /// The queue of privileged operations, and the only thing that raises a prompt.
    pub(crate) elevation: Arc<crate::elevation::Elevation>,

    /// The DNS server, and which of the two name mechanisms this home is on — roadmap task T44.
    pub(crate) dns: Arc<crate::dns::Dns>,

    /// When the process began. See [`Started`].
    started: Started,

    /// The event stream every `GET /events` subscribes to.
    events: Events,

    /// How this daemon stops, and how long it is allowed to take — see [`Shutdown`].
    shutdown: Shutdown,
}

/// What this daemon is looking after: its services, its jobs, its runtimes and its `bin/`.
///
/// One argument rather than four, and the reason is written next door on [`Shutdown`]: `Api::new`
/// takes the readings that never change, and a constructor whose arguments have to be *counted* is
/// one a caller gets wrong silently. They belong together on their own terms as well — each is built
/// before the API so a handler can reach it, and each is the only door into the thing it holds. What
/// differs is what that is: a service is a process with a lifetime of its own, a job is work with an
/// end, a runtime is software on disk that outlives every daemon that will ever run here, and
/// `bin/` is the one of the four that is *reached from outside* — by a shim in a shell that has
/// never spoken to a daemon.
///
/// T22 made the first two into one argument for exactly this reason and predicted the growth; T23
/// and T26 are the growth.
#[derive(Debug)]
pub(crate) struct Supervision {
    /// What is being supervised, and the only thing that starts or stops a service.
    pub(crate) services: Arc<services::Registry>,

    /// The long operations, and the only thing that starts or cancels one.
    pub(crate) jobs: Arc<crate::jobs::Jobs>,

    /// What is installed, what could be, and the only thing that starts an install.
    pub(crate) runtimes: Arc<crate::runtimes::Runtimes>,

    /// The same, for the servers, databases and caches a service is an instance of.
    pub(crate) packages: Arc<crate::packages::Packages>,

    /// `<root>/bin` and this user's PATH, and the only thing that writes either.
    pub(crate) shims: Arc<crate::shims::Shims>,

    /// The queue of privileged operations, and the only thing that raises a prompt.
    pub(crate) elevation: Arc<crate::elevation::Elevation>,

    /// The DNS server, and the mode it puts this home in — roadmap task T44.
    ///
    /// Here rather than built in [`Api::new`], on `services`' reasoning rather than `extensions`':
    /// it binds sockets and owns a task, so there is exactly one per daemon, and the queue that
    /// decides whether this home still needs a hosts file reads the same object.
    pub(crate) dns: Arc<crate::dns::Dns>,
}

/// The two halves of a shutdown a handler can reach: the switch, and the budget.
///
/// One type rather than two fields because they are one decision made in two places — `main` reads
/// the budget out of `config.toml` and creates the token, and `daemon.shutdown` spends the first and
/// then throws the second. Keeping them together is also what stops [`Api::new`] growing an eighth
/// argument that a reader has to count.
#[derive(Debug)]
pub(crate) struct Shutdown {
    /// The daemon's root cancellation token, so a response that never ends on its own can.
    ///
    /// `GET /events` is the whole reason it is reachable from a handler: a stream that only ends
    /// when the client stops reading would keep a shutting-down daemon waiting for a GUI nobody is
    /// looking at. `daemon.shutdown` is the other, and it cancels rather than reads.
    token: CancellationToken,

    /// The whole of what `daemon.shutdown` may spend stopping services — roadmap task **T9a**.
    ///
    /// `config.toml`'s and not a caller's: how long this machine's services may take to shut down is
    /// a property of the machine, and a request that could ask for thirty seconds could ask for
    /// thirty minutes. Read once at startup, like everything else here.
    grace: Duration,
}

impl Shutdown {
    pub(crate) fn new(token: CancellationToken, grace: Duration) -> Self {
        Self { token, grace }
    }

    /// Commit to going, and hold the thing that makes it happen — see [`Going`].
    pub(crate) fn begun(&self) -> Going {
        Going {
            token: self.token.clone(),
        }
    }

    /// The root token, for a handler whose answer outlives the request that asked for it.
    pub(crate) fn token(&self) -> &CancellationToken {
        &self.token
    }

    /// What stopping every service may take, in total.
    fn grace(&self) -> Duration {
        self.grace
    }
}

/// A shutdown that has been ordered, held for as long as the handler performing it runs.
///
/// **A guard rather than a last statement, because a handler does not only end by returning.** The
/// future serving a request is dropped where it stands when its connection goes — hyper is built
/// with its default `half_close`, so a client that is interrupted mid-request takes the handler with
/// it — and a panic anywhere inside the walk does the same. `daemon.shutdown` cannot survive either:
/// [`Registry::stopping_within`](crate::services::Registry::stopping_within) latches the registry
/// shut on its first line and nothing ever clears that, so a shutdown that got that far and no
/// further leaves a daemon that is still listening, still answering, refusing every start it is
/// asked for, and waiting on a token nobody is left to cancel. The only way out of that is the one
/// T9a exists to remove.
///
/// So the cancellation is on the way out and not on a line: whatever ends the handler, the daemon
/// goes. The ordering the method rests on is unchanged — this drops after the walk and before the
/// answer is written, because the answer is encoded by the caller.
///
/// It is the same shape and the same reasoning as the registry's own `Stopping`, one layer up: a
/// claim that has to be released however the thing holding it ends.
#[derive(Debug)]
pub(crate) struct Going {
    /// The root token, cancelled when this is dropped.
    token: CancellationToken,
}

impl Drop for Going {
    fn drop(&mut self) {
        self.token.cancel();
    }
}

impl Api {
    /// Take the readings that never change, once.
    ///
    /// `endpoint` is passed rather than recomputed from `paths` because the listener is already
    /// bound to a particular one, and the status should name what is actually being listened on
    /// rather than what would be computed again now. `started` is passed for the opposite reason:
    /// taking it here would be taking it too late — see [`Started`].
    ///
    /// `events` is passed rather than made here because the API is no longer the only publisher:
    /// the registry of running services (T19) announces every transition it persists, and it is
    /// built before this so that a handler can reach it. `services` arrives for the same reason and
    /// is the same object the accept loop waits on at shutdown — one registry per daemon, not one
    /// per reader.
    pub(crate) fn new(
        paths: &Paths,
        store: &Store,
        endpoint: &ipc::Endpoint,
        started: Started,
        events: Events,
        supervision: Supervision,
        shutdown: Shutdown,
    ) -> Arc<Self> {
        let Supervision {
            services,
            jobs,
            runtimes,
            packages,
            shims,
            elevation,
            dns,
        } = supervision;

        let extensions = crate::extensions::Extensions::new(paths, store, Arc::clone(&services));
        let projects = crate::projects::Projects::new(store);
        let sites = crate::sites::Sites::new(store, Arc::clone(&elevation), Arc::clone(&services));
        let domains = crate::domains::Domains::new(
            Arc::clone(&sites),
            store,
            Arc::clone(&dns),
            elevation.host(),
        );
        let doctor = crate::doctor::Doctor::new(
            store,
            Arc::clone(&dns),
            elevation.host(),
            Arc::clone(&elevation),
            Arc::clone(&services),
            Arc::clone(&domains),
            paths,
        );
        let repairs = crate::repair::Repairs::new(
            Arc::clone(&doctor),
            Arc::clone(&elevation),
            Arc::clone(&services),
            store,
            paths,
        );

        Arc::new(Self {
            version: env!("CARGO_PKG_VERSION"),
            protocol: mixengine_proto::PROTOCOL_VERSION,
            pid: std::process::id(),
            home: paths.root().display().to_string(),
            endpoint: endpoint.to_string(),
            database: store.file().display().to_string(),
            paths: paths.clone(),
            store: store.clone(),
            services,
            jobs,
            runtimes,
            extensions,
            packages,
            projects,
            sites,
            domains,
            doctor,
            repairs,
            shims,
            elevation,
            dns,
            started,
            events,
            shutdown,
        })
    }

    /// The handle other parts of the daemon publish events through.
    pub(crate) fn events(&self) -> &Events {
        &self.events
    }

    /// What is being supervised, for the routes that are not JSON-RPC methods.
    fn services(&self) -> &Arc<services::Registry> {
        &self.services
    }

    /// The home's directory tree — see [`Api::paths`].
    fn paths(&self) -> &Paths {
        &self.paths
    }

    /// How this daemon stops — the token a long-lived response ends on, and the budget a shutdown
    /// spends.
    pub(crate) fn shutdown(&self) -> &Shutdown {
        &self.shutdown
    }
}

/// The moment the daemon process began, on both clocks.
///
/// Taken in `main` before any work rather than here, because "when did this daemon start" is a
/// question about the process and not about its API: the first run of a home creates the directory
/// tree, runs the migrations and opens SQLite, and a reading taken afterwards would quietly leave
/// all of that out of `uptime` — on exactly the start where it takes longest.
#[derive(Debug, Clone, Copy)]
pub(crate) struct Started {
    /// The wall clock, for a client that wants to render a date.
    at: Timestamp,

    /// The same moment on the monotonic clock, which is the one uptime is computed from: a system
    /// clock corrected while the daemon runs would otherwise make it jump or go backwards.
    since: Instant,
}

impl Started {
    /// Now, on both clocks.
    pub(crate) fn now() -> Self {
        Self {
            at: Timestamp::from_system_time(SystemTime::now()),
            since: Instant::now(),
        }
    }

    /// The wall-clock moment, for `started_at`.
    fn at(self) -> Timestamp {
        self.at
    }

    /// How long ago that was, measured on the clock that cannot be corrected out from under it.
    fn elapsed(self) -> std::time::Duration {
        self.since.elapsed()
    }
}
