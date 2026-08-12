//! Process supervision: spawn, watch, restart, health-check, capture logs.
//!
//! The supervisor knows nothing about PHP, Caddy or MariaDB — it only understands a `ServiceSpec`
//! (roadmap task T12). Everything above it is built on the guarantees made here: no orphaned
//! children when the daemon dies, and an honest state machine that distinguishes `Degraded` from
//! `Failed`.

#![warn(missing_docs)]

pub mod health;
pub mod logs;
pub mod ready;
pub mod restart;

pub use health::{Health, Verdict};
pub use logs::{Capture, LogLine, Stream};
pub use ready::Ready;
pub use restart::{Decision, Restarts};

/// Failure of a supervision operation.
///
/// **A service that is unwell is not an error here.** A process that exited, failed its health check
/// or never became ready is a *state* — see [`ready::Ready`] and `ServiceState` — and travels as a
/// return value. What lands in this enum is a supervisor that could not do its job: a program that
/// could not be started, a spec that cannot be checked on this machine, a pattern that is not a
/// pattern.
#[derive(Debug, thiserror::Error)]
#[non_exhaustive]
pub enum Error {
    /// A managed process could not be started.
    ///
    /// The OS error is carried as the error `source`, so it must not be repeated in this message —
    /// the caller printing the chain would show it twice.
    #[error("failed to spawn `{program}`")]
    Spawn {
        /// The program that was to be executed.
        program: String,
        /// The underlying OS error.
        source: std::io::Error,
    },

    /// A check in the spec is one this build or this machine cannot make.
    ///
    /// The typed answer `CLAUDE.md` requires instead of a `todo!()`, and it covers two different
    /// shapes of gap on purpose, because the caller does the same thing with both: a spec written
    /// for another OS (a Unix socket named on Windows) and a probe whose dependency has not arrived
    /// yet (HTTP, a health command). `reason` says which, in a sentence meant for whoever wrote the
    /// spec.
    #[error("{check} cannot be made here: {reason}")]
    UnsupportedCheck {
        /// What was asked for, e.g. `"an HTTP ready check"`.
        check: &'static str,
        /// Why it cannot be made, phrased for whoever wrote the spec.
        reason: String,
    },

    /// A `LogPattern` check carries something that is not a regular expression.
    ///
    /// Reported before anything waits, because a spec that cannot be checked was never going to
    /// become ready and a timeout would send the reader looking at the service instead of at the
    /// spec. Boxed source: what the regex engine calls its errors is not vocabulary this API should
    /// be handing out.
    #[error("`{pattern}` is not a usable pattern")]
    Pattern {
        /// The pattern as the spec wrote it.
        pattern: String,
        /// The engine's own complaint, which names the position.
        #[source]
        source: Box<dyn std::error::Error + Send + Sync>,
    },

    /// The operating system refused something the supervisor asked of it.
    ///
    /// Passed through rather than re-described: `mixengine-platform` already says which call failed
    /// and about what, and a second sentence here would only add a layer to the chain.
    #[error(transparent)]
    Platform(#[from] mixengine_platform::Error),
}

/// Result of a supervision operation.
pub type Result<T> = std::result::Result<T, Error>;
