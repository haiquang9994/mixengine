//! `extension.inspect` — roadmap task **T80**.
//!
//! One method, and it is read-only: T80 installs nothing. What it answers is what an install on
//! this machine *would* produce, which is `blueprint.apply --dry-run`'s position — a plan is worth
//! having because it was computed rather than because it was described.
//!
//! The same split [`crate::job_api`] draws over [`crate::job`]: [`crate::extension`] is the
//! vocabulary an extension is *described* in, and this is what one call asks and answers.

use crate::{
    ExtensionId, ExtensionKind, ExtensionPermissions, NetworkReach, PackageVersion, RuntimeKind,
    ServiceId, ServiceSpec, VersionConstraint,
};

/// Which manifest to read.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ExtensionInspect {
    /// The directory holding `extension.toml`, or that file itself — because that is what a person
    /// types.
    ///
    /// Absolute; the client resolves it against its own current directory, as every path in this
    /// API is resolved. The daemon has no idea what directory a client is in, and reading a
    /// relative path against its own would read the wrong file rather than fail.
    pub path: String,
}

/// What one manifest says, and what installing it here would produce.
#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct ExtensionInspection {
    /// Its id, which is also its directory and — for a `service` — its service.
    pub id: ExtensionId,

    /// The display name.
    pub name: String,

    /// Its own version, not MixEngine's.
    pub version: PackageVersion,

    /// What it is.
    pub kind: ExtensionKind,

    /// What it is for.
    pub description: String,

    /// Where to read about it.
    pub homepage: Option<String>,

    /// What it declares.
    ///
    /// **`services` is a disclosure and not a boundary** — see [`ApiAccess`](crate::ApiAccess) and
    /// ADR 0014. `network` and `filesystem` are enforced by the manifest format itself.
    pub permissions: ExtensionPermissions,

    /// Whether this machine has an artifact to install.
    pub artifact: ArtifactAvailability,

    /// The ports it asked for. **Asked for, not held** — see [`PortWish`].
    pub ports: Vec<PortWish>,

    /// Where it would be installed.
    pub install_dir: String,

    /// Where it would write.
    pub data_dir: String,

    /// The spec that would run, for a `service`.
    ///
    /// The rendered thing rather than a description of it: every placeholder substituted, the
    /// address decided by `permissions.network`, and put through `ServiceSpec`'s own checks. For
    /// the three kinds with nothing to supervise this is [`None`], and no spec is invented to have
    /// something to show.
    pub runs: Option<ServiceSpec>,

    /// The site that would be generated, for a `web-app`.
    pub serves: Option<WebAppSummary>,

    /// What would be opened, for a `desktop-app`.
    pub opens: Option<DesktopAppSummary>,

    /// What it adds to generated configuration. May be non-empty for any kind — a `service` that
    /// also carries a recipe is Mailpit, and is the ordinary case rather than the odd one.
    pub extends: Vec<RecipeAddition>,
}

/// Whether an artifact is published for this machine.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum ArtifactAvailability {
    /// There is one. **T81 verifies it**; T80 only says it exists.
    Published {
        /// Where it comes from.
        url: String,
        /// What it must hash to.
        sha256: String,
    },

    /// The manifest publishes artifacts, and none for this machine.
    ///
    /// **A state, not an error** — the same shape T83 gives "MixDB is not installed". A client
    /// renders it as an absent affordance and says which systems it is published for.
    OtherTargets {
        /// The targets it does publish for, in the manifest's own words.
        targets: Vec<String>,
    },

    /// It downloads nothing: a `recipe`, or a `desktop-app` that is only detected.
    NotRequired,
}

/// One port an extension asked for.
///
/// **A wish and not a reservation.** Allocation happens when a row is written, which is T81's, and
/// nothing here holds a number. A line that read like a reservation is how somebody concludes a
/// port is theirs.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct PortWish {
    /// The name `[ports]` gave it, which is also its placeholder.
    pub name: String,

    /// The number it would like.
    pub wanted: u16,
}

/// The site a `web-app` would be given — roadmap task **T81b**.
///
/// Shown in the plan so the name that will be taken and the PHP it will run on are read before
/// anything is fetched; **not part of the consent**, because it is derived from a manifest the
/// person already agreed to.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct PlannedSite {
    /// `<label>.mixengine.test`.
    pub domain: String,

    /// The pool it would run on — the newest installed PHP inside `[web-app.runtime].requires`.
    pub pool: ServiceId,
}

/// What a `web-app` would serve.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct WebAppSummary {
    /// The document root, rendered.
    pub root: String,

    /// The label its internal domain is built from.
    pub domain: String,

    /// Which language it needs.
    pub runtime: RuntimeKind,

    /// Which versions of it will do.
    ///
    /// **MixEngine picks inside this**, and deliberately not the user's project: an administrative
    /// interface that broke because somebody pinned their project to an older PHP would be a tool
    /// that fails exactly when it is needed.
    pub requires: VersionConstraint,
}

/// What a `desktop-app` would be opened with.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct DesktopAppSummary {
    /// The URL scheme a handoff is written to.
    pub scheme: String,

    /// How this system would look for it, where the manifest says.
    ///
    /// **Declared only.** Locating an installed application is platform-layer work and is T83's;
    /// what belongs in a manifest is the name each system looks it up by.
    pub detect: Option<String>,
}

/// One thing an extension adds to generated configuration.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum RecipeAddition {
    /// A setting for every managed PHP.
    PhpIni {
        /// The ini key.
        key: String,
        /// Its value, rendered.
        value: String,
    },

    /// Directives added to the front end's configuration.
    FrontEnd {
        /// The fragment, rendered.
        fragment: String,
    },
}

