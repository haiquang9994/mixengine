//! The domain: what a project, site, runtime and service *are*.
//!
//! Storage and platform access arrive as injected traits, so every rule in here — version
//! resolution, config rendering, blueprint diffing — is testable without touching the machine.
//! Modules are organised by capability (`sites/`, `runtimes/`, `certs/`), never by layer.
//!
//! `core` never depends on `daemon`.

#![warn(missing_docs)]

/// Failure of a domain operation.
#[derive(Debug, thiserror::Error)]
#[non_exhaustive]
pub enum Error {
    /// The requested entity does not exist.
    #[error("no such {kind}: {id}")]
    NotFound {
        /// The kind of entity, e.g. `"site"`.
        kind: &'static str,
        /// The identifier that was looked up.
        id: String,
    },
}

/// Result of a domain operation.
pub type Result<T> = std::result::Result<T, Error>;
