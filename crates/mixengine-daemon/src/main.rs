//! `mixengined` — the only process that owns state. Clients are thin; this is not.

mod api;
mod error;
mod logging;
mod services;

use std::ffi::OsString;
use std::io::IsTerminal;
use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;

use anyhow::Context as _;
use clap::{Parser, ValueEnum};
use mixengine_core::{Paths, Store, config};
use mixengine_platform::{ipc, lock, process, signal};
use tokio::time::Instant;
use tokio_util::sync::CancellationToken;

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
/// is flushing a database, while a client is finishing one local request. It also has to fit inside
/// the few seconds Windows allows a console control handler before it terminates the process
/// regardless — see `mixengine_platform::signal`. What used to consume this budget was
/// `GET /events`, which never ends on its own; it now ends with the root token, so this is the
/// margin for a request that is genuinely mid-flight rather than the normal cost of shutting down.
const SHUTDOWN_GRACE: Duration = Duration::from_secs(2);

/// How long `--detach` waits for the daemon it started to answer.
///
/// A ceiling and not a wait: the poll returns the moment the endpoint answers. It is generous
/// because the first start of a home creates the directory tree, runs the migrations and opens
/// SQLite, and because the machine this has to be reliable on is a loaded CI runner.
const DETACH_TIMEOUT: Duration = Duration::from_secs(30);

/// How often it asks during that.
const DETACH_POLL: Duration = Duration::from_millis(50);

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

    /// Start the daemon in the background and print the endpoint it is listening on.
    ///
    /// Without this the daemon stays in the foreground, which is what a service manager wants —
    /// systemd, launchd and Task Scheduler all supervise the process themselves and would treat a
    /// process that forked away as one that had died. This flag exists for the other caller: a
    /// client that finds no daemon running and starts one (roadmap task T10) cannot sit holding it.
    ///
    /// It returns only once the daemon answers on its endpoint, so a client that gets a zero exit
    /// status can connect immediately rather than retrying against a daemon that may still be
    /// migrating a database.
    #[arg(long)]
    detach: bool,

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

/// The spelling a flag's value has on a command line, for the child a `--detach` start builds.
///
/// Asked of `clap` rather than written out a second time. A table repeating these by hand drifts the
/// moment a variant is renamed, and it drifts silently: the child would be started with a value this
/// same binary no longer accepts, and nothing would say so until somebody ran `--detach`.
fn as_arg(value: impl ValueEnum) -> String {
    value
        .to_possible_value()
        .expect("every value of a MixEngine flag is one clap can spell — none of them is skipped")
        .get_name()
        .to_owned()
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

    // Computed here rather than inside `serve`, because two of the three ways out of this function
    // need it before anything is bound: `--detach` waits on it, and a daemon that finds the lock
    // taken prints it.
    let endpoint = ipc::Endpoint::in_run_dir(home.paths.run()).map_err(|error| error.to_wire())?;

    // Deliberately before `logging::init`, and it is the *duration* that decides it rather than the
    // number of writers. This process is not a daemon: it lives alongside the one it starts for as
    // long as that one takes to come up, which is precisely when the daemon is writing its startup
    // lines and may rotate the file out from under a second writer. It also has nothing to say that
    // belongs in a daemon's log — one line on stdout is its entire output, and that is what the
    // person who typed the command is reading.
    //
    // The daemon that finds the lock taken below is the other way round on both counts: two lines
    // and gone in milliseconds, and the two lines are worth keeping, because "somebody tried to
    // start a second daemon at 3am" is the kind of thing the log exists to answer.
    if args.detach {
        return detach(&args, &home.paths, &endpoint).await;
    }

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

    // **Before `Store::open`, and that ordering is the point of taking it here.** `sqlx-sqlite`
    // implements the migration lock as a no-op, SQLite having no advisory lock to use, so two
    // daemons that both got as far as opening the database could both read the schema as behind and
    // both migrate it. A single-instance lock acquired afterwards would guard nothing.
    let lock = match lock::Lock::acquire(home.paths.lock_file()).map_err(|error| error.to_wire())? {
        lock::Acquired::Held(lock) => lock,

        // Not a failure, and the exit status says so. The caller asked for a running daemon for this
        // home and there is one — `.claude/architecture/daemon-and-ipc.md` has this print the
        // endpoint and stop, which is also what makes two clients autostarting at the same instant
        // (roadmap task T10) produce one daemon and no error message.
        lock::Acquired::Taken(holder) => {
            tracing::info!(%holder, %endpoint, "a daemon is already running for this home");
            println!("{endpoint}");
            return Ok(());
        }
    };

    // Through the same mapping as `open_home`, and for the same reason: a database that will not
    // open is a startup failure whose way out — a home directory that moved, a copy taken before
    // the last upgrade — is written at the boundary and nowhere else.
    let store = Store::open(home.paths.database_file())
        .await
        .map_err(|error| error.to_wire())?;

    tracing::info!(database = %store.file().display(), "database open and up to date");

    // Everything that runs with the database open lives in `serve`, and its result is held rather
    // than propagated with `?`, so that the close below is on the only way out — the transport
    // fails to bind whenever something else is already listening, and a `?` there would skip the
    // checkpoint on exactly the exits that matter.
    let served = serve(&home.paths, &store, started, &endpoint).await;

    // Awaited rather than dropped: closing the pool checkpoints the write-ahead log, which is what
    // leaves a single file behind instead of one with a `-wal` sidecar holding the newest commits.
    store.close().await;

    // Released last, and explicitly. While it is held no other daemon can reach the database, which
    // is the property the checkpoint above needs: dropping the lock first would open a window in
    // which the next daemon starts against a file this one is still finishing with.
    drop(lock);

    served
}

