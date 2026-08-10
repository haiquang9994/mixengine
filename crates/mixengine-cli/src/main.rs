//! `mix` — the reference client. It renders what the daemon returns and decides nothing itself.

use clap::{Parser, Subcommand};

#[derive(Debug, Parser)]
#[command(name = "mix", version, about = "MixEngine command line")]
struct Args {
    /// Emit machine-readable JSON instead of the human-facing rendering.
    #[arg(long, global = true)]
    json: bool,

    #[command(subcommand)]
    command: Command,
}

#[derive(Debug, Subcommand)]
enum Command {
    /// Show the daemon's health, version and what it is currently running.
    Status,
}

fn main() -> anyhow::Result<()> {
    let args = Args::parse();

    match args.command {
        // The transport (T7) and the client that speaks over it (T10) are not built yet, so there is
        // no daemon to ask. Report the protocol this build would negotiate and stop there.
        Command::Status => {
            if args.json {
                // Always serialised, never formatted by hand: `--json` is a machine contract.
                let payload = serde_json::json!({
                    "protocol": mixengine_proto::PROTOCOL_VERSION,
                    "daemon": serde_json::Value::Null,
                });
                println!("{}", serde_json::to_string(&payload)?);
            } else {
                println!(
                    "mix {} (protocol {}) — no daemon connection in this build yet",
                    env!("CARGO_PKG_VERSION"),
                    mixengine_proto::PROTOCOL_VERSION
                );
            }
        }
    }

    Ok(())
}
