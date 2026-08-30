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
    let caller = accepted.caller.to_string();
    let mut results = Vec::with_capacity(accepted.request.ops.len());

    for value in &accepted.request.ops {
        let outcome = match ops::decode(value) {
            Ok(op) => ops::apply(&op, elevated, &accepted.caller),
            Err(outcome) => outcome,
        };

        if elevated {
            let line = audit::entry(
                &caller,
                &accepted.request.nonce,
                ops::named(value),
                &outcome,
            );
            audit::append(log, &line).map_err(|why| Failure::new(EXIT_INTERNAL, why))?;
        }

        results.push(outcome);
    }

    Ok(results)
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
