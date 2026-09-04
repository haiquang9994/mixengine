//! `mixengine-elevate` — the only code in MixEngine that runs with administrator privileges.
//!
//! It is started through the OS elevation prompt with **one** argument, the path to a request file;
//! it validates that request **itself** rather than trusting the daemon, applies it, writes
//! `response.json` beside it, and exits. It never listens on a socket, never runs an arbitrary
//! command, and is never resident. Keep it small enough to audit in one sitting; see
//! `.claude/architecture/security-model.md` and roadmap task T40.
//!
//! **The response file is the protocol.** Exit 0 means "the batch was processed and there is a report
//! to read" — *even when every operation in it failed*. It does not mean "it worked". Every other
//! code means there is no report, and is the only case where the number itself carries the answer.
//!
//! **"User declined" is not one of these codes.** When the user clicks Cancel this binary never ran,
//! so it cannot report it; that is `ElevationOutcome::Declined`, mapped by the launcher (T40a) from
//! `ERROR_CANCELLED`, osascript's `-128` and `pkexec`'s 126.

mod audit;
mod firewall;
mod helper;
mod hosts;
mod ops;
mod port_access;
mod request;
mod resolver;
mod trust;

use std::path::{Path, PathBuf};
use std::process::ExitCode;

use mixengine_platform::elevated::is_elevated;
use mixengine_platform::lock::{Acquired, Lock};
use mixengine_proto::PROTOCOL_VERSION;
use mixengine_proto::privileged::{OpOutcome, PrivilegedOp, PrivilegedResponse};

use request::Accepted;

/// The batch was processed and reported. Read the response, not this.
const EXIT_OK: u8 = 0;
/// The arguments made no sense — a caller bug, not a user decision.
const EXIT_USAGE: u8 = 64;
/// The request could not be read, parsed, or passed whole-request validation.
const EXIT_REFUSED: u8 = 65;
/// The helper cannot run here: the machine is not in a state it will operate in.
const EXIT_UNAVAILABLE: u8 = 69;
/// Something inside this process failed, after the request had been accepted.
const EXIT_INTERNAL: u8 = 70;

/// The lock file, inside the home the request named. Held for the whole run: two overlapping
/// elevation prompts are a thing a user can produce.
const LOCK: &str = "elevate.lock";

fn main() -> ExitCode {
    match run() {
        Ok(()) => ExitCode::from(EXIT_OK),
        Err(failure) => {
            eprintln!("mixengine-elevate: {}", failure.why);
            ExitCode::from(failure.code)
        }
    }
}

/// Why there is no response file, and what number says so.
struct Failure {
    code: u8,
    why: String,
}

impl Failure {
    fn new(code: u8, why: impl Into<String>) -> Self {
        Self {
            code,
            why: why.into(),
        }
    }
}

fn run() -> Result<(), Failure> {
    let mut arguments = std::env::args_os().skip(1);

    // Exactly one, and a second is as much a caller bug as none: this binary is spawned by a daemon
    // through an elevation prompt and is not meant to be run by hand.
    let (Some(path), None) = (arguments.next(), arguments.next()) else {
        return Err(Failure::new(
            EXIT_USAGE,
            "expects exactly one argument, the path to a request file; it is not meant to be run by \
             hand",
        ));
    };

    let path = PathBuf::from(path);
    let accepted =
        request::read(&path).map_err(|why| Failure::new(EXIT_REFUSED, why.to_string()))?;

    // Asked once and carried, so the gate below and the `elevated` the response reports can never
    // disagree.
    let elevated = is_elevated();

    let log = audit::path().map_err(|why| Failure::new(EXIT_UNAVAILABLE, why))?;
    if elevated {
        audit::prepare(&log).map_err(|why| Failure::new(EXIT_UNAVAILABLE, why))?;
    }

    // Taken before anything is applied and released by the OS when this process ends — including
    // when it is killed, which is why it is a handle and not a file anybody has to clean up.
    let _held = hold(&accepted.request.home)?;

    let results = process(&accepted, elevated, &log)?;

    let response = PrivilegedResponse {
        version: PROTOCOL_VERSION,
        elevate_version: env!("CARGO_PKG_VERSION").to_owned(),
        nonce: accepted.request.nonce.clone(),
        elevated,
        supported_ops: PrivilegedOp::ALL
            .iter()
            .map(|name| (*name).to_owned())
            .collect(),
        audit_log: log,
        results,
    };

    write(&accepted.response, &response)
}

