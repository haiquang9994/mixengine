//! `mixengine-elevate` — the only code in MixEngine that runs with administrator privileges.
//!
//! It is started through the OS elevation prompt, applies one batch of privileged operations that
//! it has validated **itself** rather than trusting the daemon, and exits. It never listens on a
//! socket, never runs an arbitrary command, and is never resident. Keep it small enough to audit in
//! one sitting; see `.claude/architecture/security-model.md` and roadmap task T40.

mod audit;
mod ops;
mod request;

use std::process::ExitCode;

/// The arguments made no sense — a caller bug, not a user decision.
const EXIT_USAGE: u8 = 64;
/// The helper understood the request but cannot carry it out in this build.
const EXIT_UNAVAILABLE: u8 = 69;

fn main() -> ExitCode {
    let mut args = std::env::args_os().skip(1);

    // The typed request/response protocol over files is task T40. Until it exists this binary must
    // never exit 0 for a request it did not apply: the daemon reads exit 0 as "the batch went
    // through" and would go on believing the hosts file, the trust store and the firewall were
    // changed when nothing happened at all.
    match args.next() {
        None => {
            eprintln!(
                "mixengine-elevate: expects a request file; it is not meant to be run by hand"
            );
            ExitCode::from(EXIT_USAGE)
        }
        Some(_) => {
            eprintln!(
                "mixengine-elevate: no privileged operations are implemented yet (roadmap T40)"
            );
            ExitCode::from(EXIT_UNAVAILABLE)
        }
    }
}
