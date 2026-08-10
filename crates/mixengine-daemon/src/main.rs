//! `mixengined` — the only process that owns state. Clients are thin; this is not.

mod error;
mod logging;

use std::io::IsTerminal;
use std::path::PathBuf;

use anyhow::Context as _;
use clap::{Parser, ValueEnum};
use mixengine_core::config;

use error::ToWire as _;

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
    let store = mixengine_core::Store::open(home.paths.database_file())
        .await
        .map_err(|error| error.to_wire())?;

    tracing::info!(database = %store.file().display(), "database open and up to date");

    // Everything that runs with the database open lives in `serve`, and its result is held rather
    // than propagated with `?`, so that the close below is on the only way out. There is nothing
    // after the open that can fail *today* — but the transport (T7) and the server (T8) go inside
    // that call, and a `?` among them would skip the checkpoint on exactly the exits that matter.
    let served = serve(&store).await;

    // Awaited rather than dropped: closing the pool checkpoints the write-ahead log, which is what
    // leaves a single file behind instead of one with a `-wal` sidecar holding the newest commits.
    store.close().await;

    served
}

/// What the daemon does while its state is open.
///
/// Separate from `main` so that `Store::close` has a single call site that every exit passes
/// through, including the failing ones.
async fn serve(_store: &mixengine_core::Store) -> anyhow::Result<()> {
    // The state is there, but the IPC transport (T7) and the API server (T8) are not, so there is
    // nothing to serve and no reason to linger.
    tracing::warn!("no API server in this build yet — see .claude/roadmap/todo.md tasks T7-T8");

    Ok(())
}
