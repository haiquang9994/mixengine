//! `mixengined` — the only process that owns state. Clients are thin; this is not.

mod api;
mod dns;
mod doctor;
mod domains;
mod elevation;
mod error;
mod extensions;
mod jobs;
mod logging;
mod packages;
mod projects;
mod repair;
mod runtimes;
mod services;
mod shims;
mod sites;

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
/// Deliberately shorter than the seconds `daemon.shutdown` gives a *service* to stop: a service is
/// flushing a database, while a client is finishing one local request. What used to consume this
/// budget was `GET /events`, which never ends on its own; it now ends with the root token, so this
/// is the margin for a request that is genuinely mid-flight rather than the normal cost of shutting
/// down — including the answer to `daemon.shutdown` itself, which is written into a connection this
/// daemon has already decided to stop waiting for.
///
/// **The `daemon.shutdown` path's number, and since T9a only that path's.** A shutdown the OS asked
/// for has no answer to write and arrives with a clock running — see [`SIGNAL_CLIENT_GRACE`].
const CLIENT_GRACE: Duration = Duration::from_secs(2);

/// The same, for the shutdown an operating system asked for — roadmap task **T9a**.
///
/// **A quarter of [`CLIENT_GRACE`], and the one difference between the two paths is what pays for
/// it.** `daemon.shutdown` has an answer to write into one of these connections, and the two seconds
/// above are mostly for that; a console control event has no answer to write to anybody. What is
/// left holding a connection when one arrives is a `mix status` mid-request — a local round trip
/// measured in milliseconds — or a client sitting on a keep-alive socket between requests, which
/// nobody is going to write to again and which dropping the set already handles correctly. Waiting
/// the full two seconds on *that* is two seconds taken from the WAL checkpoint on the one system
/// where something else is counting, which is the trade [`CEILING_RESERVE`] describes.
const SIGNAL_CLIENT_GRACE: Duration = Duration::from_millis(500);

/// What [`CEILING_RESERVE`] keeps back for `Store::close` — roadmap task **T9a**.
///
/// The checkpoint is the step whose overrun actually loses something: a process terminated in the
/// middle of it leaves the `-wal` sidecar holding the newest commits, and every other number here
/// exists to make sure this one is reached. A second is generous against a database of service rows
/// and settings, and it is deliberately not measured from a run on this machine — what it has to
/// survive is a laptop resuming from sleep with a hundred processes asking for the disk at once.
const CHECKPOINT_MARGIN: Duration = Duration::from_secs(1);

/// What [`CEILING_RESERVE`] keeps back for nothing in particular — roadmap task **T9a**.
///
/// Every other part of the reserve is a wait this daemon performs and can therefore be sure of.
/// This one covers what it cannot: a task the runtime schedules late because eight runners are
/// finishing at once, a `tracing` line that blocks while the log file rotates, a `join_next` that
/// returns a moment after the last connection did. The reserve used to have none of this — the
/// arithmetic came to exactly the ceiling — so the process was one slow scheduler away from being
/// terminated mid-checkpoint, which is the outcome the reserve exists to prevent rather than to
/// meet exactly.
///
/// It is also what the two waits a spent budget still permits are spent out of: see [`KILL_GRACE`]
/// and [`CONFIRMATION_REPRIEVE`].
const SCHEDULING_SLACK: Duration = Duration::from_secs(1);

