//! Process supervision: spawn, watch, restart, health-check, capture logs.
//!
//! The supervisor knows nothing about PHP, Caddy or MariaDB — it only understands a `ServiceSpec`
//! (roadmap task T12). Everything above it is built on the guarantees made here: no orphaned
//! children when the daemon dies, and an honest state machine that distinguishes `Degraded` from
//! `Failed`.

#![warn(missing_docs)]

pub mod command;
pub mod health;
mod http;
pub mod idle;
pub mod logs;
pub mod ready;
pub mod restart;

pub use command::Surroundings;
pub use health::{Health, Verdict};
pub use idle::{Counters, Observation, observe};
// `LogLine` and `Stream` are deliberately not re-exported: they are `mixengine-proto`'s, so that the
// line a capture holds, the line a file is written from and the line an event carries are one type.
pub use logs::Capture;
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

    /// An `Http` check carries something that is not a URL a request can be made from.
    ///
    /// The sibling of [`Pattern`](Self::Pattern), and it exists for the same reason: a check that
    /// can never pass is the spec's fault, and reporting it as a service that never came up sends
    /// the reader to look at the service. Found once, before anything waits. Boxed source, because
    /// what an HTTP parser calls its errors is not vocabulary this API should be handing out — and
    /// because some of these failures are this crate's own sentence rather than a parser's.
    #[error("`{url}` is not a URL a check can be made against")]
    Url {
        /// The URL as the spec wrote it.
        url: String,
        /// Which part of it could not be used.
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

impl Error {
    /// Whether asking again could ever produce a different answer.
    ///
    /// **The distinction a health loop is built on.** A probe that cannot be made is reported once
    /// and then abandoned — the service is left alone, because degrading it for a check nobody can
    /// make would report a fault in the spec as a fault in the service. That is the right answer for
    /// a statement about the *spec* or the *machine*, and the wrong one for a moment: a fork that hit
    /// `EAGAIN`, a probe binary being replaced mid-upgrade (`ETXTBSY`), a keyring that was locked.
    /// Treating those as permanent turns one unlucky second into a service that is never
    /// health-checked again for the whole life of its process — which is exactly the hang that
    /// `mariadb-admin ping` exists to catch.
    ///
    /// So: `false` for the spec's fault and for a capability this system does not have, `true` for
    /// everything the OS refused in passing.
    #[must_use]
    pub fn might_work_later(&self) -> bool {
        match self {
            // Three statements about the spec, and none of them changes while the process runs. A
            // URL that is not a URL will not become one; a build with no TLS will not grow one.
            Self::UnsupportedCheck { .. } | Self::Pattern { .. } | Self::Url { .. } => false,

            // A program that would not start. Usually the spec's fault too — and deliberately not
            // treated as such, because the cases where it is not are the ones that matter: the
            // binary is mid-replacement, or the machine is out of process slots. An install or an
            // upgrade finishing is a probe that starts working again by itself.
            Self::Spawn { .. } => true,

            Self::Platform(error) => !matches!(
                error,
                // The platform's own "there is no such thing here", which is `UnsupportedCheck`'s
                // sentence in the other crate's words: a Unix socket probe on Windows.
                mixengine_platform::Error::UnsupportedPlatform { .. }
            ),
        }
    }
}

/// Result of a supervision operation.
pub type Result<T> = std::result::Result<T, Error>;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_spec_that_cannot_be_checked_here_is_not_asked_again() {
        for error in [
            Error::UnsupportedCheck {
                check: "an HTTP health probe",
                reason: "no TLS in this build".to_owned(),
            },
            Error::Url {
                url: "127.0.0.1:2019".to_owned(),
                source: "no scheme".into(),
            },
            Error::Platform(mixengine_platform::Error::UnsupportedPlatform {
                capability: "UnixSocket",
                reason: "this system has no such socket".to_owned(),
            }),
        ] {
            assert!(!error.might_work_later(), "{error:?}");
        }
    }

    /// The half this exists for: a moment must not end a service's health checking for good.
    #[test]
    fn something_the_os_refused_in_passing_is_worth_asking_again() {
        let error = Error::Platform(mixengine_platform::Error::Io {
            action: "start",
            path: std::path::PathBuf::from("/opt/mixengine/bin/mariadb-admin"),
            source: std::io::Error::from(std::io::ErrorKind::WouldBlock),
        });

        assert!(error.might_work_later(), "{error:?}");
    }
}
