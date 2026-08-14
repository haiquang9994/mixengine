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
use mixengine_proto::{
    DaemonShutdown, DaemonStatus, Error, ErrorCode, LogFrame, ServiceId, ServiceList, ServiceQuery,
    ServiceSummary, ServiceTarget, ServiceWalk, rpc,
};

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

    /// Control the daemon itself.
    Daemon {
        #[command(subcommand)]
        command: DaemonCommand,
    },

    /// Inspect and control the services this home declares.
    Service {
        #[command(subcommand)]
        command: ServiceCommand,
    },
}

/// `mix daemon …` — the daemon as a thing in itself, rather than as what answers about services.
///
/// `status` is deliberately not here and stays `mix status`: it is the first command anybody types,
/// and moving it would be renaming the one command that already exists to make room for a namespace.
#[derive(Debug, Subcommand)]
enum DaemonCommand {
    /// Stop the services this home is running, then stop the daemon.
    Stop,
}

/// `mix service …` — one subcommand per `service.*` method, and nothing that is not one.
#[derive(Debug, Subcommand)]
enum ServiceCommand {
    /// List every declared service and what it is doing.
    List,

    /// Describe one service.
    ///
    /// The id is required, where `start` and the rest take an optional one: a status with no
    /// subject is a `list` that was typed wrongly, and answering it as a list would hide that.
    Status {
        /// The service to describe.
        #[arg(value_name = "SERVICE", value_parser = service_id)]
        service: ServiceId,
    },

    /// Print what a service has been printing.
    ///
    /// The one `mix service` subcommand that is not a `service.*` method: output is a stream, and a
    /// JSON-RPC call cannot be one — see
    /// [ADR 0009](../../../.claude/decisions/0009-logs-travel-on-their-own-stream.md).
    Logs {
        /// The service to read.
        #[arg(value_name = "SERVICE", value_parser = service_id)]
        service: ServiceId,

        /// How many of the lines already printed to begin with.
        #[arg(long, short = 'n', value_name = "LINES", default_value_t = 200)]
        lines: usize,

        /// Keep printing as the service prints, rather than stopping at what it already said.
        ///
        /// Survives the service crashing and being restarted: what is being followed is the
        /// service, not one run of its process.
        #[arg(long, short)]
        follow: bool,
    },

    /// Start a service, and everything it depends on.
    Start(Target),

    /// Stop a service, and everything that depends on it.
    Stop(Target),

    /// Stop a service and what depends on it, then start that same set again.
    Restart(Target),
}

/// What `start`, `stop` and `restart` take, which is the same question three times.
#[derive(Debug, clap::Args)]
struct Target {
    /// The service to act on. Every declared service when it is left out.
    ///
    /// Naming one does not mean acting on one — a plan is the transitive set — and what the daemon
    /// walked comes back in the answer.
    #[arg(value_name = "SERVICE", value_parser = service_id)]
    service: Option<ServiceId>,

    /// Return once the daemon has accepted the plan, rather than once it has walked it.
    ///
    /// `mix` waits by default, because `mix service start db && …` is a sentence about the database
    /// being up: an answer sent before the walk would exit `0` for a service that never came up.
    #[arg(long)]
    no_wait: bool,
}

/// A service id from the command line, refused here rather than at the daemon.
///
/// Not the client deciding anything — [`ServiceId::parse`] is the daemon's own rule, from the crate
/// that owns the vocabulary — it is only where the answer is cheapest: a typo should not start a
/// daemon and travel over a socket to be told it is a typo.
fn service_id(value: &str) -> Result<ServiceId, String> {
    ServiceId::parse(value).map_err(|error| error.to_string())
}