/// Start a daemon in the background and wait until it answers.
///
/// The child is this same binary started again without `--detach`, rather than a fork: Windows has
/// no `fork`, and forking a process that already has a Tokio runtime — several threads, a reactor,
/// a pool of locks held by threads that do not exist in the child — is a way of producing a daemon
/// that hangs on its first `await`. One mechanism on all three systems also means one code path to
/// keep working on them.
///
/// The arguments are rebuilt from what was parsed rather than filtered out of `args_os`, which
/// matters most for the home: the child is told the *resolved* root, so a `--home` given as a
/// relative path, or one that came from `MIXENGINE_HOME`, cannot be re-resolved by the child against
/// a working directory or an environment that is not the same one.
async fn detach(args: &Args, paths: &Paths, endpoint: &ipc::Endpoint) -> anyhow::Result<()> {
    // Asked before anything is started, because the common case for the caller this flag exists for
    // is that a daemon is *already* running: a client autostarts one whenever it cannot reach the
    // endpoint, and two clients doing that at once means the second one arrives to a daemon that is
    // up. Spawning a process whose entire job would be to find the lock taken and exit is a cost
    // with nothing on the other side of it.
    if ipc::Connection::connect(endpoint).await.is_ok() {
        println!("{endpoint}");
        return Ok(());
    }

    let program = std::env::current_exe().context("cannot find the running mixengined binary")?;

    let mut arguments = vec![
        OsString::from("--home"),
        paths.root().as_os_str().to_owned(),
    ];

    if let Some(level) = args.log_level {
        arguments.push("--log-level".into());
        arguments.push(as_arg(level).into());
    }

    // Passed explicitly even though the child inherits `MIXENGINE_LOG_FORMAT` with the rest of the
    // environment: a `--log-format` given on the command line has to beat that variable in the
    // child exactly as it did here, and only an argument does that.
    if let Some(format) = args.log_format {
        arguments.push("--log-format".into());
        arguments.push(as_arg(format).into());
    }

    // The home, and deliberately not this process's working directory. A daemon holds its working
    // directory for days, and the directory a client autostarting one happens to be in is a project
    // folder somebody is working in — which they would then be unable to rename or delete on
    // Windows. The home is the one directory the daemon is entitled to pin, and it exists by now
    // because `open_home` has just created it.
    let mut daemon = process::spawn_detached(&program, &arguments, paths.root())
        .map_err(|error| error.to_wire())?;

    let deadline = Instant::now() + DETACH_TIMEOUT;

    // Kept rather than re-asked, so the loop below reads as one question answered once: the child is
    // running until it is not, and what it exited with does not change afterwards.
    let mut exit: Option<process::Exit> = None;

    loop {
        // Dialling the endpoint rather than asking `/health`: what a client needs to know is that
        // there is something at the other end to send a request to, and a connection proves that
        // without this process having to speak HTTP at all.
        if ipc::Connection::connect(endpoint).await.is_ok() {
            println!("{endpoint}");
            return Ok(());
        }

        // "Not up yet" and "gone" are the same silence, and only one of them is worth waiting out.
        if exit.is_none() {
            exit = daemon.exited().map_err(|error| error.to_wire())?;
        }

        // A child that *failed* has nothing left to wait for, and saying so at once is the whole
        // point of watching it. A child that succeeded is the opposite case and must not be treated
        // as this one: it exits 0 precisely when another daemon already holds this home — and that
        // daemon takes the lock **before** it opens SQLite, so between those two moments there is a
        // whole set of migrations during which the endpoint is legitimately not answering yet. This
        // used to end the wait there and turn two clients autostarting at the same instant (roadmap
        // task T10) into a failure for whichever of them lost the race. The deadline below is what
        // that case waits on, and is generous for exactly this reason.
        if let Some(status) = &exit
            && !status.is_success()
        {
            anyhow::bail!(
                "the daemon stopped without listening on {endpoint} ({status}) — {} says why",
                paths.daemon_log_file().display()
            );
        }

        if Instant::now() >= deadline {
            return Err(match exit {
                // It stood aside for a daemon that then never answered. The lock file names the one
                // to go and look at, which is not a process this command started.
                Some(status) => anyhow::anyhow!(
                    "another daemon holds {} and did not start listening on {endpoint} within \
                     {DETACH_TIMEOUT:?} — the one started here stood aside for it ({status}), and \
                     {} says what it is doing",
                    paths.lock_file().display(),
                    paths.daemon_log_file().display()
                ),

                None => anyhow::anyhow!(
                    "the daemon (pid {}) did not start listening on {endpoint} within \
                     {DETACH_TIMEOUT:?} — it is still running, and {} says what it is doing",
                    daemon.pid(),
                    paths.daemon_log_file().display()
                ),
            });
        }

        tokio::time::sleep(DETACH_POLL).await;
    }
}

