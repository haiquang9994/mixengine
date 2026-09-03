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

mod blueprint;
mod blueprint_api;
mod bundle_api;
mod cert_api;
mod daemon;
mod database;
mod database_api;
mod doctor_api;
mod domain_api;

// Documented by its own `//!` header, like `privileged` below: an outer `///` here would put the
// module's intra-doc links into this module's scope.
pub mod domains;
pub mod elevation;
mod error;
mod event;
mod extension;
mod extension_api;
mod job;
mod job_api;
pub mod limits;
mod log;
mod metrics;
mod package_api;
mod path_api;
// Documented by its own `//!` header. An outer `///` here as well would put the module's intra-doc
// links into *this* module's scope, where `PrivilegedRequest` is not a name.
pub mod privileged;
mod project_api;
mod repair_api;
pub mod rpc;
mod runtime;
mod runtime_api;
mod service;
mod service_api;
mod site_api;
mod state;
mod time;
mod version;

pub use blueprint::{
    BlueprintApplied, BlueprintApplyResponse, BlueprintList, BlueprintPlan, BlueprintSource,
    BlueprintSummary, Disposition, PlanAction, PlanStep, SignatureCheck, StepOutcome, StepResult,
};
pub use blueprint_api::{
    AnswerSubject, BlueprintApply, BlueprintCapture, BlueprintImport, MismatchAnswer,
    ScaffoldConsent, VersionAnswer,
};
pub use bundle_api::{
    BundleReport, DiagnosticsBundle, LogExcerpt, MANIFEST_FORMAT, Manifest, Member, Omission, Part,
    PlatformFacts, ReservedRange,
};
pub use cert_api::{
    BrowserDatabase, Browsers, Ca, CaRotateQuery, CaRotateReport, CaState, CaStatus, CaStatusQuery,
    CaUninstallQuery, CaUninstallReport, CertIssue, CertIssueReport, CertProblem, CertState,
    CertStatusQuery, CertStatusReport, Handshake, IssueOutcome, RotateOutcome, SiteCert,
    SiteCertOutcome, SiteCertStatus, Trust, UninstallOutcome, Unusable, Verdict,
};
pub use daemon::{DaemonShutdown, DaemonStatus, DaemonVersion, DnsMode, DnsStatus, Health};
pub use database::{
    DatabaseAccount, DatabaseClientReport, DatabaseHandoff, DatabaseProtocol, DesktopClient,
    Launch, Made, Provisioned, SecretAddress,
};
pub use database_api::{DatabaseClientQuery, DatabaseCreate, DatabaseOpen};
pub use doctor_api::{Check, DoctorReport, Outcome, ProblemId};
pub use domain_api::{
    DomainAdd, DomainRemove, DomainStatus, DomainStatusQuery, DomainStatusReport,
};
pub use elevation::{
    ElevationDrop, ElevationStatus, ElevationSummary, GrantOutcome, PendingOp, PendingOpId,
};
pub use error::{Error, ErrorCode, flatten};
pub use event::DaemonEvent;
pub use extension::{
    ApiAccess, ExtensionId, ExtensionKind, ExtensionPermissions, FilesystemReach, FrontEndServer,
    NetworkReach,
};
pub use extension_api::{
    ArtifactAvailability, DesktopAppSummary, ExtensionCatalogue, ExtensionConsent,
    ExtensionInspect, ExtensionInspection, ExtensionInstall, ExtensionOffer, ExtensionOrigin,
    ExtensionPlan, ExtensionPlanRequest, ExtensionRemoval, ExtensionSummary, ExtensionTarget,
    ExtensionUninstall, InstalledExtensions, PlannedSite, PortWish, RecipeAddition, WebAppSummary,
};
pub use job::{JobFinish, JobId, JobKind, JobOutcome, JobProgress, JobState, JobUpdate};
pub use job_api::{JobFilter, JobList, JobQuery, JobSummary, JobWait};
pub use limits::{Enforcement, LimitMechanism, LimitSupport, MemoryMeasure, WhenExceeded};
pub use log::{LogFrame, LogLine, LogSubject, Stream};
pub use metrics::{
    MetricsFrame, MetricsHistory, MetricsHistoryQuery, MetricsMinute, MetricsSample, MetricsSubject,
};
pub use package_api::{
    PackageCatalogue, PackageFilter, PackageList, PackageRelease, PackageRemoval, PackageSummary,
    PackageTarget,
};
pub use path_api::{PathPlace, PathReport};
pub use project_api::{
    PinSource, ProjectCreate, ProjectDetail, ProjectExport, ProjectList, ProjectPin, ProjectQuery,
    ProjectRef, ProjectRemoval, ProjectSummary, ProjectUpdate,
};
pub use repair_api::{Action, DoctorRepair, Repair, RepairReport};
pub use runtime::RuntimeKind;
pub use runtime_api::{
    ExtensionChange, ExtensionChoice, ExtensionList, ExtensionSource, Linkage, PoolOutcome,
    ResolvedRuntime, RuntimeCatalogue, RuntimeExtension, RuntimeFilter, RuntimeList,
    RuntimeQuestion, RuntimeRelease, RuntimeRemoval, RuntimeSource, RuntimeSummary, RuntimeTarget,
    RuntimeUninstall,
};
pub use service::{
    Backoff, EnvValue, HealthCheck, HealthProbe, IdleExemption, IdlePolicy, IdleProbe, IdleSource,
    KEYRING_SERVICE, LogPolicy, Priority, ReadyCheck, ReloadBehaviour, ReloadSignal,
    ResourceLimits, RestartPolicy, ServiceId, ServiceSpec, ServiceSpecBuilder, SpecError,
    StopBehaviour,
};
pub use service_api::{
    IdleReport, MemoryWatchdog, PortMoved, ServiceCreate, ServiceCreation, ServiceDelete,
    ServiceFailure, ServiceIdleSet, ServiceLimitsReport, ServiceLimitsSet, ServiceList,
    ServiceQuery, ServiceRemoval, ServiceSummary, ServiceTarget, ServiceWalk,
};
pub use site_api::{
    SharingChange, SiteCreate, SiteCreation, SiteDetail, SiteKind, SiteList, SiteListQuery,
    SiteOwner, SitePool, SiteQuery, SiteRef, SiteRemoval, SiteServiceLink, SiteShare, SiteSharing,
    SiteState, SiteSummary, SiteUpdate,
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
