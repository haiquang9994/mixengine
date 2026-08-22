//! What `project.*` asks and answers: a directory this home has been told about, and the versions
//! it pins.
//!
//! The same split `package_api` draws — this module is the API surface, and there is no
//! `project.rs` beside it, because what a project *is* has no vocabulary of its own: a name, a
//! directory and a map of constraints is the whole of it.
//!
//! **The database is the source of truth and the manifest is input** (spec D1). A create writes a
//! row and does not touch the user's repository; [`ProjectExport`] is what a person asks for when
//! they want the file a colleague will read. But the manifest *outranks* the row at resolve time,
//! so a row pin the manifest contradicts can never take effect — which is why [`ProjectPin`]
//! carries where it was read from and whether anything installed satisfies it, rather than leaving
//! somebody to read `8.3` in a window while their shell runs `8.4`.

use std::collections::BTreeMap;

use crate::{PackageVersion, RuntimeKind, VersionConstraint};

/// Which project a call is about.
///
/// **Two ways in, because a person has two.** A GUI holds the name it listed; a shell is *inside*
/// the directory and knows nothing else. [`ProjectRef::Path`] is resolved by walking up to the
/// nearest registered root, which is what the shim already does two feet away — so `mix project
/// show` typed three directories deep finds the project.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ProjectRef {
    /// By the name `project.list` shows, which is `projects.name` and is unique.
    Name(String),

    /// By any directory at or inside its root. The **nearest** registered root wins.
    Path(String),
}

/// Register a directory as a project.
///
/// One method rather than a create and an import (spec D2): both produce one row, and the only
/// difference is where `name` and `pins` came from. Each falls through — the argument, then the
/// `mixengine.toml` at `root`, then the directory's own base name for `name` and nothing for
/// `pins`.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct ProjectCreate {
    /// The project's root, which has to be an absolute directory that exists.
    pub root: String,

    /// What to call it. Falls through to `[project] name`, then to the directory's base name.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,

    /// The versions it wants. Falls through to `[runtimes]`, then to no pins at all.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub pins: Option<BTreeMap<RuntimeKind, VersionConstraint>>,
}

/// Change one of the three things a project row holds.
///
/// **`pins` replaces rather than merges** (spec D6): an absent field means unchanged, and `{}`
/// clears every pin. A merge would leave no way to remove one.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct ProjectUpdate {
    /// Which project.
    pub project: ProjectRef,

    /// A new name, validated exactly as a create's is.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,

    /// A new root — for a repository that moved, which is the only reason this exists.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub root: Option<String>,

    /// The pins, replacing whatever the row held.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub pins: Option<BTreeMap<RuntimeKind, VersionConstraint>>,
}

/// Which project `project.show`, `project.delete` and `project.export` are about.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct ProjectQuery {
    /// Which project.
    pub project: ProjectRef,
}

/// What `project.list` answers.
///
/// An object around the list rather than a bare array, on [`PackageList`](crate::PackageList)'s
/// precedent: a field can be added beside it without changing every existing client's parser.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct ProjectList {
    /// Every registered project, in name order.
    pub projects: Vec<ProjectSummary>,
}

/// One registered project.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct ProjectSummary {
    /// What it is called, which is also how it is addressed.
    pub name: String,

    /// Its root directory, spelled the way the filesystem spells it.
    pub root: String,

    /// When it was registered, as `YYYY-MM-DDTHH:MM:SSZ`.
    pub created_at: String,

    /// The `mixengine.toml` at the root, when there is one.
    ///
    /// Which is what decides whether the row's pins can take effect at all — the file outranks the
    /// row, so a listing that did not say whether one is there would be a listing missing the
    /// reason.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub manifest: Option<String>,
}

/// One project and what it actually resolves to.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct ProjectDetail {
    /// The project itself.
    pub project: ProjectSummary,

    /// Its pins in **effective** order — the manifest's entry where there is one, the row's where
    /// there is not — because that is what the shim will do, and a panel showing anything else is
    /// a panel that lies.
    pub pins: Vec<ProjectPin>,
}

/// One language's pin, where it was read, and whether this machine can satisfy it.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct ProjectPin {
    /// Which language.
    pub kind: RuntimeKind,

    /// What was asked for.
    pub constraint: VersionConstraint,

    /// Which of the two would win at resolve time, and where it was read.
    pub source: PinSource,

    /// The installed version this resolves to today, or [`None`] when nothing does.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub resolved: Option<PackageVersion>,

    /// When it resolves to nothing: the command that would fix it.
    ///
    /// The same sentence a failed resolution already gives, so a person meets one wording rather
    /// than two.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub hint: Option<String>,
}

/// Where an effective pin was read.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(tag = "from", rename_all = "snake_case")]
pub enum PinSource {
    /// The row in this home.
    Registered,

    /// `[runtimes]` in a manifest, which outranks the row.
    Manifest {
        /// The file that decided it, because that is what somebody would go and edit.
        path: String,
    },
}

/// What `project.delete` answers.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct ProjectRemoval {
    /// The project that is gone.
    pub removed: ProjectSummary,

    /// The directory, left exactly as it was and said out loud.
    ///
    /// [`ServiceRemoval`](crate::ServiceRemoval)'s rule: a directory nobody was told about is a
    /// directory nobody ever cleans up.
    pub root_kept: String,

    /// Its manifest, when there was one, on the same reasoning.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub manifest_kept: Option<String>,
}

/// What `project.export` answers.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct ProjectExport {
    /// The file that was written, which is `<root>/mixengine.toml` and nowhere else.
    pub path: String,

    /// Whether the file was made, or an existing one merged into.
    pub created: bool,

    /// The sites that were not written, because a manifest holds one `[site]` (spec D9).
    ///
    /// Their primary domains, so a person knows what the file does not say — a limit of the file
    /// format rather than of the model.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub sites_omitted: Vec<String>,
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    /// A project is addressed either way round, and the tag is the word a person types.
    #[test]
    fn a_project_is_named_by_its_name_or_by_any_directory_inside_it() {
        let by_name: ProjectRef = serde_json::from_value(json!({"name": "blog"})).expect("a name");
        let by_path: ProjectRef =
            serde_json::from_value(json!({"path": "/srv/blog/public"})).expect("a path");

        assert_eq!(by_name, ProjectRef::Name("blog".to_owned()));
        assert_eq!(by_path, ProjectRef::Path("/srv/blog/public".to_owned()));
        assert_eq!(
            serde_json::to_value(&by_name).expect("it serialises"),
            json!({"name": "blog"}),
            "the wire spells it the way it was read"
        );
    }

    /// `project.create { root }` in a directory holding a colleague's manifest *is* the import —
    /// D2 — so both optional halves have to be absent-able.
    #[test]
    fn a_create_that_names_only_a_directory_is_a_whole_request() {
        let create: ProjectCreate =
            serde_json::from_value(json!({"root": "/srv/blog"})).expect("a create");

        assert_eq!(create.name, None);
        assert_eq!(create.pins, None, "absent is not the same as an empty map");
    }

    /// D6: an absent `pins` leaves them unchanged, an empty one clears them, and the two must not
    /// decode to the same value.
    #[test]
    fn an_update_tells_unchanged_apart_from_cleared() {
        let unchanged: ProjectUpdate =
            serde_json::from_value(json!({"project": {"name": "blog"}})).expect("an update");
        let cleared: ProjectUpdate =
            serde_json::from_value(json!({"project": {"name": "blog"}, "pins": {}}))
                .expect("an update");

        assert_eq!(unchanged.pins, None);
        assert_eq!(cleared.pins, Some(std::collections::BTreeMap::new()));
    }
}