/// What the daemon does while its state is open.
///
/// Separate from `main` so that `Store::close` has a single call site that every exit passes
/// through, including the failing ones.
async fn serve(
    paths: &Paths,
    store: &Store,
    started: api::Started,
    endpoint: &ipc::Endpoint,
) -> anyhow::Result<()> {
    // Through the wire mapping, and for the same reason the startup steps above are: the failure a
    // person actually meets here is "something else is already listening for this home", and the
    // sentence that says what to do about it is written at the boundary and nowhere else.
    let mut listener = ipc::Listener::bind(endpoint).map_err(|error| error.to_wire())?;

    // Registered before the first client rather than inside the loop. `select!` builds its futures
    // afresh on every turn, so registering there would tear the handlers down and reinstall them
    // continuously and could lose a signal that arrived in between. Failing the start is the right
    // answer to a handler the OS will not install: a daemon that cannot be asked to stop is one
    // somebody has to kill, and a shutdown is the wrong moment to find that out.
    let mut signals = signal::Signals::listen().map_err(|error| error.to_wire())?;

    // The root token every other shutdown path is a branch of. It is cancelled by a signal and
    // awaited by the accept loop and by every `GET /events`; `daemon.shutdown` (roadmap task T9a)
    // cancels the same object, and every service the registry supervises hangs a child token off it.
    let shutdown = CancellationToken::new();

    // Made here rather than inside the API, because the API is no longer the only publisher: the
    // registry announces every state change it persists, from tasks that outlive any one request.
    let events = api::Events::new();

    // **The source is `Undeclared` until T30.** Nothing in this build renders a `services` row into
    // a runnable spec, so the honest declared set is the empty one — the registry, the graph and the
    // walk all handle it without a special case, and the day the generator exists is the day this
    // line changes and nothing else does.
    let services = Arc::new(services::Registry::new(
        paths,
        store,
        mixengine_platform::host(),
        events.clone(),
        Arc::new(services::Undeclared),
        shutdown.clone(),
    ));

    // Built after the listener rather than before it, so `daemon.status` reports the endpoint that
    // was actually bound instead of the one that would be computed again now.
    let api = api::Api::new(paths, store, endpoint, started, events, shutdown.clone());

    tracing::info!(endpoint = %endpoint, "listening for clients");

    // Connections are tracked rather than detached, because `.claude/standards/rust.md` forbids a
    // task that outlives shutdown and because a `/events` stream would otherwise be cut mid-frame.
    let mut connections = tokio::task::JoinSet::new();

    loop {
        tokio::select! {
            // Ctrl-C, `systemctl --user stop`, the console closing, the machine shutting down —
            // whichever of them this OS has. Cancel safe, so a turn that serves a client instead
            // has not swallowed one.
            stop = signals.stopped() => {
                tracing::info!(%stop, "shutting down");
                shutdown.cancel();
                break;
            }

            // Cancelled by something inside the daemon rather than by the OS. Nothing does that
            // yet; the arm is here because the token is what T9a's `daemon.shutdown` cancels, and
            // because the loop must not be the one place that only understands signals.
            () = shutdown.cancelled() => {
                tracing::info!("shutting down");
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

    // Before the connections, and that order is the point: a service is what a user loses if this
    // process exits while it is still flushing, and a client mid-request is not. The root token is
    // already cancelled, so every runner is performing the stop its spec asks for; this is where the
    // daemon waits for them instead of leaving the job to a destructor that kills.
    services.shut_down().await;

    shut_down(connections).await;

    Ok(())
}

/// Let the connections that are still open finish, then stop waiting.
///
/// A grace period rather than an abort, because a client mid-request has already been told the
/// daemon accepted it — and rather than an unbounded wait, because a connection is kept alive
/// between requests: a client that has ended its `GET /events` (the root token does that as this is
/// called) may still be holding a socket nobody is going to write to again. Dropping the set at the
/// end aborts whatever is left, which for a connection with no request in flight is exactly right.
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
