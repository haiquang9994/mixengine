//! Requests in the `blueprint.*` namespace — roadmap task **T77**.

use crate::ProjectRef;

/// `blueprint.capture` — write down what a project is made of.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct BlueprintCapture {
    /// Which project. The CLI fills this from the current directory when nobody named one.
    pub project: ProjectRef,

    /// The slug to file it under, which becomes the row's key and the rendered file's stem.
    pub name: String,

    /// What it is for.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,

    /// Whether to replace a blueprint already filed under this slug.
    ///
    /// Refusing by default is what stops a second capture quietly overwriting the first. There is
    /// no `blueprint.delete` in this build, so without this flag a mistyped name would be
    /// permanent — which is a worse default than asking.
    #[serde(default)]
    pub overwrite: bool,
}

/// `blueprint.apply` — what applying one would do, and (from T78) doing it.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct BlueprintApply {
    /// Which blueprint, by slug.
    pub blueprint: String,

    /// What the new project is called, and what `{project}` expands to.
    pub project: String,

    /// Where it would live. Absolute; the client resolves it against its own current directory.
    pub root: String,

    /// Whether to stop after planning.
    ///
    /// **`false` is answered with `Unsupported` in this build**, naming T78 (the T77 design, D12).
    /// Not a `todo!()`, and not a CLI that refuses to send it: a client renders what the daemon
    /// answers rather than holding the rule itself.
    #[serde(default)]
    pub dry_run: bool,
}