/// Take the lock in `home`, or say who has it.
fn hold(home: &Path) -> Result<Acquired, Failure> {
    let path = home.join("run").join(LOCK);

    match Lock::acquire(&path) {
        Ok(Acquired::Held(lock)) => Ok(Acquired::Held(lock)),
        Ok(Acquired::Taken(holder)) => Err(Failure::new(
            EXIT_UNAVAILABLE,
            format!("another elevated operation is in progress ({holder})"),
        )),
        Err(error) => Err(Failure::new(EXIT_UNAVAILABLE, error.to_string())),
    }
}

/// Decode, gate and apply each operation in turn, recording every one.
fn process(accepted: &Accepted, elevated: bool, log: &Path) -> Result<Vec<OpOutcome>, Failure> {
    Ok(apply_each(
        &accepted.request.ops,
        &accepted.caller,
        &accepted.request.nonce,
        elevated,
        log,
    ))
}

/// The body of [`process`], taking values rather than the request so a test can drive it.
///
/// **`audit-log-remove` is applied after every other operation in the batch, and no line is written
/// for it** — roadmap task T87, the design's D5. Applied in place it would be followed by the line
/// describing it, which recreates the file the operation exists to remove; and every operation after
/// it in the batch would recreate it again. Its outcome still lands at **its own index**, because
/// the daemon settles the queue by position.
///
/// **The ordering is enforced here rather than by refusing a badly ordered request.** A refusal
/// would turn a queue that happened to accumulate one extra row into a queue that can never be
/// granted again; and trusting the daemon to enqueue in the right order would make an audit property
/// depend on the one caller this binary is built not to trust.
///
/// **A line that cannot be written is no longer fatal to the run.** It was, and could not stay so
/// once a batch could remove the file being appended to: the response is the record that reaches the
/// daemon, and losing the run because the evidence could not be filed would be losing the thing the
/// evidence is about.
fn apply_each(
    ops: &[serde_json::Value],
    caller: &mixengine_platform::elevated::Owner,
    nonce: &str,
    elevated: bool,
    log: &Path,
) -> Vec<OpOutcome> {
    let mut results: Vec<Option<OpOutcome>> = vec![None; ops.len()];
    let mut last: Option<usize> = None;

    for (index, value) in ops.iter().enumerate() {
        let decoded = ops::decode(value);

        if matches!(decoded, Ok(PrivilegedOp::AuditLogRemove {})) {
            last = Some(index);
            continue;
        }

        let outcome = match decoded {
            Ok(op) => ops::apply(&op, elevated, caller),
            Err(outcome) => outcome,
        };

        if elevated {
            record(log, caller, nonce, ops::named(value), &outcome);
        }

        results[index] = Some(outcome);
    }

    if let Some(index) = last {
        let outcome = match ops::decode(&ops[index]) {
            Ok(op) => ops::apply(&op, elevated, caller),
            Err(outcome) => outcome,
        };

        // Recorded nowhere, on purpose: see this function's own documentation.
        results[index] = Some(outcome);
    }

    results
        .into_iter()
        .map(|outcome| outcome.expect("every index was filled"))
        .collect()
}

/// One line, or a complaint on stderr that there was nowhere to put it.
///
/// Standard error and not a returned failure, for the reason [`apply_each`] gives: this is where the
/// run stopped being allowed to fail over its own evidence.
fn record(
    log: &Path,
    caller: &mixengine_platform::elevated::Owner,
    nonce: &str,
    op: &str,
    outcome: &OpOutcome,
) {
    let line = audit::entry(&caller.to_string(), nonce, op, outcome);

    if let Err(why) = audit::append(log, &line) {
        eprintln!("mixengine-elevate: cannot record {op}: {why}");
    }
}