/// What a shutdown keeps back from an operating system's ceiling for everything that is not a
/// service — roadmap task **T9a**.
///
/// Windows gives a console control handler about five seconds and then ends the process whatever it
/// is doing ([`signal::STOP_CEILING`]), and stopping services is not the last thing a shutdown does:
/// the connections still open get [`SIGNAL_CLIENT_GRACE`], and `Store::close` then checkpoints the
/// write-ahead log, which is what leaves one database file behind instead of one with a `-wal`
/// sidecar holding the newest commits. A budget that spent the whole ceiling on services would be
/// terminated in the middle of exactly that.
///
/// **Summed from its parts rather than chosen, because as one number it left no margin at all.** It
/// was two and a half seconds against a two-second client grace, so the checkpoint had five hundred
/// milliseconds and the whole came to the ceiling exactly: 2.5 s of services, 2 s of clients and
/// 0.5 s of checkpoint is 5 s of 5, with nothing left for a task scheduled late or a machine under
/// load — and `windows/signal.rs` says the ceiling itself may be *shorter* than five where
/// `WaitToKillTimeout` or `HungAppTimeout` were configured. So what it keeps back is now stated as
/// the three things that happen after the last service stops:
///
/// - [`SIGNAL_CLIENT_GRACE`], half a second for the connections still open,
/// - [`CHECKPOINT_MARGIN`], a second for `Store::close` and the write-ahead log,
/// - [`SCHEDULING_SLACK`], a second that is left over on purpose.
///
/// The total is the same 2.5 s, which leaves the services 2.5 s on Windows and puts the daemon at
/// 2.5 + 0.5 + 1 = 4 s of the 5 s the OS allows, one second of it slack. **The margin is bought from
/// the clients rather than from the services**, which is the choice worth naming: shortening the
/// client grace on the signal path costs a client that is between requests nothing, where raising
/// this constant instead would have taken the same second out of MariaDB's flush.
///
/// Subtracted **only** where there is a ceiling to subtract it from. On Unix nothing is counting and
/// the budget is the configured one entire.
const CEILING_RESERVE: Duration = SIGNAL_CLIENT_GRACE
    .saturating_add(CHECKPOINT_MARGIN)
    .saturating_add(SCHEDULING_SLACK);

/// How long a shutdown that was asked to hurry still waits for the services it has just told to
/// kill — roadmap task **T9a**.
///
/// A second request to stop narrows the budget to nothing, which puts every runner still to reach a
/// stop at the same place: `Supervised`'s kill — `TerminateJobObject` or a `SIGKILL` to the group,
/// and the reap of a child this process is the parent of. Those are syscalls rather than grace
/// periods, so half a second is a great deal of room for them even on a machine that is swapping.
///
/// **Bounded at all, because the whole point of the escalation is that a third signal must not be
/// needed.** A narrowed budget reaches a runner that has not begun its stop, and does not reach one
/// that already has: a grace period is a deadline fixed from what the budget said when the stop
/// began, so a service that was inside one when the second request arrived runs to the end of the
/// time the *first* request granted it. That is the case this bound exists for, and it is the common
/// one — the person is asking again precisely because something is taking its whole grace period.
///
/// What running out costs is stated rather than hidden: the runners still waiting are abandoned
/// rather than aborted, so a service this daemon had asked to stop is left for the next one to meet
/// as the crash recovery it already performs (roadmap task T18). That is the same bargain `kill -9`
/// would have made — except that this way the database still gets its checkpoint, which is the whole
/// reason the escalation is not a `return`.
///
/// **Bounded at half a second, because [`SCHEDULING_SLACK`] is what pays for it.** The wait arrives
/// on top of a budget that may already have run to the end of its clock, so the worst case on
/// Windows is 2.5 s of services, 0.5 s here, 0.5 s of clients and 1 s of checkpoint — 4.5 s of the
/// 5 s the OS allows. That it fits inside the slack is a test rather than this sentence.
const KILL_GRACE: Duration = Duration::from_millis(500);

/// How far past its own deadline a shutdown may still watch a process it has just killed — roadmap
/// task **T9a**.
///
/// **The one wait for which zero is not a real answer.** Every other constant the budget shortens is
/// a wait whose absence costs something bounded and stated: a grace period of zero is a service
/// killed at once, a log drain of zero is a tail nobody was reading. The poll after the kill in
/// `Runner::stop_adopted` is different in kind, because what it bounds is not a wait but a
/// *question* — whether the survivor this daemon just killed has left the process table. Asked with
/// no window at all it is asked microseconds after the kill, the kernel has not finished with the
/// process yet, and the answer is read as a survivor that will not go: the row keeps its `stopping`,
/// the stop is reported as failed, and the walk stops there on the ordering rule, so every service
/// after it in the plan is left running by a stop that in fact succeeded.
///
/// **One window for the whole walk rather than an allowance per service**, which is what keeps it
/// out of the ceiling arithmetic: it is expressed as a second deadline this far past the first (see
/// [`Budget`](services::Budget)), so eight survivors reached after the budget ran out cost what one
/// of them costs. Per service it would have been unbounded, and an unbounded term is exactly what
/// [`CEILING_RESERVE`] cannot contain.
///
/// **A quarter second, and paid from [`SCHEDULING_SLACK`] alongside [`KILL_GRACE`].** The worst case
/// on Windows is 2.5 s of services, 0.5 s of escalation, 0.25 s here, 0.5 s of clients and 1 s of
/// checkpoint — 4.75 s of the 5 s the OS allows. That it fits is a test rather than this sentence.
const CONFIRMATION_REPRIEVE: Duration = Duration::from_millis(250);

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

    /// Read the package index from here instead of the one MixEngine publishes.
    ///
    /// A team mirror, or a test's own registry. `.claude/operations/runtime-packaging.md` promises
    /// this and promises that the signature requirement stays — which is why it is useless without
    /// the flag below, and why the two are read together.
    #[arg(long, env = "MIXENGINE_INDEX_URL", value_name = "URL")]
    index_url: Option<String>,

    /// Verify that index against this minisign public key instead of the compiled-in one.
    ///
    /// **Overriding it is trusting a different publisher**, and nothing about that is hidden: only
    /// somebody who already controls how this daemon starts can set it, and a daemon started with it
    /// says so in its log. The alternative — a URL that can move while the key cannot — would be a
    /// mirror setting that can only ever fail, since nobody else can sign with our key.
    #[arg(
        long,
        env = "MIXENGINE_INDEX_KEY",
        value_name = "KEY",
        requires = "index_url"
    )]
    index_key: Option<String>,
}

