//! `mixengined` — the only process that owns state. Clients are thin; this is not.

use std::io::IsTerminal;
use std::path::PathBuf;

use clap::{Parser, ValueEnum};
use mixengine_core::config;

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

fn tracing_level(level: config::LogLevel) -> tracing::Level {
    match level {
        config::LogLevel::Error => tracing::Level::ERROR,
        config::LogLevel::Warn => tracing::Level::WARN,
        config::LogLevel::Info => tracing::Level::INFO,
        config::LogLevel::Debug => tracing::Level::DEBUG,
        config::LogLevel::Trace => tracing::Level::TRACE,
    }
}

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    let args = Args::parse();

    // Before anything else: find the home directory, read config.toml, create what is missing.
    // It happens before logging is set up because the log level is one of the things it reads —
    // a failure here is reported by `main` returning it, not by a logger that does not exist yet.
    let host = mixengine_platform::host();
    let home = mixengine_core::open_home(args.home.as_deref(), host.as_ref())?;

    let level = args
        .log_level
        .map_or(home.config.log.level, config::LogLevel::from);

    // Deliberately minimal: file sinks, rotation and `MIXENGINE_LOG_FORMAT=json` are task T4.
    // Colour only when a human is watching — the daemon normally runs with its output redirected,
    // and escape codes baked into `daemon.log` make "copy diagnostics" (T66) useless.
    tracing_subscriber::fmt()
        .with_max_level(tracing_level(level))
        .with_writer(std::io::stderr)
        .with_ansi(std::io::stderr().is_terminal())
        .init();

    tracing::info!(
        version = env!("CARGO_PKG_VERSION"),
        protocol = %mixengine_proto::PROTOCOL_VERSION,
        home = %home.paths.root().display(),
        "mixengined starting"
    );

    if home.config.log.format == config::LogFormat::Json {
        tracing::warn!("log.format = \"json\" is not honoured yet (T4) — logging as text");
    }

    if !args.foreground {
        tracing::warn!("detaching is not implemented yet (T9) — staying in the foreground");
    }

    // The home directory exists and is configured, but the store (T6), the IPC transport (T7) and
    // the API server (T8) do not, so there is nothing to serve and no reason to linger.
    tracing::warn!("no API server in this build yet — see .claude/roadmap/todo.md tasks T6-T8");

    Ok(())
}
