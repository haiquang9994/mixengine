//! `extension.inspect` — roadmap task **T80**.
//!
//! One method, and it is read-only: T80 installs nothing. What it answers is what an install on
//! this machine *would* produce, which is `blueprint.apply --dry-run`'s position — a plan is worth
//! having because it was computed rather than because it was described.
//!
//! The same split [`crate::job_api`] draws over [`crate::job`]: [`crate::extension`] is the
//! vocabulary an extension is *described* in, and this is what one call asks and answers.

use crate::{
    ExtensionId, ExtensionKind, ExtensionPermissions, PackageVersion, RuntimeKind, ServiceSpec,
    VersionConstraint,
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