impl Args {
    /// Where the package index comes from: what was asked for, or what MixEngine publishes.
    ///
    /// The one place either value is read. Configuration enters at `main` and is passed down —
    /// `.claude/standards/rust.md` — so nothing below this reaches for an environment variable to
    /// find out where to download from.
    fn index_source(&self) -> runtimes::IndexSource {
        let default = runtimes::IndexSource::default();

        runtimes::IndexSource {
            url: self.index_url.clone().unwrap_or(default.url),
            public_key: self.index_key.clone().unwrap_or(default.public_key),
        }
    }
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
    // Read here rather than inside `serve`, on the same rule the flags follow: the environment and
    // the process's own identity enter the program at `main`. What it is for is finding
    // `mixengine-shim`, which ships beside this binary — see [`mixengine_core::shims::source`].
    let program = std::env::current_exe().context("cannot find the running mixengined binary")?;

    let served = serve(
        &home.paths,
        &store,
        started,
        &endpoint,
        &home.config,
        &args.index_source(),
        program,
    )
    .await;

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

    // The same rule again, and it matters more here than for the log format: a mirror named on the
    // command line has to reach the child, or a `--detach`ed daemon would quietly go back to the
    // published index and refuse the mirror's signature — which is the one failure a person setting
    // these would have no way of explaining.
    if let Some(url) = &args.index_url {
        arguments.push("--index-url".into());
        arguments.push(url.into());
    }
    if let Some(key) = &args.index_key {
        arguments.push("--index-key".into());
        arguments.push(key.into());
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
    config: &config::Config,
    index: &runtimes::IndexSource,
    program: PathBuf,
) -> anyhow::Result<()> {
    // The two settings this function spends, read out of the file `main` loaded. One argument
    // rather than two, because a seventh would put this over the count clippy allows and because
    // the next task to want a key would have added an eighth.
    let shutdown_grace = Duration::from_secs(config.daemon.shutdown_grace_seconds);

    // **`<root>/bin` is refreshed on every start** — roadmap task T26. It is a projection of a table
    // compiled into this binary, exactly as `etc/` is a projection of the database, so a home whose
    // `bin/` was emptied is repaired by starting the daemon. Touching nothing outside the root is
    // what separates it from putting that directory on the user's PATH — that is `path.install`'s,
    // and is only ever done when somebody asks.
    //
    // **Before the endpoint is bound**, unlike the two recovery passes below, and the reason is the
    // endpoint rather than the work: a bound listener that is not yet in `accept` has exactly one
    // pending connection on Windows, so every moment between the two is a moment a second client
    // meets `ERROR_PIPE_BUSY`. Recovery is a database read and a handful of process lookups;
    // nineteen file copies are not, and putting them after the bind made an ordinary parallel test
    // run fail. Nothing is listening yet while this runs, and a client that finds nothing there
    // retries — which is the same thing it does for the migrations that ran a moment ago.
    //
    // Nothing here fails the start, on the rule the recovery passes follow: a `bin/` that could not
    // be written leaves a home whose shims are missing, which a person can see and act on, where
    // refusing to start would leave them with no daemon at all.
    let shims = Arc::new(shims::Shims::new(
        paths,
        program.clone(),
        mixengine_platform::host(),
    ));

    match shims.refresh() {
        Ok(refreshed) if refreshed.written.is_empty() && refreshed.removed.is_empty() => {
            tracing::debug!(commands = refreshed.commands.len(), "bin/ is up to date");
        }
        Ok(refreshed) => tracing::info!(
            written = ?refreshed.written,
            removed = ?refreshed.removed,
            refused = ?refreshed.refused,
            "filled bin/ with one shim per command"
        ),
        Err(error) => tracing::warn!(
            %error,
            "could not fill bin/ — the commands in it may be missing or out of date"
        ),
    }

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

    // **The source is the generator** — roadmap task T30. Every `service.*` call renders the
    // `services` table into `etc/` and into the specs the registry supervises, which is also why
    // there is nothing to do here beyond handing it the two things it reads: a home with no services
    // in it declares nothing, and the registry, the graph and the walk all handle that without a
    // special case.
    // **Before the registry**, which takes it — roadmap task T33. A service that has never been
    // started here may have a first-run ritual to perform, and that is minutes of work reported
    // through a job rather than something a `service.start` can do inline.
    let jobs = Arc::new(jobs::Jobs::new(store, events.clone(), shutdown.clone()));

    // One host for both, rather than two: `declared` asks it what this system makes a front end
    // bind, and the registry keeps it for everything else.
    let host = mixengine_platform::host();

    let services = Arc::new(services::Registry::new(
        paths,
        store,
        Arc::clone(&host),
        events.clone(),
        services::declared(paths, store, host.as_ref()),
        shutdown.clone(),
        Arc::clone(&jobs),
    ));

    // **The DNS server, and the mode it puts this home in** — roadmap task T44. Started here, after
    // the host and before the queue that reads its mode: `require_hosts` asks whether this home
    // still needs a hosts file at all, and until T45 wires a resolver the answer is always yes.
    //
    // Nothing here fails the start, on the rule every block around it follows — a port somebody else
    // is holding is a mode and a sentence on `mix status`, not a machine with no daemon.
    let dns = Arc::new(dns::Dns::start(&config.dns, host.as_ref(), shutdown.clone()).await);

    // **Read once, reported, never refused** — the T40b design, D10. Refusing to start is what
    // ADR 0005's first sentence seems to demand and is wrong here for a measured reason: CI's whole
    // Windows third runs the daemon suites under a full administrative token (T2b), and a hard
    // refusal would turn one of three platforms red for a reason that has nothing to do with the
    // code under test. What is worth saying about it is not that this daemon cannot elevate — it is
    // that every service it supervises inherits the token.
    let elevation = elevation::Elevation::new(
        paths,
        store,
        events.clone(),
        Arc::clone(&jobs),
        mixengine_platform::host(),
        program,
        Arc::clone(&dns),
    );

    if mixengine_platform::elevated::is_elevated() {
        tracing::warn!(
            "this daemon holds an administrative token — every service it supervises inherits it, \
             and writes files into this home as an administrator. `mix status` says so too."
        );
    }

    // **Before the first client, and after the listener is bound** — roadmap task T18. A daemon that
    // was killed leaves rows claiming a supervisor that no longer exists, and until they are
    // reconciled `service.list` would report a machine that does not exist: services running with
    // nothing behind them. Doing it here rather than earlier costs nothing and buys the ordering
    // that matters, which is that the single-instance lock is long since held: no second daemon can
    // be looking at these rows, and nothing that survived can be adopted twice.
    //
    // Nothing here fails the start. A survivor that cannot be stopped, a row that cannot be
    // cleared, a source that cannot say what is declared — each is reported and each leaves one
    // service in a state a user can see and act on, where refusing to start would leave them with a
    // machine that has no daemon at all.
    //
    // **Stale endpoint files are not part of it**, although the architecture document lists them in
    // the same sentence. `ipc::Listener::bind` already unlinks a socket nothing answers on and binds
    // again (T7), and there is no pid file to go stale: `run/mixengined.lock` is held as an open
    // handle the OS releases even when the daemon is killed, so the file surviving means nothing and
    // its contents are rewritten by whoever takes the lock next (T9).
    let recovered = services.recover().await;

    if recovered.is_empty() {
        tracing::debug!("nothing was left running by a previous daemon");
    } else if recovered.refused.is_empty() {
        tracing::info!(
            adopted = ?recovered.adopted,
            stopped = ?recovered.stopped,
            cleared = ?recovered.cleared,
            "reconciled what the last daemon left behind"
        );
    } else {
        // **Warn rather than info, and said differently**, because this is the one boot where the
        // sentence above would be untrue: something the last daemon left behind is still running,
        // still holding whatever it held, and its row still names it. Each of them has its own
        // `error!` from the registry saying which and why; this is the line that stops the summary
        // from reading like a clean start.
        tracing::warn!(
            adopted = ?recovered.adopted,
            stopped = ?recovered.stopped,
            cleared = ?recovered.cleared,
            refused = ?recovered.refused,
            "could not reconcile everything the last daemon left behind; the services listed as \
             refused are still running with nothing supervising them"
        );
    }

    // **Every start asks whether this machine will still let the front end answer on 80 and 443** —
    // roadmap task T42, and the re-probe T88b asked for. A capability is cleared by any write to the
    // binary, so an update loses it and the next start is what notices; the answer costs one read
    // and no privilege. A home with no front end asks for nothing.
    //
    // After recovery, because that is what has just rendered the graph this reads the program path
    // out of. Nothing here fails the start, on the rule every block around it follows: a machine
    // that was not asked is one command away from being asked, where refusing to start would leave
    // the user with no daemon at all.
    if let Err(error) = elevation
        .require_port_access(services.front_end_program().await.as_deref())
        .await
    {
        tracing::warn!(%error, "could not ask for permission to answer on 80 and 443");
    }

    // **And every start asks whether this machine still sends its managed TLDs here** — roadmap
    // task T45, and the same shape as the block above for the same reason: reading the wiring costs
    // one file or one registry key and no privilege, so asking on every start is what notices a
    // resolver an OS update, another home, or a person removed.
    //
    // **Here, before any site exists, and that ordering is the point** (the T45 design, D7). On a
    // fresh home this puts the operation in the queue in time for first-run setup's single grant;
    // asking after the first site was created would mean emptying a hosts block that already had a
    // line in it, which is a second operation and therefore a second prompt.
    if let Err(error) = elevation.require_resolver().await {
        tracing::warn!(%error, "could not ask for this machine's managed TLDs to be routed here");
    }

    // **Every installed runtime gets the service its recipe says it should have** — roadmap task
    // T32. Idempotent and run here as well as after an install, which is what gives a PHP installed
    // by an earlier build its pool with no data migration and repairs a home whose row somebody
    // removed by hand. Nothing here fails the start, on the same rule the two blocks around it
    // follow: a runtime with no service is one command away from having one, where refusing to start
    // would leave the user with no daemon at all.
    match mixengine_core::services::pools::ensure(
        store,
        mixengine_platform::host().as_ref(),
        &services::catalogue(),
    )
    .await
    {
        Ok(created) if created.is_empty() => {
            tracing::debug!("every installed runtime already has the service it needs");
        }
        Ok(created) => tracing::info!(pools = ?created, "installed runtimes were given services"),
        Err(error) => tracing::warn!(%error, "could not give every installed runtime its service"),
    }

    // **And every installed runtime's ini set** — roadmap task T28, on the same policy as `bin/`
    // above: `etc/` is a projection of the database, so it is rebuilt here rather than trusted, and
    // a home whose `etc/php/` was deleted is repaired by starting the daemon. Nothing here fails the
    // start either.
    match mixengine_core::runtimes::extensions::refresh_all(store, paths).await {
        Ok(moved) if moved.is_empty() => {
            tracing::debug!("every installed runtime's conf.d is up to date");
        }
        Ok(moved) => {
            tracing::info!(runtimes = ?moved, "rewrote the generated conf.d of installed runtimes");
        }
        Err(error) => tracing::warn!(%error, "could not rebuild every installed runtime's conf.d"),
    }

    // **The other half of recovery, and it needs no OS reading at all** — roadmap task T22. A
    // service is a process that can outlive the daemon that spawned it, which is why the step above
    // asks the OS what survived; the work behind a job is a task *inside* this process, so a row
    // still saying `running` means one thing only: the daemon doing it stopped. There is nothing to
    // adopt and nothing to signal, only a row to close, and it is closed as a failure because nobody
    // asked for the work to stop.
    //
    // Before the first client for the same reason as above: a `job.list` answered before this ran
    // would show work nobody is doing.
    match mixengine_core::jobs::abandon(
        store,
        mixengine_proto::Timestamp::from_system_time(std::time::SystemTime::now()),
    )
    .await
    {
        Ok(abandoned) if abandoned.is_empty() => {
            tracing::debug!("no jobs were left unfinished by a previous daemon");
        }
        Ok(abandoned) => tracing::info!(jobs = abandoned.len(), "closed jobs nobody is doing"),

        // Nothing here fails the start, on the same rule the service half follows: a row that could
        // not be closed leaves one job a user can see and act on, where refusing to start would
        // leave them with no daemon at all.
        Err(error) => tracing::warn!(%error, "could not close the jobs a previous daemon left"),
    }

    // **Fails the start rather than the first call** (roadmap task T23). What can go wrong here is a
    // public key that is not one — the compiled-in constant, or an `--index-key` somebody pasted
    // half of — and a daemon that will refuse every install for the rest of its life should say so
    // while the person who started it is still watching.
    let fetcher =
        runtimes::Fetcher::new(paths, index).map_err(|error| anyhow::anyhow!("{error}"))?;
    let runtimes = runtimes::Runtimes::new(paths, store, Arc::clone(&jobs), Arc::clone(&fetcher));
    let packages = packages::Packages::new(paths, store, Arc::clone(&jobs), fetcher);

    if index.url != mixengine_core::index::DEFAULT_URL {
        // Worth a line of its own: from here on this daemon trusts a publisher that is not us, and
        // the log is where somebody debugging a refused signature will look for that fact.
        tracing::info!(
            url = index.url,
            "reading the package index from somewhere other than the published one"
        );
    }

    // Built after the listener rather than before it, so `daemon.status` reports the endpoint that
    // was actually bound instead of the one that would be computed again now.
    let api = api::Api::new(
        paths,
        store,
        endpoint,
        started,
        events,
        api::Supervision {
            services: Arc::clone(&services),
            jobs: Arc::clone(&jobs),
            runtimes,
            packages,
            shims,
            elevation: Arc::clone(&elevation),
            dns,
        },
        api::Shutdown::new(shutdown.clone(), shutdown_grace),
    );

    tracing::info!(endpoint = %endpoint, "listening for clients");

    // Connections are tracked rather than detached, because `.claude/standards/rust.md` forbids a
    // task that outlives shutdown and because a `/events` stream would otherwise be cut mid-frame.
    let mut connections = tokio::task::JoinSet::new();

    // Which of the two shutdowns this turned out to be, kept because the connections' grace differs
    // between them and nothing below can tell them apart afterwards — see `SIGNAL_CLIENT_GRACE`. It
    // is set on every path where the OS has started counting, including a console event that arrives
    // during a `daemon.shutdown` that was already under way.
    let mut on_the_os_clock = false;

    loop {
        tokio::select! {
            // Ctrl-C, `systemctl --user stop`, the console closing, the machine shutting down —
            // whichever of them this OS has. Cancel safe, so a turn that serves a client instead
            // has not swallowed one.
            stop = signals.stopped() => {
                // **The budget is granted before the token is cancelled, and that order is the
                // whole of the signal half of T9a.** Cancelling first would release every runner
                // into the stop its spec asks for, with nothing having said how long the *daemon*
                // has — and on Windows the OS is already counting.
                let budget = signalled_budget(shutdown_grace);

                tracing::info!(%stop, ?budget, "shutting down");
                on_the_os_clock = true;
                services.stopping_within(budget);
                shutdown.cancel();
                break;
            }

            // Cancelled by something inside the daemon rather than by the OS: `daemon.shutdown`,
            // which has already stopped the services in dependency order and granted its own budget
            // before it got here (T9a). The loop understands both, and neither is the only one.
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
    //
    // **Pinned and selected against rather than simply awaited, because this wait is the one part of
    // a shutdown a person can be trapped in** — roadmap task T9a. Leaving the accept loop used to be
    // the last time anything read `signals`, although the handlers stay installed for the rest of
    // this function: on Unix tokio's `SIGINT`/`SIGTERM` handlers are process-global and permanent,
    // so the default disposition is gone, and a second Ctrl-C during a stop that had wedged on one
    // service was delivered into a channel nobody was reading and did nothing whatever. The only
    // escape left was `kill -9`, which is the outcome this task exists to remove.
    // **Jobs stop beside the services rather than after them** — roadmap task T22. The root token
    // has already cancelled both, and what is left is the waiting: a service holds a port and a data
    // directory, a job holds a staging directory it is the only thing that can remove. Neither wait
    // is shortened by the other finishing, so running them in sequence would add one budget to the
    // other — which is the arithmetic T9a's single budget exists to prevent. A job that will not
    // stop inside it is left, and its row is the next daemon's `abandon` to close.
    let mut stopping = std::pin::pin!(async {
        tokio::join!(services.shut_down(), jobs.shut_down(shutdown_grace));
    });

    tokio::select! {
        () = &mut stopping => {}

        // **An escalation and not an exit.** Returning from `serve` here would skip `Store::close`
        // and leave behind the `-wal` sidecar every number above is sized around — one bad outcome
        // traded for a worse one. What a second request means is that the person is no longer
        // willing to wait for the polite stop, so the budget is narrowed to nothing and every runner
        // still to reach a stop goes straight to the kill: `Registry::stopping_within` is the
        // narrow-only mechanism T9a already built for a second shutdown arriving during a first, and
        // a second answer to the same question here would be one for them to disagree about.
        //
        // Then the *same* future is waited on again rather than dropped, because dropping it detaches
        // the runners it drained out of the registry and the children they own are orphaned instead
        // of reaped — the thing the wait was for. Bounded by `KILL_GRACE` so that a third signal is
        // never the answer.
        stop = signals.stopped() => {
            on_the_os_clock = true;

            tracing::warn!(
                %stop,
                grace = ?KILL_GRACE,
                "asked to stop again while services were still stopping; killing them now"
            );

            services.stopping_within(Duration::ZERO);

            if tokio::time::timeout(KILL_GRACE, &mut stopping).await.is_err() {
                tracing::warn!(
                    "some services were still stopping when the escalated shutdown stopped waiting \
                     for them; whatever they had left goes with this process"
                );
            }
        }
    }

    shut_down(
        connections,
        if on_the_os_clock {
            SIGNAL_CLIENT_GRACE
        } else {
            CLIENT_GRACE
        },
    )
    .await;

    Ok(())
}

/// What a shutdown the *operating system* asked for may spend on services — roadmap task **T9a**.
///
/// **One budget, two ceilings.** `daemon.shutdown` arrives over a socket with nothing counting
/// against it and gets the configured number entire; a console control event on Windows arrives with
/// about five seconds already ticking, and a daemon that spent the configured ten would be
/// terminated somewhere in the middle of a database it had asked to flush — the worst of both, since
/// the polite stop was begun and not finished.
///
/// So where the OS states a ceiling this takes the smaller of the two, minus what the rest of the
/// shutdown still has to do afterwards ([`CEILING_RESERVE`]). Where it states none — every Unix, and
/// the `--detach`ed Windows daemon that has no console for an event to arrive on — the configured
/// budget is the whole answer, because nothing else is going to end this process early.
///
/// Saturating rather than clamped to a minimum: a machine whose ceiling is smaller than the reserve
/// is one where services get nothing and are killed at once, which is the honest outcome and is what
/// the row and the log then say. Inventing a floor there would spend time the OS has already decided
/// this process does not have.
fn signalled_budget(configured: Duration) -> Duration {
    match signal::STOP_CEILING {
        Some(ceiling) => configured.min(ceiling.saturating_sub(CEILING_RESERVE)),
        None => configured,
    }
}

/// Let the connections that are still open finish, then stop waiting.
///
/// A grace period rather than an abort, because a client mid-request has already been told the
/// daemon accepted it — and rather than an unbounded wait, because a connection is kept alive
/// between requests: a client that has ended its `GET /events` (the root token does that as this is
/// called) may still be holding a socket nobody is going to write to again. Dropping the set at the
/// end aborts whatever is left, which for a connection with no request in flight is exactly right.
///
/// `grace` is [`CLIENT_GRACE`] or the shorter [`SIGNAL_CLIENT_GRACE`], and which of the two it is,
/// is the whole of what the two ways of shutting down differ by from here on: one of them has an
/// answer to write into a connection and the other arrived with an OS clock already running. Passed
/// rather than read from a constant inside, because the caller is the only thing that knows which
/// shutdown this is.
async fn shut_down(mut connections: tokio::task::JoinSet<()>, grace: Duration) {
    if connections.is_empty() {
        return;
    }

    tracing::info!(
        open = connections.len(),
        ?grace,
        "waiting for clients to finish"
    );

    if tokio::time::timeout(grace, async {
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

#[cfg(test)]
mod tests {
    use super::*;

    /// The "two ceilings" half of T9a, asserted as the one rule rather than as three per-OS numbers.
    ///
    /// Written this way on purpose: a test that said "2.5 seconds on Windows" would be a second copy
    /// of `STOP_CEILING` in the daemon, which is exactly the `#[cfg]` the platform layer exists to
    /// keep out of here. What is checked is the relationship — a ceiling shortens the budget and
    /// leaves room for what comes after the services, and no ceiling leaves it alone.
    #[test]
    fn a_shutdown_the_operating_system_asked_for_fits_inside_whatever_clock_it_started() {
        let configured = Duration::from_secs(10);
        let budget = signalled_budget(configured);

        match signal::STOP_CEILING {
            Some(ceiling) => {
                assert!(
                    budget + CEILING_RESERVE <= ceiling,
                    "the connections and the WAL checkpoint happen after the services stop, and \
                     this budget leaves no room for them: {budget:?} of {ceiling:?}"
                );
                assert!(budget < configured, "a ceiling shortens the budget");
            }

            // Nothing is counting, so the configured budget is the whole answer and shortening it
            // would kill a database that had every right to finish flushing.
            None => assert_eq!(budget, configured),
        }
    }

    /// The margin the reserve is now made of, asserted as the relation and not as its parts.
    ///
    /// The defect this pins is that the old reserve left none: 2.5 s of services, 2 s of clients and
    /// what was left for the checkpoint added up to the ceiling exactly, so any of the three running
    /// a moment over meant a process terminated mid-checkpoint — the one thing the reserve exists to
    /// prevent. **Strictly less, not at most**, because "adds up to exactly the ceiling" is precisely
    /// the arrangement that was wrong, and a reserve that is only ever spent in full is a reserve in
    /// name.
    #[test]
    fn a_shutdown_the_operating_system_asked_for_leaves_slack_over_after_the_checkpoint() {
        let budget = signalled_budget(Duration::from_secs(10));

        // Only where a clock is running. Where none is — every Unix — there is no ceiling for
        // anything to fit inside, and what this test still has to say is the clause below it, which
        // holds on all three systems.
        if let Some(ceiling) = signal::STOP_CEILING {
            assert!(
                budget + SIGNAL_CLIENT_GRACE + CHECKPOINT_MARGIN < ceiling,
                "the clients and the WAL checkpoint follow the services, and what this budget \
                 leaves for them is the whole of the rest of the ceiling: {budget:?} of {ceiling:?}"
            );
        }

        assert!(
            SIGNAL_CLIENT_GRACE + CHECKPOINT_MARGIN < CEILING_RESERVE,
            "the reserve is supposed to keep back more than the two waits it is spent on; \
             {SCHEDULING_SLACK:?} of it is meant to be left over"
        );
    }

    /// The escalation half of T9a: a second request to stop is paid for out of the slack.
    ///
    /// Asserted against [`SCHEDULING_SLACK`] rather than against a ceiling, so that it is a claim
    /// every OS can check — the arithmetic it stands for is that a budget spent to its last
    /// millisecond, plus this wait, plus the clients and the checkpoint, still ends inside whatever
    /// clock the OS started. The version with the ceiling in it follows for the one system that has
    /// one.
    #[test]
    fn a_second_request_to_stop_costs_less_than_the_reserve_keeps_in_hand() {
        assert!(
            KILL_GRACE + CONFIRMATION_REPRIEVE < SCHEDULING_SLACK,
            "an escalated shutdown waits {KILL_GRACE:?}, and a stop watching a killed survivor \
             {CONFIRMATION_REPRIEVE:?}, on top of a budget that may already be spent, and only \
             {SCHEDULING_SLACK:?} of the reserve is not already promised to something"
        );

        if let Some(ceiling) = signal::STOP_CEILING {
            let budget = signalled_budget(Duration::from_secs(10));

            assert!(
                budget
                    + KILL_GRACE
                    + CONFIRMATION_REPRIEVE
                    + SIGNAL_CLIENT_GRACE
                    + CHECKPOINT_MARGIN
                    < ceiling,
                "a second Ctrl-C at the last instant of the budget must still leave the checkpoint \
                 inside {ceiling:?}"
            );
        }
    }

    #[test]
    fn a_configured_budget_smaller_than_the_ceiling_is_still_the_one_that_applies() {
        // The ceiling is what the OS *allows*, not what a shutdown is entitled to take. Somebody who
        // set a second in `config.toml` gets a second on every system, which is the whole reason
        // this is a `min` of the two rather than "the ceiling wherever there is one".
        assert_eq!(
            signalled_budget(Duration::from_millis(1)),
            Duration::from_millis(1)
        );
    }
}
