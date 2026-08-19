//! Wire types shared by `mixengined` and every client.
//!
//! This crate is the single source of truth for the API surface: requests, responses, events and
//! the wire error. It is `serde`-only on purpose — no I/O, no platform code, no domain logic — so
//! that a client can depend on it without pulling in the daemon's world, and so the TypeScript
//! bindings can be generated from it (see roadmap task T56).
//!
//! The payload types are re-exported flat, because a caller writes `DaemonStatus` and never wants
//! to say which module it came from. [`rpc`] stays a module: `rpc::Request` is a JSON-RPC request
//! and not a MixEngine one, and the qualification is what keeps that visible at every call site.

#![warn(missing_docs)]

mod daemon;
mod error;
mod event;
mod job;
mod job_api;
mod log;
mod package_api;
mod path_api;
pub mod rpc;
mod runtime;
mod runtime_api;
mod service;
mod service_api;
mod state;
mod time;
mod version;

pub use daemon::{DaemonShutdown, DaemonStatus, DaemonVersion, Health};
pub use error::{Error, ErrorCode, flatten};
pub use event::DaemonEvent;
pub use job::{JobFinish, JobId, JobKind, JobOutcome, JobProgress, JobState, JobUpdate};
pub use job_api::{JobFilter, JobList, JobQuery, JobSummary, JobWait};
pub use log::{LogFrame, LogLine, Stream};
pub use package_api::{
    PackageCatalogue, PackageFilter, PackageList, PackageRelease, PackageRemoval, PackageSummary,
    PackageTarget,
};
pub use path_api::{PathPlace, PathReport};
pub use runtime::RuntimeKind;
pub use runtime_api::{
    ResolvedRuntime, RuntimeCatalogue, RuntimeFilter, RuntimeList, RuntimeQuestion, RuntimeRelease,
    RuntimeRemoval, RuntimeSource, RuntimeSummary, RuntimeTarget,
};
pub use service::{
    Backoff, EnvValue, HealthCheck, HealthProbe, IdlePolicy, IdleProbe, LogPolicy, Priority,
    ReadyCheck, ReloadBehaviour, ReloadSignal, ResourceLimits, RestartPolicy, ServiceId,
    ServiceSpec, ServiceSpecBuilder, SpecError, StopBehaviour,
};
pub use service_api::{
    ServiceCreate, ServiceFailure, ServiceList, ServiceQuery, ServiceRemoval, ServiceSummary,
    ServiceTarget, ServiceWalk,
};
pub use state::{ServiceState, ServiceTransition, StateReason};
pub use time::{Millis, Timestamp, Uptime};
pub use version::{PackageChannel, PackageVersion, VersionConstraint, VersionError};

/// Version of the JSON-RPC protocol spoken over the local IPC transport.
///
/// The daemon and every client negotiate this on connect, and so do the daemon and
/// `mixengine-elevate`. Bump it when a change is not backwards compatible for an older peer.
pub const PROTOCOL_VERSION: ProtocolVersion = ProtocolVersion(1);

/// A protocol version, exchanged during the handshake.
#[derive(
    Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, serde::Serialize, serde::Deserialize,
)]
#[serde(transparent)]
pub struct ProtocolVersion(pub u32);

impl std::fmt::Display for ProtocolVersion {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "v{}", self.0)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn protocol_version_is_wire_transparent() {
        let encoded = serde_json::to_string(&PROTOCOL_VERSION).unwrap();
        assert_eq!(encoded, "1");
        assert_eq!(
            serde_json::from_str::<ProtocolVersion>(&encoded).unwrap(),
            PROTOCOL_VERSION
        );
    }
}