fn main() -> ExitCode {
    let args = Args::parse();
    let json = args.json;

    match run(args) {
        Ok(code) => code,
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
async fn run(args: Args) -> Result<ExitCode, Error> {
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
        // **Never autostarts, whatever the flags say**, and it is the one command that decides this
        // for itself: starting a daemon in order to ask it to stop is a machine left exactly as it
        // was found, one process later. A home with nothing running is told so as the wire error for
        // a daemon that is not there, which is the same sentence every other command gets.
        Command::Daemon {
            command: DaemonCommand::Stop,
        } => daemon_stop(&endpoint, args.json).await,
        Command::Service { command } => {
            service(command, &endpoint, autostart.as_ref(), args.json).await
        }
    }
}

/// `mix daemon stop`: stop the services, then the daemon.
///
/// **The answer arrives before the daemon goes**, which is what makes an exit code possible here at
/// all: the walk it carries says whether everything this home was running actually stopped, and a
/// service that refused is worth a non-zero status even though the daemon stopped regardless. What
/// happens to the connection a moment later is not this command's business — the response has been
/// read by then.
///
/// **A stop that could not be ordered is the same kind of non-zero**, and for the same reason rather
/// than by analogy: the exit code here has never meant "the daemon stopped" — it means "what was
/// asked for happened" — and what `mix daemon stop` asks for is every service stopped in dependency
/// order. A daemon that could not work one out stopped them all at the same moment instead, which is
/// the arrangement the ordering exists to prevent, and exiting `0` would carry a
/// `mix daemon stop && …` past it in silence. Both halves are still on stdout in both renderings;
/// only the status changes.
async fn daemon_stop(endpoint: &Endpoint, json: bool) -> Result<ExitCode, Error> {
    let mut client = Client::connect(endpoint, None).await?;
    let shutdown: DaemonShutdown = ask(&mut client, rpc::method::DAEMON_SHUTDOWN, None).await?;

    emit(&rendered(json, &shutdown, || {
        render::daemon_shutdown(&shutdown)
    }))?;

    Ok(match (&shutdown.services.failed, &shutdown.unordered) {
        (None, None) => ExitCode::SUCCESS,
        _ => ExitCode::FAILURE,
    })
}

/// `mix status`: what the daemon says about itself.
async fn status(
    endpoint: &Endpoint,
    autostart: Option<&Autostart>,
    json: bool,
) -> Result<ExitCode, Error> {
    let mut client = Client::connect(endpoint, autostart).await?;
    let status: DaemonStatus = ask(&mut client, rpc::method::DAEMON_STATUS, None).await?;

    match json {
        // The newline is here rather than in `render`, which builds a document and does not know
        // whether it is the last thing on the stream. The human rendering ends in one already.
        true => emit(&format!("{}\n", render::status_json(&status)))?,
        false => emit(&render::status(&status))?,
    }

    Ok(ExitCode::SUCCESS)
}

/// `mix service …`: one call, one rendering, and an exit code that means what a shell expects.
///
/// **A walk that failed is an answer and not an error**, which is why this returns an
/// [`ExitCode`] rather than reporting through [`report`]: a plan of six services where the fourth
/// fails leaves three running, one failed and two never tried, and all of that goes to stdout in
/// both renderings. What the failure changes is the exit status, so `mix service start db && …`
/// stops where a person reading the output would.
async fn service(
    command: ServiceCommand,
    endpoint: &Endpoint,
    autostart: Option<&Autostart>,
    json: bool,
) -> Result<ExitCode, Error> {
    let mut client = Client::connect(endpoint, autostart).await?;

    // The walk methods differ by one string and one verb, so they are one arm with both in it —
    // three copies of this block would be three places for the two to drift apart.
    let (method, walked, params) = match &command {
        ServiceCommand::List => {
            let list: ServiceList = ask(&mut client, rpc::method::SERVICE_LIST, None).await?;
            emit(&rendered(json, &list, || render::service_list(&list)))?;
            return Ok(ExitCode::SUCCESS);
        }

        ServiceCommand::Status { service } => {
            let query = ServiceQuery {
                service: service.clone(),
            };
            let summary: ServiceSummary =
                ask(&mut client, rpc::method::SERVICE_STATUS, encode(&query)).await?;
            emit(&rendered(json, &summary, || {
                render::service_status(&summary)
            }))?;
            return Ok(ExitCode::SUCCESS);
        }

        ServiceCommand::Logs {
            service,
            lines,
            follow,
        } => {
            return logs(&mut client, service, *lines, *follow, json).await;
        }

        ServiceCommand::Start(target) => {
            (rpc::method::SERVICE_START, render::Walked::Start, target)
        }
        ServiceCommand::Stop(target) => (rpc::method::SERVICE_STOP, render::Walked::Stop, target),
        ServiceCommand::Restart(target) => (
            rpc::method::SERVICE_RESTART,
            render::Walked::Restart,
            target,
        ),
    };

    let target = ServiceTarget {
        service: params.service.clone(),
        wait: !params.no_wait,
    };

    let walk: ServiceWalk = ask(&mut client, method, encode(&target)).await?;
    emit(&rendered(json, &walk, || {
        render::service_walk(walked, &walk)
    }))?;

    Ok(match walk.failed {
        None => ExitCode::SUCCESS,
        Some(_) => ExitCode::FAILURE,
    })
}

/// `mix service logs`: what a service has printed, and what it prints next.
///
/// **Written out as it arrives rather than collected**, which is the whole difference between this
/// and every other command here: a `--follow` never has a last message, and a buffer that filled
/// until the stream ended would print nothing for as long as the service kept running.
///
/// **The text goes out exactly as the service wrote it.** No timestamp, no `[stderr]`, nothing of
/// MixEngine's — for the same reason `current.log` carries none: this is piped into `grep` by
/// somebody who greps MariaDB's log the same way, and a prefix of ours would break every one of
/// those to restate what `--json` already carries. What the human rendering does add is the one
/// thing that is not output: a gap, on stderr, where the daemon or this client fell behind and lines
/// were lost. Silence there would make a log with a hole in it look complete.
async fn logs(
    client: &mut Client,
    service: &ServiceId,
    lines: usize,
    follow: bool,
    json: bool,
) -> Result<ExitCode, Error> {
    let path = format!("/logs/{service}?tail={lines}&follow={}", u8::from(follow));
    let mut stream = client.stream(&path).await?;

    while let Some(frame) = stream.next::<LogFrame>().await? {
        match (json, &frame) {
            // Verbatim, one object per line: a script filtering on `stream` or ordering by `at`
            // needs what the human rendering deliberately drops.
            (true, _) => emit(&format!(
                "{}\n",
                serde_json::to_string(&frame).expect("a proto type always serialises")
            ))?,

            (false, LogFrame::Line(line)) => emit(&format!("{}\n", line.text))?,
            (false, LogFrame::Historic { text }) => emit(&format!("{text}\n"))?,

            (false, LogFrame::Gap { missed }) => {
                report_gap(*missed);
            }

            // A variant from a later daemon. Ignored rather than refused, which is what the wire
            // types are `non_exhaustive` for.
            (false, _) => {}
        }
    }

    Ok(ExitCode::SUCCESS)
}

/// Say on stderr that lines were lost, so that a redirected log stays exactly the log.
fn report_gap(missed: u64) {
    let mut stderr = std::io::stderr();

    // Nothing to do about a stderr that will not take it, and nothing worth failing the command
    // over: the output the user asked for is still going out.
    let _ = writeln!(
        stderr,
        "mix: {missed} lines were dropped — this client fell behind the service"
    );
}

/// Call a method and decode what it answered.
///
/// **Decoded rather than passed through as the [`Value`](serde_json::Value) it arrived as, even for
/// `--json`.** The handshake has already established that this daemon speaks our protocol, so a
/// field this build cannot read is a bug worth reporting as one — and `--json` promising a
/// `ServiceWalk` means it has to be a `ServiceWalk` that goes out.
async fn ask<T: serde::de::DeserializeOwned>(
    client: &mut Client,
    method: &str,
    params: Option<serde_json::Value>,
) -> Result<T, Error> {
    let result = client.call(method, params).await?;

    serde_json::from_value(result).map_err(|error| {
        Error::new(
            ErrorCode::Internal,
            format!(
                "mix {} cannot read the answer to {method} from mixengined {}: {error}",
                env!("CARGO_PKG_VERSION"),
                client.daemon().version
            ),
        )
    })
}

/// The parameters of a call, as the wire carries them.
///
/// `expect` and not a failure path: every params type here is `mixengine-proto`'s and made of
/// strings, booleans and options, none of which can fail to serialise.
fn encode(params: &impl serde::Serialize) -> Option<serde_json::Value> {
    Some(serde_json::to_value(params).expect("a proto params type always serialises"))
}

/// One of the two renderings of an answer, ready to be written.
///
/// The `--json` half is the daemon's answer **verbatim**, unlike `mix status`, whose envelope exists
/// so a captured diagnostic says which `mix` produced it. A script asking about services wants
/// `.services[]` and `.failed.reason.kind` where the API names them, and the daemon's build is one
/// `mix status` away.
fn rendered(json: bool, answer: &impl serde::Serialize, human: impl FnOnce() -> String) -> String {
    match json {
        true => format!(
            "{}\n",
            serde_json::to_string(answer).expect("a proto answer type always serialises")
        ),
        false => human(),
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
