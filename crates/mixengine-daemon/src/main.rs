//! `mixengined` — the only process that owns state. Clients are thin; this is not.

mod api;
mod error;
mod logging;

use std::io::IsTerminal;
use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;

use anyhow::Context as _;
use clap::{Parser, ValueEnum};
use mixengine_core::{Paths, Store, config};
use mixengine_platform::ipc;

use error::ToWire as _;

/// How long to wait before accepting again after `accept` itself failed.
///
/// A failure there is nearly always about one connection — a client that died between the kernel
/// queueing it and us asking who it was — and must not end the accept loop, which would take the
/// daemon and every service it supervises down with it. But the failures that are *not* per
/// connection would spin this loop at the speed of the CPU, so the retry is paced.
const ACCEPT_PAUSE: Duration = Duration::from_millis(200);

/// How long a shutting-down daemon waits for the connections that are still open.
///
/// Deliberately shorter than the ten seconds `daemon.shutdown` gives a *service* to stop: a service
/// is flushing a database, while a client is finishing one local request. What actually consumes
/// this budget is `GET /events`, which never ends on its own — see [`shut_down`].
const SHUTDOWN_GRACE: Duration = Duration::from_secs(2);

/// Command line of the daemon. Configuration enters the program here and is passed down; nothing
/// deeper reads the environment on its own.
#[derive(Debug, Parser)]
#[command(name = "mixengined", version, about = "MixEngine daemon")]
struct Args {
    /// Root directory for everything MixEngine owns.
    ///
    /// Defaults to the OS convention (`%LOCALAPPDATA%\MixEngine`,
    /// `~/Library/Application Support/MixEngine`, `$XDG_DATA_HOME/mixengine`). Point it somewhere
    /// disposable while experimenting — this is the only thing separating a sandbox from a real
    /// install.
    #[arg(long, env = "MIXENGINE_HOME", value_name = "DIR")]
    home: Option<PathBuf>,

    /// Stay in the foreground instead of detaching (the default while developing).
    ///
    /// Detaching itself is task T9; until then this build always stays in the foreground and says
    /// so rather than letting the caller believe it forked.
    #[arg(long)]
    foreground: bool,

    /// How much to log. Overrides `log.level` in `config.toml`.
    #[arg(long, value_enum)]
    log_level: Option<LogLevel>,

    /// How to shape each log line. Overrides `log.format` in `config.toml`.
    ///
    /// The environment variable is the one a log collector sets: it wraps a command it did not
    /// write, so it cannot add a flag to it. A value neither this build nor the collector
    /// recognises fails the start rather than being ignored — silently text-formatted output is a
    /// log nobody is reading.
    #[arg(long, value_enum, env = "MIXENGINE_LOG_FORMAT")]
    log_format: Option<LogFormat>,
}

/// Verbosity of the daemon log.
///
/// A closed set on purpose: a free-form string would let a typo silence logging entirely, and the
/// process would start looking perfectly healthy while saying nothing. It mirrors
/// [`config::LogLevel`] because `clap` belongs to the binary and `core` must not depend on it.
#[derive(Debug, Clone, Copy, ValueEnum)]
enum LogLevel {
    Error,
    Warn,
    Info,
    Debug,
    Trace,
}

impl From<LogLevel> for config::LogLevel {
    fn from(level: LogLevel) -> Self {
        match level {
            LogLevel::Error => Self::Error,
            LogLevel::Warn => Self::Warn,
            LogLevel::Info => Self::Info,
            LogLevel::Debug => Self::Debug,
            LogLevel::Trace => Self::Trace,
        }
    }
}

/// Shape of each log line, mirroring [`config::LogFormat`] for the same reason as [`LogLevel`].
#[derive(Debug, Clone, Copy, ValueEnum)]
enum LogFormat {
    Text,
    Json,
}

