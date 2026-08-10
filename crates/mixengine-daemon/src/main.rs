//! `mixengined` — the only process that owns state. Clients are thin; this is not.

use std::io::IsTerminal;

use clap::{Parser, ValueEnum};

/// Command line of the daemon. Configuration enters the program here and is passed down; nothing
/// deeper reads the environment on its own.
#[derive(Debug, Parser)]
#[command(name = "mixengined", version, about = "MixEngine daemon")]
struct Args {
    /// Stay in the foreground instead of detaching (the default while developing).
    ///
    /// Detaching itself is task T9; until then this build always stays in the foreground and says
    /// so rather than letting the caller believe it forked.
    #[arg(long)]
    foreground: bool,

    /// How much to log.
    #[arg(long, value_enum, default_value = "info")]
    log_level: LogLevel,
}

/// Verbosity of the daemon log.
///
/// A closed set on purpose: a free-form string would let a typo silence logging entirely, and the
/// process would start looking perfectly healthy while saying nothing.
#[derive(Debug, Clone, Copy, ValueEnum)]
enum LogLevel {
    Error,
    Warn,
    Info,
    Debug,
    Trace,
}

impl From<LogLevel> for tracing::Level {
    fn from(level: LogLevel) -> Self {
        match level {
            LogLevel::Error => Self::ERROR,
            LogLevel::Warn => Self::WARN,
            LogLevel::Info => Self::INFO,
            LogLevel::Debug => Self::DEBUG,
            LogLevel::Trace => Self::TRACE,
        }
    }
}

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    let args = Args::parse();

    // Deliberately minimal: file sinks, rotation and `MIXENGINE_LOG_FORMAT=json` are task T4.
    // Colour only when a human is watching — the daemon normally runs with its output redirected,
    // and escape codes baked into `daemon.log` make "copy diagnostics" (T66) useless.
    tracing_subscriber::fmt()
        .with_max_level(tracing::Level::from(args.log_level))
        .with_writer(std::io::stderr)
        .with_ansi(std::io::stderr().is_terminal())
        .init();

    tracing::info!(
        version = env!("CARGO_PKG_VERSION"),
        protocol = %mixengine_proto::PROTOCOL_VERSION,
        "mixengined starting"
    );

    if !args.foreground {
        tracing::warn!("detaching is not implemented yet (T9) — staying in the foreground");
    }

    // The workspace is scaffolded but empty: paths (T3), the store (T6), the IPC transport (T7) and
    // the API server (T8) do not exist yet, so there is nothing to serve and no reason to linger.
    tracing::warn!("no API server in this build yet — see .claude/roadmap/todo.md tasks T3-T8");

    Ok(())
}
