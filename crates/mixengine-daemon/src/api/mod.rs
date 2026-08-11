//! The API server: JSON-RPC over HTTP/1.1 on the local endpoint T7 opened.
//!
//! Three layers, one per file, and the split is the same one the architecture draws:
//!
//! - [`http`] is the transport. Routing, body limits, timeouts, spans, and the connection loop.
//! - [`rpc`] is the protocol. Batches, notifications, method dispatch, panic containment.
//! - [`events`] is the stream. `GET /events`, a bounded broadcast, and what a slow client is told.
//!
//! Nothing in here is business logic — `.claude/CLAUDE.md` puts that in `mixengine-core` — and the
//! handlers are the proof: each one turns state the daemon already holds into a `mixengine-proto`
//! type and does nothing else.

mod events;
mod http;
mod rpc;

use std::sync::Arc;
use std::time::{Instant, SystemTime};

use mixengine_core::{Paths, Store};
use mixengine_platform::ipc;
use mixengine_proto::{ProtocolVersion, Timestamp};
use tokio_util::sync::CancellationToken;

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

    /// When the process began. See [`Started`].
    started: Started,

    /// The event stream every `GET /events` subscribes to.
    events: Events,

    /// The daemon's root cancellation token, so a response that never ends on its own can.
    ///
    /// `GET /events` is the whole reason it is here: a stream that only ends when the client stops
    /// reading would keep a shutting-down daemon waiting for a GUI nobody is looking at.
    shutdown: CancellationToken,
}

impl Api {
    /// Take the readings that never change, once.
    ///
    /// `endpoint` is passed rather than recomputed from `paths` because the listener is already
    /// bound to a particular one, and the status should name what is actually being listened on
    /// rather than what would be computed again now. `started` is passed for the opposite reason:
    /// taking it here would be taking it too late — see [`Started`].
    pub(crate) fn new(
        paths: &Paths,
        store: &Store,
        endpoint: &ipc::Endpoint,
        started: Started,
        shutdown: CancellationToken,
    ) -> Arc<Self> {
        Arc::new(Self {
            version: env!("CARGO_PKG_VERSION"),
            protocol: mixengine_proto::PROTOCOL_VERSION,
            pid: std::process::id(),
            home: paths.root().display().to_string(),
            endpoint: endpoint.to_string(),
            database: store.file().display().to_string(),
            started,
            events: Events::new(),
            shutdown,
        })
    }

    /// The handle other parts of the daemon publish events through.
    pub(crate) fn events(&self) -> &Events {
        &self.events
    }

    /// The root token, for a handler whose answer outlives the request that asked for it.
    pub(crate) fn shutdown(&self) -> &CancellationToken {
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