/// Write the answer beside the request.
///
/// `create_new`: the anti-replay check ran before any of this, and this is the same rule enforced
/// against a race rather than against a replay.
fn write(path: &Path, response: &PrivilegedResponse) -> Result<(), Failure> {
    let file = std::fs::File::create_new(path).map_err(|source| {
        Failure::new(
            EXIT_INTERNAL,
            format!("cannot create {}: {source}", path.display()),
        )
    })?;

    serde_json::to_writer(&file, response).map_err(|source| {
        Failure::new(
            EXIT_INTERNAL,
            format!("cannot write {}: {source}", path.display()),
        )
    })?;

    // The daemon may be watching for this file and reading it the moment it appears.
    file.sync_all().map_err(|source| {
        Failure::new(
            EXIT_INTERNAL,
            format!("cannot flush {}: {source}", path.display()),
        )
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    /// D2: every code is at or below 125. `pkexec` reserves 126 and 127 for its own failures and
    /// shells use 128+n; a helper that spent those numbers would be indistinguishable from the
    /// launcher failing to start it.
    #[test]
    fn no_exit_code_collides_with_a_launchers_own() {
        for code in [
            EXIT_OK,
            EXIT_USAGE,
            EXIT_REFUSED,
            EXIT_UNAVAILABLE,
            EXIT_INTERNAL,
        ] {
            assert!(code <= 125, "{code}");
        }
    }

    /// D5: the line describing the log's removal would recreate the file that removal exists to
    /// remove, so it is applied after everything else and recorded nowhere — while its outcome still
    /// arrives at its own index, because the daemon settles the queue by position.
    ///
    /// **The removal sits in the middle of the batch on purpose.** The daemon puts it last; this is
    /// what proves the helper does not depend on it having done so.
    #[test]
    fn the_audit_log_removal_is_applied_last_and_written_into_no_line() {
        let directory = tempfile::TempDir::new().expect("a temporary directory");
        let log = directory.path().join("elevate.log");

        // The only kind of `Owner` there is: the type has no public constructor, deliberately.
        let file = directory.path().join("request.json");
        std::fs::write(&file, b"{}").expect("the file");
        let caller = mixengine_platform::elevated::owner_of(&file).expect("its owner");

        let ops = vec![
            serde_json::json!({ "op": "probe" }),
            serde_json::json!({ "op": "audit-log-remove" }),
            serde_json::json!({ "op": "probe" }),
        ];

        let results = apply_each(&ops, &caller, "n", true, &log);

        assert_eq!(results.len(), 3);
        assert!(
            matches!(results[0], OpOutcome::Applied { .. }),
            "{results:?}"
        );
        assert!(
            matches!(results[2], OpOutcome::Applied { .. }),
            "{results:?}"
        );

        // Two lines and not three: both probes, and nothing for the removal between them.
        let written = std::fs::read_to_string(&log).unwrap_or_default();
        let lines: Vec<&str> = written.lines().collect();

        assert_eq!(lines.len(), 2, "{written}");
        for line in lines {
            let entry: serde_json::Value =
                serde_json::from_str(line).expect("each line is its own document");
            assert_eq!(entry["op"], "probe", "the removal recorded itself: {entry}");
        }
    }

    /// And each means something different, or the daemon cannot tell them apart.
    #[test]
    fn every_exit_code_is_its_own() {
        let codes = [
            EXIT_OK,
            EXIT_USAGE,
            EXIT_REFUSED,
            EXIT_UNAVAILABLE,
            EXIT_INTERNAL,
        ];
        let mut seen = codes.to_vec();
        seen.sort_unstable();
        seen.dedup();

        assert_eq!(seen.len(), codes.len());
    }
}