impl From<LogFormat> for config::LogFormat {
    fn from(format: LogFormat) -> Self {
        match format {
            LogFormat::Text => Self::Text,
            LogFormat::Json => Self::Json,
        }
    }
}

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    // First line of the process, so that `daemon.status` answers when this daemon *started* rather
    // than when it finished starting: creating a home, running the migrations and opening SQLite
    // are seconds a user would otherwise never see in `uptime`.
    let started = api::Started::now();

    let args = Args::parse();

    // Before anything else: find the home directory, read config.toml, create what is missing.
    // It happens before logging is set up because the log level is one of the things it reads —
    // a failure here is reported by `main` returning it, not by a logger that does not exist yet.
    let host = mixengine_platform::host();
    // Through the wire mapping even though there is no wire yet: the boundary is the only place a
    // hint is written, and a startup failure — the wrong MIXENGINE_HOME, a `[paths]` override onto
    // a disk nobody mounted — is exactly the kind that needs one. Whoever is reading stderr now
    // gets the same sentence a client would get later.
    let home = mixengine_core::open_home(args.home.as_deref(), host.as_ref())
        .map_err(|error| error.to_wire())?;

    // A flag beats the file, and the file beats the default. Neither is read anywhere but here.
    let options = logging::Options {
        file: home.paths.daemon_log_file(),
        level: args
            .log_level
            .map_or(home.config.log.level, config::LogLevel::from),
        format: args
            .log_format
            .map_or(home.config.log.format, config::LogFormat::from),
        // Colour only when a human is watching. The file never gets any — see `logging`.
        colour: std::io::stderr().is_terminal(),
    };

    logging::init(&options).with_context(|| {
        format!(
            "cannot write the daemon log at {}",
            home.paths.daemon_log_file().display()
        )
    })?;

    tracing::info!(
        version = env!("CARGO_PKG_VERSION"),
        protocol = %mixengine_proto::PROTOCOL_VERSION,
        home = %home.paths.root().display(),
        log = %home.paths.daemon_log_file().display(),
        "mixengined starting"
    );

    if !args.foreground {
        tracing::warn!("detaching is not implemented yet (T9) — staying in the foreground");
    }

    // Through the same mapping as `open_home`, and for the same reason: a database that will not
    // open is a startup failure whose way out — a home directory that moved, a copy taken before
    // the last upgrade — is written at the boundary and nowhere else.
    let store = Store::open(home.paths.database_file())
        .await
        .map_err(|error| error.to_wire())?;

    tracing::info!(database = %store.file().display(), "database open and up to date");

    // Everything that runs with the database open lives in `serve`, and its result is held rather
    // than propagated with `?`, so that the close below is on the only way out — the transport
    // fails to bind whenever a second daemon is already up, and a `?` there would skip the
    // checkpoint on exactly the exits that matter.
    let served = serve(&home.paths, &store, started).await;

    // Awaited rather than dropped: closing the pool checkpoints the write-ahead log, which is what
    // leaves a single file behind instead of one with a `-wal` sidecar holding the newest commits.
    store.close().await;

    served
}

/// What the daemon does while its state is open.
///
/// Separate from `main` so that `Store::close` has a single call site that every exit passes
/// through, including the failing ones.
async fn serve(paths: &Paths, store: &Store, started: api::Started) -> anyhow::Result<()> {
    // Both through the wire mapping, and for the same reason the two startup steps above are: the
    // failure a person actually meets here is "a daemon is already running for this home", and the
    // sentence that says what to do about it is written at the boundary and nowhere else.
    let endpoint = ipc::Endpoint::in_run_dir(paths.run()).map_err(|error| error.to_wire())?;
    let mut listener = ipc::Listener::bind(&endpoint).map_err(|error| error.to_wire())?;

    // Built after the listener rather than before it, so `daemon.status` reports the endpoint that
    // was actually bound instead of the one that would be computed again now.
    let api = api::Api::new(paths, store, &endpoint, started);

    tracing::info!(endpoint = %endpoint, "listening for clients");

    // Connections are tracked rather than detached, because `.claude/standards/rust.md` forbids a
    // task that outlives shutdown and because a `/events` stream would otherwise be cut mid-frame.
    // T9 replaces the interrupt below with a cancellation token; this set is what that token will
    // have to wait on, and it is here now so the wait is not bolted on afterwards.
    let mut connections = tokio::task::JoinSet::new();

    loop {
        tokio::select! {
            // Placeholder for the cancellation token T9 brings: without it `--foreground` would
            // have to be killed rather than stopped, and the socket would be left behind every
            // time — which is the case its own cleanup path exists for and should not be the
            // normal one.
            interrupted = tokio::signal::ctrl_c() => {
                interrupted.context("cannot listen for an interrupt")?;
                tracing::info!("interrupted — shutting down");
                break;
            }

            accepted = listener.accept() => match accepted {
                Ok(ipc::Accepted::Trusted(connection)) => {
                    tracing::debug!("a client connected");
                    connections.spawn(api::serve_connection(Arc::clone(&api), connection));
                }

                // Not an error and not a failure of anything: the endpoint's own permissions
                // should already have made this impossible, so it is worth a line saying whose
                // connection was turned away and never worth ending the loop over.
                Ok(ipc::Accepted::Untrusted(peer)) => {
                    tracing::warn!(%peer, "refused a connection from another account");
                }

                Err(error) => {
                    tracing::warn!(%error, "cannot accept a connection");
                    tokio::time::sleep(ACCEPT_PAUSE).await;
                }
            },

            // Reaped as they finish rather than only at shutdown, so a daemon a client has been
            // connecting to all day does not accumulate one completed task per connection.
            Some(finished) = connections.join_next(), if !connections.is_empty() => {
                if let Err(error) = finished {
                    tracing::warn!(%error, "a client connection task did not finish cleanly");
                }
            }
        }
    }

    shut_down(connections).await;

    Ok(())
}

/// Let the connections that are still open finish, then stop waiting.
///
/// A grace period rather than an abort, because a client mid-request has already been told the
/// daemon accepted it — and rather than an unbounded wait, because `GET /events` never ends on its
/// own: a stream nobody closed would keep a shutting-down daemon alive until the client noticed.
/// Dropping the set at the end aborts whatever is left, which for an event stream is exactly right.
async fn shut_down(mut connections: tokio::task::JoinSet<()>) {
    if connections.is_empty() {
        return;
    }

    tracing::info!(open = connections.len(), "waiting for clients to finish");

    if tokio::time::timeout(SHUTDOWN_GRACE, async {
        while connections.join_next().await.is_some() {}
    })
    .await
    .is_err()
    {
        tracing::info!(
            open = connections.len(),
            "closing connections that were still open"
        );
    }
}
