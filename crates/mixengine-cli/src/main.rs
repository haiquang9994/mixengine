//! `mix` — the reference client. It renders what the daemon returns and decides nothing itself.
//!
//! Every command here is one RPC and one rendering. That is the rule from
//! [CLAUDE.md](../../../CLAUDE.md) — *no business logic in clients* — and it is why this binary is
//! shaped the way it is: the only decisions it makes on its own are which home it is talking about,
//! whether to start a daemon that is not running, and how to put the answer on screen. Everything
//! else is `mixengined`'s, including the wording of every failure.
//!
//! **Failures are the wire error, always.** Whether the daemon refused a call or `mix` never
//! reached one, what comes out is a `mixengine_proto::Error` — a stable code, one sentence, and a
//! hint where there is something to do. A script gets the same object out of `--json` in both cases
//! and can branch on `code` without caring which side of the socket produced it.

mod autostart;
mod client;
mod error;
mod home;
mod render;

use std::io::Write as _;
use std::path::PathBuf;
use std::process::ExitCode;

use clap::{Parser, Subcommand};
use mixengine_platform::ipc::Endpoint;
use mixengine_proto::{DaemonStatus, Error, ErrorCode, rpc};

use autostart::Autostart;
use client::Client;

/// Command line of the client. Configuration enters the program here and is passed down; nothing
/// deeper reads the environment on its own.
#[derive(Debug, Parser)]
#[command(name = "mix", version, about = "MixEngine command line")]
struct Args {
    /// Root directory of the MixEngine installation to talk to.
    ///
    /// Defaults to the OS convention, exactly as `mixengined` resolves it — the two have to agree
    /// or they would be talking about different daemons.
    #[arg(long, global = true, env = "MIXENGINE_HOME", value_name = "DIR")]
    home: Option<PathBuf>,

    /// Emit machine-readable JSON instead of the human-facing rendering.
    #[arg(long, global = true)]
    json: bool,

    /// Fail instead of starting a daemon when none is running for this home.
    ///
    /// `mix` normally starts one, which is what makes the first command a person types work. The
    /// flag is for the caller that wants a question answered rather than a machine changed: a
    /// monitoring check, or a CI step that should not create a home as a side effect of asking
    /// whether one is there.
    #[arg(long, global = true)]
    no_autostart: bool,

    #[command(subcommand)]
    command: Command,
}

#[derive(Debug, Subcommand)]
enum Command {
    /// Show the daemon's health, version and what it is currently running.
    Status,
}

fn main() -> ExitCode {
    let args = Args::parse();
    let json = args.json;

    match run(args) {
        Ok(()) => ExitCode::SUCCESS,
        Err(error) => {
            report(&error, json);
            ExitCode::FAILURE
        }
    }
}

/// Everything the command does, with one way out for a failure.
///
/// `current_thread`, because a client sends one request and exits: the multi-thread runtime the
/// daemon needs would be several worker threads started to wait on a single socket, paid for on
/// every `mix` invocation in a shell prompt.
#[tokio::main(flavor = "current_thread")]
async fn run(args: Args) -> Result<(), Error> {
    let host = mixengine_platform::host();
    let root = home::resolve_root(args.home.as_deref(), host.as_ref())?;
    let endpoint = home::endpoint(&root)?;

    // Prepared either way, and not because it is free: deciding *whether* to autostart here rather
    // than inside the client is what keeps "this run may start a daemon" a property of the command
    // line and not of a code path somewhere below.
    let autostart = (!args.no_autostart).then(|| Autostart::for_home(&root));

    // Dialled by the command and not here. Every command there is today needs a daemon, but that is
    // a fact about `status` rather than about `mix` — `mix doctor` (T47) has to be able to describe
    // a home that has none — and connecting above the match would have made starting one the first
    // thing every future command did, whether or not it had anything to ask.
    match args.command {
        Command::Status => status(&endpoint, autostart.as_ref(), args.json).await,
    }
}

/// `mix status`: what the daemon says about itself.
async fn status(
    endpoint: &Endpoint,
    autostart: Option<&Autostart>,
    json: bool,
) -> Result<(), Error> {
    let mut client = Client::connect(endpoint, autostart).await?;

    let method = rpc::method::DAEMON_STATUS;
    let result = client.call(method, None).await?;

    // Decoded rather than printed as the `Value` it arrived as, even for `--json`. The handshake has
    // already established that this daemon speaks our protocol, so a field this build cannot read
    // is a bug worth reporting as one — and `--json` promising `DaemonStatus` means it has to be a
    // `DaemonStatus` that goes out.
    let status: DaemonStatus = serde_json::from_value(result).map_err(|error| {
        Error::new(
            ErrorCode::Internal,
            format!(
                "mix {} cannot read the answer to {method} from mixengined {}: {error}",
                env!("CARGO_PKG_VERSION"),
                client.daemon().version
            ),
        )
    })?;

    match json {
        // The newline is here rather than in `render`, which builds a document and does not know
        // whether it is the last thing on the stream. The human rendering ends in one already.
        true => emit(&format!("{}\n", render::status_json(&status))),
        false => emit(&render::status(&status)),
    }
}

/// Put the command's answer on stdout.
///
/// `write!` and not `print!`, for the reason [`report`] gives for stderr — the macro panics when the
/// write fails — but the two failures it can meet are not the same failure and are not answered the
/// same way. A reader that went away, `mix status | head -1`, is not this program's problem and is
/// what every well-behaved tool exits quietly on. Anything else — a full disk, a handle closed
/// before the process started — is a command that did not deliver its answer, and says so in the
/// same wire error every other failure here uses.
///
/// Flushed explicitly, because the lock is a `LineWriter`: a rendering that reaches the buffer and
/// no further would otherwise fail on drop, where the error is discarded and this run would have
/// exited zero having printed nothing.
fn emit(rendered: &str) -> Result<(), Error> {
    let mut stdout = std::io::stdout().lock();

    stdout
        .write_all(rendered.as_bytes())
        .and_then(|()| stdout.flush())
        .or_else(|source| match source.kind() {
            std::io::ErrorKind::BrokenPipe => Ok(()),
            _ => Err(Error::new(
                ErrorCode::Io,
                format!("cannot write to stdout: {source}"),
            )),
        })
}

/// Put a failure where the person or the program running `mix` will find it.
///
/// **stderr, in both renderings.** stdout carries the command's answer and nothing else, so a script
/// that redirects it into a file gets either a status object or an empty file — never an error
/// object where a status was meant to be.
fn report(error: &Error, json: bool) {
    let mut stderr = std::io::stderr().lock();

    // The `Display` in `mixengine-proto` is the human rendering: the message, and the hint on a
    // line of its own the way `cargo` prints one. A wire error is three owned strings and cannot
    // fail to serialise, so the fallback is a formality rather than a case.
    let rendered = match json {
        true => serde_json::to_string(error).unwrap_or_else(|_| format!("error: {error}")),
        false => format!("error: {error}"),
    };

    // `writeln!` and not `eprintln!`, which panics if stderr is closed — `mix status 2>&-` in a
    // pipeline that has already gone away is not worth a panic message about a panic.
    let _ = writeln!(stderr, "{rendered}");
}
