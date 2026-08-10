//! Process supervision: spawn, watch, restart, health-check, capture logs.
//!
//! The supervisor knows nothing about PHP, Caddy or MariaDB — it only understands a `ServiceSpec`
//! (roadmap task T12). Everything above it is built on the guarantees made here: no orphaned
//! children when the daemon dies, and an honest state machine that distinguishes `Degraded` from
//! `Failed`.

#![warn(missing_docs)]

/// Failure of a supervision operation.
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
}

/// Result of a supervision operation.
pub type Result<T> = std::result::Result<T, Error>;