/// Where an install gets its manifest — roadmap task **T81**.
///
/// **`ExtensionOrigin` and not `ExtensionSource`**: that name belongs to a *PHP* extension's
/// `runtime_api`, which is the same collision `php_extensions.rs` was renamed for in T80.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum ExtensionOrigin {
    /// The published registry, whose signature is checked when it is read.
    Registry {
        /// Which entry.
        id: ExtensionId,
    },

    /// A directory on this machine. **Nothing vouches for one of these.**
    Path {
        /// The directory holding `extension.toml`, or that file. Absolute, as every path in this
        /// API is.
        path: String,
    },
}

/// Ask what installing something here would do.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ExtensionPlanRequest {
    /// What to read.
    pub source: ExtensionOrigin,
}

/// What installing it here would do, and what a person is being asked to agree to.
#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct ExtensionPlan {
    /// Its id.
    pub id: ExtensionId,

    /// Its display name.
    pub name: String,

    /// Its own version.
    pub version: PackageVersion,

    /// What it is.
    pub kind: ExtensionKind,

    /// What it is for.
    pub description: String,

    /// Whether a signature covered it. `false` for every `--path` install, and every surface that
    /// renders this says so loudly.
    pub signed: bool,

    /// What it declares.
    ///
    /// **`services` is a disclosure and not a boundary** — see [`ApiAccess`](crate::ApiAccess) and
    /// ADR 0014.
    pub permissions: ExtensionPermissions,

    /// The ports it asks for. **Asked for, not held.**
    pub ports: Vec<PortWish>,

    /// Where its own files would go.
    pub install_dir: String,

    /// Where what it writes would go — outside `install_dir`, so an uninstall can keep it.
    pub data_dir: String,

    /// The site it would be served on, for a `web-app` — roadmap task **T81b**.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub site: Option<PlannedSite>,
}

/// Agreement to install one extension, naming what was read.
///
/// **It names what was shown rather than saying yes** — [`ScaffoldConsent`](crate::ScaffoldConsent)'s
/// shape, and for its reason: the registry can be refreshed between the plan somebody read and the
/// install they sent, and a consent naming the version and the reach they were shown is the only
/// kind that cannot be spent on a different one. Disagreement in either direction refuses the
/// install before anything is fetched.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct ExtensionConsent {
    /// The extension as it was shown.
    pub id: ExtensionId,

    /// The version as it was shown.
    pub version: PackageVersion,

    /// Whether the person was told nothing vouches for this.
    pub signed: bool,

    /// The reach they were shown.
    pub network: NetworkReach,
}

/// Install an extension.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ExtensionInstall {
    /// What to install.
    pub source: ExtensionOrigin,

    /// What the person agreed to, from the plan they were shown.
    pub consent: ExtensionConsent,
}

/// Remove an installed extension.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ExtensionUninstall {
    /// Which one.
    pub id: ExtensionId,

    /// Whether to delete its data directory as well.
    ///
    /// **`false` is the answer a client sends when nobody said**, because this is the one thing an
    /// uninstall cannot give back.
    #[serde(default)]
    pub delete_data: bool,
}

/// Name one installed extension — for `start` and `stop`.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ExtensionTarget {
    /// Which one.
    pub id: ExtensionId,
}

/// What an uninstall did.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct ExtensionRemoval {
    /// Which extension went.
    pub id: ExtensionId,

    /// The service that went with it, where there was one.
    pub service: Option<ServiceId>,

    /// The data directory that was **kept**, so a client can say where it still is.
    pub data_dir_kept: Option<String>,

    /// The domain that was released with it, for a `web-app` — roadmap task **T81b**.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub site: Option<String>,
}

/// One installed extension, as a listing shows it.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct ExtensionSummary {
    /// Its id.
    pub id: ExtensionId,

    /// Its display name.
    pub name: String,

    /// Its own version.
    pub version: PackageVersion,

    /// What it is.
    pub kind: ExtensionKind,

    /// Whether a signature covered it when it arrived.
    ///
    /// **Decided once, when it was installed**, exactly as a blueprint's trust is (T79b): this is a
    /// record of what was checked, never a re-check.
    pub signed: bool,

    /// The service it runs as, where it runs one.
    pub service: Option<ServiceId>,

    /// The ports it holds, by the name each was asked for under.
    pub ports: Vec<PortWish>,

    /// The domain it is served on, for a `web-app` — roadmap task **T81b**.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub site: Option<String>,
}

/// Every extension this home has installed.
///
/// **`InstalledExtensions` and not `ExtensionList`**, for [`ExtensionOrigin`]'s reason: the shorter
/// name is a *PHP* extension listing's.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct InstalledExtensions {
    /// What is installed, by id.
    pub extensions: Vec<ExtensionSummary>,
}

/// One extension the registry publishes.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct ExtensionOffer {
    /// Its id.
    pub id: ExtensionId,

    /// Its display name.
    pub name: String,

    /// The published version.
    pub version: PackageVersion,

    /// What it is.
    pub kind: ExtensionKind,

    /// What it is for.
    pub description: String,

    /// Whether this home already has it.
    pub installed: bool,

    /// Whether this machine has an artifact to install.
    pub artifact: ArtifactAvailability,
}

/// What the registry publishes.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct ExtensionCatalogue {
    /// The entries this build can read.
    pub extensions: Vec<ExtensionOffer>,

    /// How many entries it could not — roadmap task **T81**, the design's D4.
    ///
    /// **Said rather than swallowed.** An extension missing from a listing is one somebody goes
    /// looking for in the wrong place, and "your MixEngine is older than this entry" is an answer
    /// nothing else can give them.
    pub unreadable: usize,

    /// Whether this came from a cache past its freshness because the network could not be reached.
    pub stale: bool,
}
