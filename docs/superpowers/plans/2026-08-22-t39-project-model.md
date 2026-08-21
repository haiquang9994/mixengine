# T39 — Project model Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Make step 3 of the version-resolution order — *a project registered in this home* — actually run, by giving MixEngine a `project.*` namespace, one reader/writer for `mixengine.toml`, and the `runtime.uninstall` refusal that a project pin was always supposed to produce.

**Architecture:** The `projects` table already exists in `0001_initial.sql`, so nothing here is a migration. A new `core::projects` module owns every write to it and lifts `resolve::in_a_project`'s ancestor walk into `find`, so "which project is this directory in?" has one implementation. A new `core::manifest` becomes the single reader of `mixengine.toml` and the only writer — editing through `toml_edit` so a user's comments, key order and hand-written `[site]` survive an export. The daemon gets a `projects.rs` beside `packages.rs` and six RPC arms; `mix` gets `mix project …` and a `--force` on `mix runtime uninstall`.

**Tech Stack:** Rust 2024 (rust-version 1.97.1), `sqlx` with compile-time-checked queries against SQLite, `serde`/`serde_json`, `toml` 1.1.4 for reading, **`toml_edit` 0.25 (new direct dependency)** for writing, `clap` 4 derive for the CLI, `tokio` + `hyper` for the daemon and its tests.

**Spec:** [docs/superpowers/specs/2026-08-22-t39-project-model-design.md](../specs/2026-08-22-t39-project-model-design.md) — read it before Task 1. Decisions are cited below as **D1**–**D10** and the spec is the authority on every one of them.

## Global Constraints

- **No business logic in clients.** `mix` renders what the daemon returns and decides nothing. Every refusal's wording is the daemon's.
- **No direct OS calls outside `mixengine-platform`.** Path normalisation goes through `mixengine_platform::paths::in_full` and nowhere else; no `#[cfg(windows)]` in core or daemon code.
- **No migration.** `crates/mixengine-core/migrations/0001_initial.sql` already declares `projects(id, name, root_path, runtime_pins_json, created_at, blueprint_id)` with `name` and `root_path` both `UNIQUE`. Never edit a released migration; never add one for this task.
- **`projects.blueprint_id` stays `NULL`.** No method in this task writes it — it is Phase 8's.
- **`created_at` is ISO-8601 text** (`Timestamp::to_rfc3339`), matching the column and `.claude/architecture/data-model.md`'s split: it is written once, read by a person, branched on by nobody.
- **The clock is the daemon's.** `core` never reads it: the daemon passes `Timestamp::from_system_time(SystemTime::now())`, exactly as `crates/mixengine-daemon/src/runtimes.rs:369` does.
- **Name validation, once:** non-empty after trimming, at most 64 characters, no control characters, no `/` and no `\` (**D4**).
- **Error codes are the twelve in `mixengine_proto::ErrorCode`.** The mapping this task uses: `invalid_argument`, `already_exists`, `not_found`, `precondition_failed`, `internal`, `io`.
- **Cross-platform or not merged.** Everything here must compile and pass on Windows, macOS and Linux. Tests that compare paths must not assume a separator.
- **After any `sqlx::query!`/`query_scalar!` change:** `cargo sqlx prepare --workspace -- --all-targets --all-features`, and commit the `.sqlx/` changes with the code.
- **Before every commit:** `cargo fmt --all --check`, `cargo clippy --workspace --all-targets -- -D warnings`, `cargo test --workspace`.
- **Commits happen only when the user asks for one.** The commit step at the end of each task is written out so it is ready; per `CLAUDE.md` do not run it unauthorised, and never add a `Co-Authored-By` trailer.
- **Branch:** `t39-project-model` (already checked out; the spec is on it, uncommitted).
- **Doc comments carry the decision, not the mechanics.** `#![warn(missing_docs)]` is on in `mixengine-core` and `mixengine-proto`: every public item needs a doc comment, and the house style is that a comment explains *why this and not the obvious alternative*.

### One deliberate deviation from the spec, recorded

**D9** describes `core::manifest::Manifest` as capturing `[site]` and `[[services]]` as raw `toml::Value` "until T39a gives them types". This plan does **not** add that field, because the reason for capturing it disappears once **D10** is implemented: the writer edits a `toml_edit::DocumentMut` in place and never reserialises a struct, so those sections survive an export whether or not the typed reader has ever seen them — which is what the capture was for. Nothing in T39 reads them. The round-trip test in Task 2 is the proof, and T39a adds the typed fields when it has something to do with them.

---

### Task 1: The wire vocabulary

**Files:**
- Create: `crates/mixengine-proto/src/project_api.rs`
- Modify: `crates/mixengine-proto/src/lib.rs` (module declaration + re-exports)
- Modify: `crates/mixengine-proto/src/runtime_api.rs` (add `RuntimeUninstall`)
- Modify: `crates/mixengine-proto/src/rpc.rs` (six method constants in `pub mod method`)
- Test: inline `#[cfg(test)] mod tests` in `crates/mixengine-proto/src/project_api.rs` and in `crates/mixengine-proto/src/runtime_api.rs`

**Interfaces:**
- Consumes: `crate::{PackageVersion, RuntimeKind, VersionConstraint}`, all already in this crate.
- Produces: `ProjectRef`, `ProjectCreate`, `ProjectUpdate`, `ProjectQuery`, `ProjectList`, `ProjectSummary`, `ProjectDetail`, `ProjectPin`, `PinSource`, `ProjectRemoval`, `ProjectExport`, `RuntimeUninstall`, and `rpc::method::{PROJECT_CREATE, PROJECT_LIST, PROJECT_SHOW, PROJECT_UPDATE, PROJECT_DELETE, PROJECT_EXPORT}`. Every later task uses these names verbatim.

- [ ] **Step 1: Write the failing tests**

Create `crates/mixengine-proto/src/project_api.rs` with *only* the test module at first, so the build fails for the reason the test names:

```rust
#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    /// A project is addressed either way round, and the tag is the word a person types.
    #[test]
    fn a_project_is_named_by_its_name_or_by_any_directory_inside_it() {
        let by_name: ProjectRef =
            serde_json::from_value(json!({"name": "blog"})).expect("a name");
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
```

- [ ] **Step 2: Run them to verify they fail**

Run: `cargo test -p mixengine-proto --lib project_api`
Expected: FAIL — `file not found for module 'project_api'`, then `cannot find type 'ProjectRef' in this scope`.

- [ ] **Step 3: Write the types**

Above the test module in `crates/mixengine-proto/src/project_api.rs`:

```rust
//! What `project.*` asks and answers: a directory this home has been told about, and the versions
//! it pins.
//!
//! The same split [`crate::package_api`] draws — this module is the API surface, and there is no
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
    /// The same sentence a failed resolution already gives — `core::resolve::install_command` — so
    /// a person meets one wording rather than two.
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
    /// [`ServiceRemoval::data_kept`](crate::ServiceRemoval)'s rule: a directory nobody was told
    /// about is a directory nobody ever cleans up.
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
}
```

- [ ] **Step 4: Add `RuntimeUninstall` and its test**

In `crates/mixengine-proto/src/runtime_api.rs`, after `RuntimeTarget`:

```rust
/// What `runtime.uninstall` takes: a version, and whether to cross a refusal.
///
/// Flattened rather than made a field on [`RuntimeTarget`]: that type is also `runtime.install`'s
/// and `runtime.set_default`'s parameter, where a `force` would mean nothing. The flatten keeps
/// today's wire shape and adds one optional key, so an older client's request still parses.
///
/// **It crosses the project-pin refusal and nothing else** (spec D8). A broken pin is a statement
/// about the future — the next `cd` into that directory fails with a message naming the install
/// that fixes it — and a person who has been shown the affected projects is entitled to decide. A
/// running php-fpm pool is a fact about the present, and no flag buys a live process with no files
/// under it.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct RuntimeUninstall {
    /// Which version.
    #[serde(flatten)]
    pub target: RuntimeTarget,

    /// Remove it even though a registered project pins it.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub force: bool,
}
```

And in that file's test module (create one if it has none):

```rust
    /// An older client sends what it always sent, and it still parses.
    #[test]
    fn an_uninstall_without_a_force_is_still_an_uninstall() {
        let asked: RuntimeUninstall =
            serde_json::from_value(serde_json::json!({"kind": "php", "version": "8.3.33"}))
                .expect("the shape every client has sent since T23");

        assert!(!asked.force);
        assert_eq!(asked.target.kind, crate::RuntimeKind::Php);
    }
```

- [ ] **Step 5: Declare the module, re-export the types, name the methods**

In `crates/mixengine-proto/src/lib.rs`, beside the other `pub mod` lines and in alphabetical position among the `pub use` block:

```rust
pub mod project_api;

pub use project_api::{
    PinSource, ProjectCreate, ProjectDetail, ProjectExport, ProjectList, ProjectPin, ProjectQuery,
    ProjectRef, ProjectRemoval, ProjectSummary, ProjectUpdate,
};
```

Add `RuntimeUninstall` to the existing `pub use runtime_api::{…}` list.

In `crates/mixengine-proto/src/rpc.rs`, at the end of `pub mod method`:

```rust
    /// Register a directory as a project. Takes [`ProjectCreate`](crate::ProjectCreate), answers
    /// the [`ProjectDetail`](crate::ProjectDetail) the new row became.
    ///
    /// **The import too.** `name` and `pins` fall through to the `mixengine.toml` lying at the
    /// root, so a create that names only a directory is how a colleague's checkout is adopted —
    /// see `.claude/features/runtime-versions.md` for what that file may say.
    pub const PROJECT_CREATE: &str = "project.create";

    /// Every registered project. Takes no parameters, answers
    /// [`ProjectList`](crate::ProjectList).
    pub const PROJECT_LIST: &str = "project.list";

    /// One of them, with its pins in effective order and whether each resolves today. Takes
    /// [`ProjectQuery`](crate::ProjectQuery), answers [`ProjectDetail`](crate::ProjectDetail).
    pub const PROJECT_SHOW: &str = "project.show";

    /// Change a project's name, root or pins. Takes [`ProjectUpdate`](crate::ProjectUpdate),
    /// answers the [`ProjectDetail`](crate::ProjectDetail) it now is.
    ///
    /// `pins` **replaces** rather than merges: absent means unchanged, `{}` clears them.
    pub const PROJECT_UPDATE: &str = "project.update";

    /// Forget a project. Takes [`ProjectQuery`](crate::ProjectQuery), answers
    /// [`ProjectRemoval`](crate::ProjectRemoval).
    ///
    /// **The directory is kept and named**, on `service.delete`'s reasoning: nothing about
    /// unregistering a project says anything about wanting somebody's repository gone.
    pub const PROJECT_DELETE: &str = "project.delete";

    /// Write the project into `<root>/mixengine.toml`. Takes
    /// [`ProjectQuery`](crate::ProjectQuery), answers [`ProjectExport`](crate::ProjectExport).
    ///
    /// **Merges rather than rewrites**: comments, key order and a hand-written `[site]` survive,
    /// because the file's whole purpose is to be read by a person.
    pub const PROJECT_EXPORT: &str = "project.export";
```

- [ ] **Step 6: Run the tests to verify they pass**

Run: `cargo test -p mixengine-proto`
Expected: PASS, including the existing suites.

- [ ] **Step 7: Lint and commit**

```bash
cargo fmt --all
cargo clippy -p mixengine-proto --all-targets -- -D warnings
git add crates/mixengine-proto
git commit -m "feat(proto): the project.* vocabulary, and a force on runtime.uninstall (T39)"
```

---

### Task 2: `core::manifest` — one reader, and a writer that edits

**Files:**
- Create: `crates/mixengine-core/src/manifest.rs`
- Modify: `crates/mixengine-core/src/lib.rs` (`pub mod manifest;`, and the new `Error::ManifestEdit` variant)
- Modify: `crates/mixengine-core/Cargo.toml` (add `toml_edit.workspace = true`)
- Modify: `Cargo.toml` (add `toml_edit` to `[workspace.dependencies]`)
- Modify: `crates/mixengine-daemon/src/error.rs` (map `Error::ManifestEdit`)
- Test: inline `#[cfg(test)] mod tests` in `crates/mixengine-core/src/manifest.rs`

**Interfaces:**
- Consumes: `crate::{Error, Result}`, `mixengine_proto::{RuntimeKind, VersionConstraint}`.
- Produces:
  - `pub const FILE_NAME: &str = "mixengine.toml";`
  - `pub struct Manifest { pub project: Option<Project>, pub runtimes: BTreeMap<RuntimeKind, VersionConstraint> }`
  - `pub struct Project { pub name: Option<String> }`
  - `pub fn at(directory: &Path) -> PathBuf`
  - `pub fn read(path: &Path) -> Result<Option<Manifest>>` — `Ok(None)` for a file that is not there or cannot be opened; `Err(Error::Manifest)` for one that does not parse.
  - `pub fn write(directory: &Path, name: &str, pins: &BTreeMap<RuntimeKind, VersionConstraint>) -> Result<bool>` — `true` when the file was created.
  - `Error::ManifestEdit { path: PathBuf, reason: String }`

- [ ] **Step 1: Add the dependency**

In the root `Cargo.toml`, in `[workspace.dependencies]`, in alphabetical position beside `toml`:

```toml
# Writing `mixengine.toml` back (T39). `toml` cannot: a `Value` carries no formatting, so
# serialising a fresh document over the user's file would take their comments, their key order and
# the `[site]` block they wrote by hand with it — to the one file whose entire purpose is to be
# read by a person. Already in `Cargo.lock` at this version underneath `toml` itself, so this is a
# new *direct* edge rather than a new subtree.
toml_edit = "0.25.13"
```

In `crates/mixengine-core/Cargo.toml`, under `[dependencies]` after `toml.workspace = true`:

```toml
toml_edit.workspace = true
```

- [ ] **Step 2: Write the failing tests**

Create `crates/mixengine-core/src/manifest.rs` containing only:

```rust
#[cfg(test)]
mod tests {
    use super::*;

    fn somewhere() -> tempfile::TempDir {
        tempfile::tempdir().expect("a temporary directory")
    }

    fn pins(entries: &[(RuntimeKind, &str)]) -> BTreeMap<RuntimeKind, VersionConstraint> {
        entries
            .iter()
            .map(|(kind, text)| {
                (
                    *kind,
                    VersionConstraint::parse((*text).to_owned()).expect("a constraint"),
                )
            })
            .collect()
    }

    /// The whole file, where `resolve` used to read a third of it — and the sections this build has
    /// no types for still must not make it refuse the file.
    #[test]
    fn a_manifest_declaring_more_than_runtimes_is_read_whole() {
        let home = somewhere();
        std::fs::write(
            at(home.path()),
            "[project]\nname = \"blog\"\n\n[runtimes]\nphp = \"^8.3\"\n\n\
             [site]\ndomain = \"blog.test\"\n\n[[services]]\nname = \"redis\"\n",
        )
        .expect("a manifest");

        let manifest = read(&at(home.path()))
            .expect("it parses")
            .expect("it is there");

        assert_eq!(
            manifest.project.and_then(|project| project.name).as_deref(),
            Some("blog")
        );
        assert_eq!(
            manifest.runtimes.get(&RuntimeKind::Php).map(VersionConstraint::as_str),
            Some("^8.3")
        );
    }

    /// A directory with no manifest is not a failure — it is the ordinary case.
    #[test]
    fn a_directory_with_no_manifest_answers_nothing_rather_than_failing() {
        let home = somewhere();

        assert_eq!(read(&at(home.path())).expect("no manifest is fine"), None);
    }

    /// A pin that does nothing looks exactly like a pin that does not work, so the file is refused
    /// by name — `Error::Manifest`'s own reasoning, kept when the reader moved here.
    #[test]
    fn a_manifest_that_does_not_parse_names_itself() {
        let home = somewhere();

        for body in [
            "[runtimes]\nphp = \"~8.3\"\n",
            "[runtimes]\nphhp = \"8.3\"\n",
            "[runtimes\n",
        ] {
            std::fs::write(at(home.path()), body).expect("a manifest");

            let error = read(&at(home.path())).expect_err("the manifest is wrong");

            assert!(
                matches!(&error, Error::Manifest { path, .. } if path.ends_with(FILE_NAME)),
                "{error:?} for {body:?}"
            );
        }
    }

    /// **What D10 is for.** An export is written into somebody's version-controlled file, so
    /// everything it does not own survives it byte for byte.
    #[test]
    fn writing_a_manifest_keeps_the_comments_the_order_and_the_sections_it_does_not_own() {
        let home = somewhere();
        let original = "# the blog\n\
                        [runtimes]\n\
                        node = \"22\"      # the front end build\n\
                        php = \"8.2\"\n\n\
                        [site]\n\
                        domain = \"blog.test\"\n\n\
                        [[services]]\n\
                        name = \"redis\"\n";
        std::fs::write(at(home.path()), original).expect("a manifest");

        let created = write(
            home.path(),
            "blog",
            &pins(&[(RuntimeKind::Php, "^8.3")]),
        )
        .expect("it is written");

        let after = std::fs::read_to_string(at(home.path())).expect("the file");

        assert!(!created, "the file was already there");
        assert!(after.contains("# the blog"), "{after}");
        assert!(after.contains("# the front end build"), "{after}");
        assert!(after.contains("[site]") && after.contains("blog.test"), "{after}");
        assert!(after.contains("[[services]]"), "{after}");
        assert!(
            after.find("node =").expect("node") < after.find("php =").expect("php"),
            "the key order the user chose is theirs: {after}"
        );
        assert!(after.contains("php = \"^8.3\""), "the owned key changed: {after}");
        assert!(after.contains("name = \"blog\""), "the name was written: {after}");

        // And what it wrote is what the reader reads back.
        let manifest = read(&at(home.path())).expect("it parses").expect("it is there");
        assert_eq!(
            manifest.runtimes.get(&RuntimeKind::Php).map(VersionConstraint::as_str),
            Some("^8.3")
        );
    }

    /// A directory with no manifest gets one, and says that it did.
    #[test]
    fn a_directory_with_no_manifest_gets_one_written() {
        let home = somewhere();

        let created = write(home.path(), "blog", &pins(&[(RuntimeKind::Php, "8.3")]))
            .expect("it is written");

        assert!(created);
        assert_eq!(
            read(&at(home.path()))
                .expect("it parses")
                .expect("it is there")
                .project
                .and_then(|project| project.name)
                .as_deref(),
            Some("blog")
        );
    }
}
```

- [ ] **Step 3: Run them to verify they fail**

Run: `cargo test -p mixengine-core --lib manifest`
Expected: FAIL — the module is not declared, then `cannot find function 'read' in this scope`.

- [ ] **Step 4: Write the module**

Above the tests in `crates/mixengine-core/src/manifest.rs`:

```rust
//! `mixengine.toml` — the file a project pins its runtimes in, read and written in one place.
//!
//! **One reader** (spec D9). [`crate::resolve`] used to deserialise a deliberately narrow struct of
//! its own: `[runtimes]` and nothing else. T39 needs `[project] name` on import and a writer for
//! export, and two structs describing one file would be two answers to one question — so the narrow
//! one is gone and `resolve` is a caller.
//!
//! **Unknown sections are allowed through**, exactly as they were: the file also declares a site and
//! its services, which are T39a's, and a `deny_unknown_fields` here would make this build refuse the
//! manifests that task is going to write. What is still closed is the map inside `[runtimes]`: a key
//! naming a language MixEngine does not manage is a pin that would silently do nothing, which is
//! `config.toml`'s rule about typos in the one place it still applies.
//!
//! # The writer edits; it does not rewrite
//!
//! This file lives in the user's repository, under version control, with their comments and their
//! key order in it — and, after T39a, a `[site]` block they wrote by hand. Serialising a fresh
//! document over it would destroy all of that, and would do it to the one file whose entire purpose
//! is to be read by a person. So [`write`] edits a `toml_edit` document: it sets `[project] name`
//! and the `[runtimes]` keys it owns, and leaves every other byte alone.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use mixengine_proto::{RuntimeKind, VersionConstraint};

use crate::{Error, Result};

/// The file a project pins its runtimes in, checked into the user's repository.
pub const FILE_NAME: &str = "mixengine.toml";

/// `mixengine.toml`, as this build understands it.
///
/// Two sections and no catch-all: what the writer preserves it preserves through the document it
/// edits rather than through a field nothing reads, so `[site]` and `[[services]]` survive an export
/// without this type having to hold them until T39a gives them meaning.
#[derive(Debug, Default, PartialEq, Eq, serde::Deserialize)]
pub struct Manifest {
    /// `[project]`, when the file has one.
    #[serde(default)]
    pub project: Option<Project>,

    /// The versions this project wants, by language.
    #[serde(default)]
    pub runtimes: BTreeMap<RuntimeKind, VersionConstraint>,
}

/// `[project]`.
#[derive(Debug, Default, PartialEq, Eq, serde::Deserialize)]
pub struct Project {
    /// What the project is called, when the file says.
    #[serde(default)]
    pub name: Option<String>,
}

/// Where a directory's manifest is.
#[must_use]
pub fn at(directory: &Path) -> PathBuf {
    directory.join(FILE_NAME)
}

/// Read one, or [`None`] where there is none to read.
///
/// **A file that cannot be opened is treated as one that is not there**, which is the rule
/// [`crate::resolve`] has always followed and the reason it can walk to the root: the ancestor walk
/// passes through other people's directories on the way up, and a permission error three levels
/// above somebody's project is not a fact about their project.
///
/// # Errors
///
/// [`Error::Manifest`] for a file that does not parse — including a `[runtimes]` key naming a
/// language this build does not manage — and [`Error::Io`] for a read that failed some other way.
pub fn read(path: &Path) -> Result<Option<Manifest>> {
    let text = match std::fs::read_to_string(path) {
        Ok(text) => text,
        Err(error)
            if matches!(
                error.kind(),
                std::io::ErrorKind::NotFound | std::io::ErrorKind::PermissionDenied
            ) =>
        {
            return Ok(None);
        }
        Err(source) => {
            return Err(Error::Io {
                action: "read",
                path: path.to_path_buf(),
                source,
            });
        }
    };

    toml::from_str(&text)
        .map(Some)
        .map_err(|source| Error::Manifest {
            path: path.to_path_buf(),
            source,
        })
}

/// Set `[project] name` and these `[runtimes]` keys in `<directory>/mixengine.toml`.
///
/// Answers whether the file had to be created. Keys this call does not name are left as they are —
/// a pin the user wrote and MixEngine does not know about is still theirs.
///
/// # Errors
///
/// [`Error::Manifest`] for an existing file that does not parse — refused before a byte is written,
/// so a broken manifest is never made worse — [`Error::ManifestEdit`] for one that parses as TOML
/// but not as a document this can edit, and [`Error::Io`] when the file cannot be read or written.
pub fn write(
    directory: &Path,
    name: &str,
    pins: &BTreeMap<RuntimeKind, VersionConstraint>,
) -> Result<bool> {
    let path = at(directory);

    // Validated through the reader first, so the failure a caller sees for a broken file is the
    // same `Error::Manifest` every other door gives it, naming the same path.
    let created = read(&path)?.is_none() && !path.exists();

    let text = match std::fs::read_to_string(&path) {
        Ok(text) => text,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => String::new(),
        Err(source) => {
            return Err(Error::Io {
                action: "read",
                path,
                source,
            });
        }
    };

    let mut document: toml_edit::DocumentMut =
        text.parse().map_err(|error: toml_edit::TomlError| Error::ManifestEdit {
            path: path.clone(),
            reason: error.to_string(),
        })?;

    set(&mut document, "project", |table| {
        table["name"] = toml_edit::value(name);
    });

    set(&mut document, "runtimes", |table| {
        for (kind, constraint) in pins {
            table[kind.as_str()] = toml_edit::value(constraint.as_str());
        }
    });

    std::fs::write(&path, document.to_string()).map_err(|source| Error::Io {
        action: "write",
        path,
        source,
    })?;

    tracing::info!(path = %at(directory).display(), created, "a project manifest was written");

    Ok(created)
}

/// Reach one top-level table, creating it if the file has none, and edit it.
///
/// `set_implicit(false)` is what makes a created table render its own `[header]`: a table
/// toml_edit believes is implicit is one it prints only through its children, and a `[project]`
/// that never appears is a file the reader is right about and a person is confused by.
fn set(document: &mut toml_edit::DocumentMut, section: &str, edit: impl FnOnce(&mut toml_edit::Table)) {
    let item = document
        .entry(section)
        .or_insert_with(|| toml_edit::Item::Table(toml_edit::Table::new()));

    if let Some(table) = item.as_table_mut() {
        table.set_implicit(false);
        edit(table);
    }
}
```

- [ ] **Step 5: Declare the module and add the error variant**

In `crates/mixengine-core/src/lib.rs`, beside the other `pub mod` lines:

```rust
pub mod manifest;
```

And in `pub enum Error`, immediately after the `Manifest` variant:

```rust
    /// A `mixengine.toml` that parses but could not be edited in place.
    ///
    /// [`Error::Manifest`]'s sibling on the write path, and separate from it because the two are
    /// different accusations: the first says the user's file is wrong, and this says this build
    /// could not put something into a file that is right. The reason is carried as text rather than
    /// as the editor's own error type, so the shape of a dependency does not become part of this
    /// enum.
    #[error("{} could not be edited: {reason}", path.display())]
    ManifestEdit {
        /// The manifest that could not be edited.
        path: PathBuf,
        /// What the editor said about it.
        reason: String,
    },
```

In `crates/mixengine-daemon/src/error.rs`, extend the existing `Core::Manifest` arm's neighbourhood:

```rust
            // The user's file again, and the same code: what a person does about either is open the
            // file. The hint differs because the repair does — nothing here is about `[runtimes]`.
            Core::ManifestEdit { path, .. } => Error::new(ErrorCode::InvalidArgument, chain(self))
                .with_hint(format!(
                    "{} could not be rewritten with the project in it — check that it is a TOML \
                     file this user can write",
                    path.display()
                )),
```

- [ ] **Step 6: Run the tests to verify they pass**

Run: `cargo test -p mixengine-core --lib manifest`
Expected: PASS — five tests.

- [ ] **Step 7: Lint and commit**

```bash
cargo fmt --all
cargo clippy --workspace --all-targets -- -D warnings
git add Cargo.toml Cargo.lock crates/mixengine-core crates/mixengine-daemon/src/error.rs
git commit -m "feat(core): one reader and one writer for mixengine.toml (T39)"
```

---

### Task 3: `resolve` reads through `core::manifest`, and the bench says whether it may

**Files:**
- Modify: `crates/mixengine-core/src/resolve.rs` (delete the private `Manifest`, call `manifest::read`, re-export the file-name constant, rewrite the module note)
- Test: the existing `#[cfg(test)] mod tests` in `crates/mixengine-core/src/resolve.rs` (unchanged — it is the regression net)

**Interfaces:**
- Consumes: `crate::manifest::{at, read, FILE_NAME}` from Task 2.
- Produces: `resolve::MANIFEST_FILE_NAME` continues to exist as a re-export, so no caller of it changes.

- [ ] **Step 1: Run the existing suite so the baseline is a fact, not a memory**

Run: `cargo test -p mixengine-core --lib resolve`
Expected: PASS — this is what must still pass in step 4.

- [ ] **Step 2: Rewire the reader**

In `crates/mixengine-core/src/resolve.rs`:

Replace the `MANIFEST_FILE_NAME` definition with a re-export:

```rust
/// The file a project pins its runtimes in, checked into the user's repository.
///
/// Owned by [`crate::manifest`] since T39, and still named here because this is where every caller
/// learned it.
pub use crate::manifest::FILE_NAME as MANIFEST_FILE_NAME;
```

Replace the body of `in_a_manifest` with:

```rust
fn in_a_manifest(kind: RuntimeKind, cwd: &Path) -> Result<Option<Asked>> {
    for directory in cwd.ancestors() {
        let path = crate::manifest::at(directory);

        let Some(manifest) = crate::manifest::read(&path)? else {
            continue;
        };

        if let Some(constraint) = manifest.runtimes.get(&kind) {
            return Ok(Some(Asked {
                constraint: constraint.clone(),
                source: RuntimeSource::Manifest {
                    path: path.display().to_string(),
                },
            }));
        }
    }

    Ok(None)
}
```

Delete the private `struct Manifest` and its doc comment entirely — its reasoning now lives on `crate::manifest::Manifest`.

Replace the module-level note that says the table is always empty:

```rust
//! # One of the four sources reads something that is often not there
//!
//! A `mixengine.toml` is optional, and most directories have none. A project record is the other
//! half of the same walk and **is** reachable now: T39 gave this build `project.*`, so step 3 stops
//! being a step nothing has ever taken. Both are walked to the top before the next one starts,
//! which is the order the feature spec lists and not the same thing as one walk asking both
//! questions per directory.
```

- [ ] **Step 3: Run the resolve suite**

Run: `cargo test -p mixengine-core --lib resolve`
Expected: PASS — every test that passed in step 1, unchanged. `a_manifest_declaring_more_than_runtimes_is_still_read` and `a_manifest_that_does_not_parse_names_itself` are the two that prove the swap kept the behaviour.

- [ ] **Step 4: Run the bench that decides whether this may stay (D9's condition)**

The shim resolves in its own process on every `php`, and T29 measured it against a 15 ms budget. A full-file deserialise now sits on that walk, and the spec's condition is explicit: **the measurement decides this, not the document.**

```bash
cargo build --release -p mixengine-testkit --bin fakeservice
cargo test --release -p mixengine-shim --test overhead -- --ignored --nocapture --test-threads=1
```

Expected: PASS, with the printed overhead inside the budget the test asserts.

**If it fails:** revert this task's change to `in_a_manifest` only — restore the narrow private `struct Manifest` in `resolve.rs`, keep `core::manifest` as the write path — and add this test to `crates/mixengine-core/src/manifest.rs`, which is what stops the two readers drifting:

```rust
    /// The two readers agree about the one section they share — the price of keeping the narrow one.
    #[test]
    fn the_resolvers_narrow_reading_of_runtimes_matches_this_one() {
        let home = somewhere();
        std::fs::write(at(home.path()), "[runtimes]\nphp = \"^8.3\"\nnode = \"22\"\n")
            .expect("a manifest");

        let whole = read(&at(home.path())).expect("it parses").expect("it is there");

        assert_eq!(
            whole.runtimes,
            pins(&[(RuntimeKind::Php, "^8.3"), (RuntimeKind::Node, "22")])
        );
    }
```

Record which way it went in the commit message either way.

- [ ] **Step 5: Lint and commit**

```bash
cargo fmt --all
cargo clippy -p mixengine-core --all-targets -- -D warnings
git add crates/mixengine-core/src/resolve.rs crates/mixengine-core/src/manifest.rs
git commit -m "refactor(core): resolve reads the manifest through core::manifest (T39)"
```

---

### Task 4: `core::projects` — the rows, and the walk that was in `resolve`

**Files:**
- Create: `crates/mixengine-core/src/projects.rs`
- Modify: `crates/mixengine-core/src/lib.rs` (`pub mod projects;`, two new `Error` variants)
- Modify: `crates/mixengine-daemon/src/error.rs` (map the two new variants to `already_exists`)
- Test: inline `#[cfg(test)] mod tests` in `crates/mixengine-core/src/projects.rs`

**Interfaces:**
- Consumes: `crate::{Error, Result, Store}`, `mixengine_platform::paths::in_full`, `mixengine_proto::{ProjectRef, RuntimeKind, Timestamp, VersionConstraint}`.
- Produces, and every later task uses these exact signatures:

```rust
pub struct ProjectRecord {
    pub id: i64,
    pub name: String,
    pub root: PathBuf,
    pub pins: BTreeMap<RuntimeKind, VersionConstraint>,
    pub created_at: String,
}

pub struct Registration {
    pub name: String,
    pub root: PathBuf,
    pub pins: BTreeMap<RuntimeKind, VersionConstraint>,
}

pub struct Change {
    pub name: Option<String>,
    pub root: Option<PathBuf>,
    pub pins: Option<BTreeMap<RuntimeKind, VersionConstraint>>,
}

pub fn validated_name(name: &str) -> Result<String>;
pub async fn create(store: &Store, registration: &Registration, at: Timestamp) -> Result<ProjectRecord>;
pub async fn records(store: &Store) -> Result<Vec<ProjectRecord>>;
pub async fn find(store: &Store, reference: &ProjectRef) -> Result<Option<ProjectRecord>>;
pub async fn update(store: &Store, id: i64, change: &Change) -> Result<ProjectRecord>;
pub async fn delete(store: &Store, id: i64) -> Result<()>;
```

- [ ] **Step 1: Write the failing tests**

Create `crates/mixengine-core/src/projects.rs` containing only:

```rust
#[cfg(test)]
mod tests {
    use super::*;

    const NOW: Timestamp = Timestamp(1_760_000_000_000);

    async fn store() -> (tempfile::TempDir, Store) {
        let home = tempfile::tempdir().expect("a temporary directory");
        let store = Store::open(&home.path().join(crate::paths::DATABASE_FILE_NAME))
            .await
            .expect("a database");
        (home, store)
    }

    /// A real directory to register, because `create` normalises what it is given.
    fn tree(depth: &[&str]) -> (tempfile::TempDir, PathBuf) {
        let root = tempfile::tempdir().expect("a temporary directory");
        let mut path = root.path().to_path_buf();
        for name in depth {
            path = path.join(name);
        }
        std::fs::create_dir_all(&path).expect("a directory");
        (root, path)
    }

    fn pins(entries: &[(RuntimeKind, &str)]) -> BTreeMap<RuntimeKind, VersionConstraint> {
        entries
            .iter()
            .map(|(kind, text)| {
                (
                    *kind,
                    VersionConstraint::parse((*text).to_owned()).expect("a constraint"),
                )
            })
            .collect()
    }

    fn registration(name: &str, root: &Path) -> Registration {
        Registration {
            name: name.to_owned(),
            root: root.to_path_buf(),
            pins: BTreeMap::new(),
        }
    }

    /// What was written is what comes back.
    #[tokio::test]
    async fn a_registered_project_is_listed_with_the_pins_it_was_given() {
        let (_home, store) = store().await;
        let (_root, blog) = tree(&["blog"]);

        let mut asked = registration("blog", &blog);
        asked.pins = pins(&[(RuntimeKind::Php, "^8.3")]);

        let written = create(&store, &asked, NOW).await.expect("a project");

        assert_eq!(written.name, "blog");
        assert_eq!(written.pins, asked.pins);
        assert_eq!(written.created_at, NOW.to_rfc3339());
        assert_eq!(records(&store).await.expect("a listing"), vec![written]);
    }

    /// **D3.** The walk `resolve::in_a_project` used to hold: the nearest registered root at or
    /// above the directory, and never a further one when a nearer one exists.
    #[tokio::test]
    async fn a_directory_finds_the_nearest_project_above_it() {
        let (_home, store) = store().await;
        let (root, deep) = tree(&["blog", "packages", "theme", "src"]);
        let inner = root.path().join("blog").join("packages").join("theme");

        create(&store, &registration("outer", root.path()), NOW)
            .await
            .expect("the outer project");
        create(&store, &registration("theme", &inner), NOW)
            .await
            .expect("the inner project");

        let found = find(&store, &ProjectRef::Path(deep.display().to_string()))
            .await
            .expect("a lookup")
            .expect("something above it");

        assert_eq!(found.name, "theme", "the nearer root wins");

        // And a root inside another project's root is allowed: nesting has a defined answer.
        let outer = find(
            &store,
            &ProjectRef::Path(root.path().join("blog").display().to_string()),
        )
        .await
        .expect("a lookup")
        .expect("the outer project");
        assert_eq!(outer.name, "outer");
    }

    /// **D5, the half that would otherwise be silently broken on Windows.** A row normalised on the
    /// way in and a directory that was not are two strings for one directory — so the query side is
    /// normalised too, and the test asks it the way a caller does.
    #[tokio::test]
    async fn a_directory_is_found_however_this_filesystem_spells_it() {
        let (_home, store) = store().await;
        let (root, blog) = tree(&["blog"]);

        create(&store, &registration("blog", &blog), NOW)
            .await
            .expect("a project");

        // The temporary root as the OS handed it over — which on Windows may be an 8.3 alias — and
        // the same directory spelled in full. Both are the project.
        for spelling in [root.path().join("blog"), in_full(&root.path().join("blog"))] {
            assert!(
                find(&store, &ProjectRef::Path(spelling.display().to_string()))
                    .await
                    .expect("a lookup")
                    .is_some(),
                "{} did not find the project",
                spelling.display()
            );
        }
    }

    /// One directory is one project, and the refusal names the project already holding it.
    #[tokio::test]
    async fn the_same_directory_twice_is_refused_by_the_name_that_has_it() {
        let (_home, store) = store().await;
        let (_root, blog) = tree(&["blog"]);

        create(&store, &registration("blog", &blog), NOW)
            .await
            .expect("the first");
        let error = create(&store, &registration("other", &blog), NOW)
            .await
            .expect_err("the second");

        assert!(
            matches!(&error, Error::ProjectRootTaken { holder, .. } if holder == "blog"),
            "{error:?}"
        );
    }

    /// The other unique column, whose repair is different: a name is not freed by moving anything.
    #[tokio::test]
    async fn the_same_name_twice_is_refused() {
        let (_home, store) = store().await;
        let (_first, one) = tree(&["blog"]);
        let (_second, two) = tree(&["shop"]);

        create(&store, &registration("blog", &one), NOW)
            .await
            .expect("the first");
        let error = create(&store, &registration("blog", &two), NOW)
            .await
            .expect_err("the second");

        assert!(matches!(error, Error::ProjectNameTaken { .. }), "{error:?}");
    }

    /// **D4.** A handle typed on a command line, and T39a will make a domain out of it.
    #[test]
    fn a_name_is_a_handle_rather_than_free_text() {
        assert_eq!(validated_name("  blog  ").expect("trimmed"), "blog");

        for refused in ["", "   ", "blog/site", "blog\\site", "a\u{7}b", &"x".repeat(65)] {
            assert!(
                validated_name(refused).is_err(),
                "{refused:?} should not be a project name"
            );
        }
    }

    /// **D6.** An absent map leaves the pins alone; an empty one clears them.
    #[tokio::test]
    async fn updating_pins_replaces_them_and_an_empty_map_clears_them() {
        let (_home, store) = store().await;
        let (_root, blog) = tree(&["blog"]);
        let mut asked = registration("blog", &blog);
        asked.pins = pins(&[(RuntimeKind::Php, "8.3"), (RuntimeKind::Node, "22")]);
        let written = create(&store, &asked, NOW).await.expect("a project");

        let renamed = update(
            &store,
            written.id,
            &Change {
                name: Some("weblog".to_owned()),
                root: None,
                pins: None,
            },
        )
        .await
        .expect("a rename");
        assert_eq!(renamed.name, "weblog");
        assert_eq!(renamed.pins, asked.pins, "an absent map changed nothing");

        let replaced = update(
            &store,
            written.id,
            &Change {
                name: None,
                root: None,
                pins: Some(pins(&[(RuntimeKind::Php, "^8.4")])),
            },
        )
        .await
        .expect("a replacement");
        assert_eq!(
            replaced.pins,
            pins(&[(RuntimeKind::Php, "^8.4")]),
            "replacing is not merging: node is gone"
        );

        let cleared = update(
            &store,
            written.id,
            &Change {
                name: None,
                root: None,
                pins: Some(BTreeMap::new()),
            },
        )
        .await
        .expect("a clearing");
        assert!(cleared.pins.is_empty());
    }

    /// A row written by a build that manages a fifth language must not stop this one reading the
    /// project — `resolve`'s own rule, kept where the reading moved to.
    #[tokio::test]
    async fn a_pin_naming_a_language_this_build_does_not_manage_is_ignored_rather_than_fatal() {
        let (_home, store) = store().await;
        let (_root, blog) = tree(&["blog"]);
        let root = in_full(&blog).display().to_string();

        sqlx::query(
            "INSERT INTO projects (name, root_path, runtime_pins_json, created_at)
             VALUES ('blog', ?, '{\"php\": \"8.3\", \"go\": \"1.22\"}', '2026-08-22T06:55:12Z')",
        )
        .bind(&root)
        .execute(store.pool())
        .await
        .expect("a project");

        let found = find(&store, &ProjectRef::Name("blog".to_owned()))
            .await
            .expect("a lookup")
            .expect("the project");

        assert_eq!(found.pins, pins(&[(RuntimeKind::Php, "8.3")]));
    }

    /// A reference matching nothing is nothing, not an error — the daemon is what turns it into
    /// `not_found`, because only it knows which of the two words a person typed.
    #[tokio::test]
    async fn a_reference_that_matches_nothing_answers_nothing() {
        let (_home, store) = store().await;

        assert!(
            find(&store, &ProjectRef::Name("blog".to_owned()))
                .await
                .expect("a lookup")
                .is_none()
        );
    }
}
```

- [ ] **Step 2: Run them to verify they fail**

Run: `cargo test -p mixengine-core --lib projects`
Expected: FAIL — module not declared, then unresolved `create`, `find`, `Error::ProjectRootTaken`.

- [ ] **Step 3: Write the module**

Above the tests in `crates/mixengine-core/src/projects.rs`:

```rust
//! The `projects` table: a directory this home has been told about, and the versions it pins.
//!
//! [`crate::packages`]' shape one table across — this module owns every write to `projects` and
//! nothing else — with one addition that is the point of the task: [`find`] is the walk
//! [`crate::resolve`] used to hold privately. Two implementations of "which project is this
//! directory in?" would be two answers to a question that has exactly one, which is the same rule
//! that put `resolve` in this crate to begin with.
//!
//! # One directory is one project, on both sides of the comparison
//!
//! `root_path` is `UNIQUE`, and it is normalised through
//! [`in_full`](mixengine_platform::paths::in_full) **before it is written** — that is what makes
//! `C:\Users\RUNNER~1\blog` and `C:\Users\runneradmin\blog` one project rather than two. The query
//! side is normalised as well, once, before the walk starts: a row normalised on the way in and a
//! caller's `cwd` that was not are two different strings for one directory, and step 3 of the
//! resolution order would miss on the very day it first had a row to hit.
//!
//! `in_full` expands 8.3 aliases and settles case. It does **not** follow symlinks or junctions, so
//! two paths reaching one directory through a junction can still register as two projects. That is
//! a known limit rather than an oversight: `std::fs::canonicalize` on Windows answers with a `\\?\`
//! verbatim path, a spelling nothing else in this workspace uses and which would leak into every
//! message and every rendered file.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use mixengine_platform::paths::in_full;
use mixengine_proto::{ProjectRef, RuntimeKind, Timestamp, VersionConstraint};

use crate::{Error, Result, Store};

/// The longest a project's name may be.
///
/// A handle typed on a command line, and T39a will take a site's default domain from it.
const NAME_LIMIT: usize = 64;

/// One row of `projects`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ProjectRecord {
    /// The rowid, which stays inside this crate: the wire handle is the name (spec D4).
    pub id: i64,

    /// What it is called.
    pub name: String,

    /// Its root, spelled the way the filesystem spells it.
    pub root: PathBuf,

    /// The versions it wants, by language, with anything this build cannot read left out.
    pub pins: BTreeMap<RuntimeKind, VersionConstraint>,

    /// When it was registered, as ISO-8601 text.
    pub created_at: String,
}

/// Everything registering a project has to write down.
#[derive(Debug, Clone)]
pub struct Registration {
    /// What to call it, already fallen through the manifest and the directory name.
    pub name: String,

    /// The root, as the caller spelled it. Normalised here.
    pub root: PathBuf,

    /// The pins, already fallen through the manifest.
    pub pins: BTreeMap<RuntimeKind, VersionConstraint>,
}

/// What an update is changing, where [`None`] means "leave it".
#[derive(Debug, Clone, Default)]
pub struct Change {
    /// A new name.
    pub name: Option<String>,

    /// A new root.
    pub root: Option<PathBuf>,

    /// The pins, **replacing** what the row held. An empty map clears them.
    pub pins: Option<BTreeMap<RuntimeKind, VersionConstraint>>,
}

/// A name, trimmed, or the reason it is not one.
///
/// # Errors
///
/// [`Error::InvalidProjectName`] for an empty name, one over sixty-four characters, one holding a
/// control character, and one holding `/` or `\` — a name that carries a path separator can be
/// neither a command-line handle nor the domain T39a will make of it.
pub fn validated_name(name: &str) -> Result<String> {
    let trimmed = name.trim();

    let refusal = if trimmed.is_empty() {
        Some("it is empty")
    } else if trimmed.chars().count() > NAME_LIMIT {
        Some("it is longer than sixty-four characters")
    } else if trimmed.chars().any(char::is_control) {
        Some("it holds a control character")
    } else if trimmed.contains('/') || trimmed.contains('\\') {
        Some("it holds a path separator")
    } else {
        None
    };

    match refusal {
        Some(because) => Err(Error::InvalidProjectName {
            name: name.to_owned(),
            because,
        }),
        None => Ok(trimmed.to_owned()),
    }
}

/// Register a directory as a project.
///
/// # Errors
///
/// [`Error::InvalidProjectName`] for a name that is not one; [`Error::ProjectNameTaken`] and
/// [`Error::ProjectRootTaken`] for the two unique columns, the second naming the project already
/// holding the directory; and [`Error::Database`] when the row cannot be written.
pub async fn create(
    store: &Store,
    registration: &Registration,
    at: Timestamp,
) -> Result<ProjectRecord> {
    let name = validated_name(&registration.name)?;
    let root = in_full(&registration.root);
    let root_column = root.display().to_string();
    let created_at = at.to_rfc3339();
    let pins = encode(&registration.pins);

    // Asked before the insert so the answer can name the project that is in the way, which a unique
    // index cannot — and asked again by the index underneath, which is what makes two clients
    // racing produce a refusal rather than two rows.
    if let Some(holder) = holder(store, &root_column).await? {
        return Err(Error::ProjectRootTaken {
            root: root_column,
            holder,
        });
    }

    let inserted = sqlx::query!(
        "INSERT INTO projects (name, root_path, runtime_pins_json, created_at)
         VALUES (?, ?, ?, ?)
         ON CONFLICT DO NOTHING",
        name,
        root_column,
        pins,
        created_at
    )
    .execute(store.pool())
    .await
    .map_err(|source| store.failure("write", source))?;

    if inserted.rows_affected() == 0 {
        return match holder(store, &root_column).await? {
            Some(holder) => Err(Error::ProjectRootTaken {
                root: root_column,
                holder,
            }),
            None => Err(Error::ProjectNameTaken { name }),
        };
    }

    tracing::info!(%name, root = %root_column, "a project was registered");

    Ok(ProjectRecord {
        id: inserted.last_insert_rowid(),
        name,
        root,
        pins: registration.pins.clone(),
        created_at,
    })
}

/// Every registered project, in name order.
///
/// The order a listing is scanned in, on [`crate::packages::records`]' reasoning: a table somebody
/// looks for a row in should be in the order the eye can predict.
///
/// # Errors
///
/// [`Error::UnreadableProjectRow`] for a row this build cannot read at all, and [`Error::Database`]
/// when the table cannot be read.
pub async fn records(store: &Store) -> Result<Vec<ProjectRecord>> {
    let rows = sqlx::query!(
        "SELECT id, name, root_path, runtime_pins_json, created_at FROM projects ORDER BY name"
    )
    .fetch_all(store.pool())
    .await
    .map_err(|source| store.failure("read", source))?;

    rows.into_iter()
        .map(|row| {
            Ok(ProjectRecord {
                id: row.id,
                pins: decode(&row.root_path, &row.runtime_pins_json)?,
                root: PathBuf::from(row.root_path),
                name: row.name,
                created_at: row.created_at,
            })
        })
        .collect()
}

/// The project a reference names, or [`None`].
///
/// A [`ProjectRef::Path`] is answered by **walking up**: the nearest registered root at or above
/// that directory, which is what a shell three directories deep inside a repository means. One
/// `in_full` call before the walk rather than one per ancestor — the answer is the same and the
/// walk is the hot half.
///
/// # Errors
///
/// The errors [`records`] gives.
pub async fn find(store: &Store, reference: &ProjectRef) -> Result<Option<ProjectRecord>> {
    let known = records(store).await?;

    match reference {
        ProjectRef::Name(name) => Ok(known.into_iter().find(|project| &project.name == name)),

        ProjectRef::Path(path) => {
            let directory = in_full(Path::new(path));

            Ok(directory.ancestors().find_map(|ancestor| {
                known
                    .iter()
                    .find(|project| project.root == ancestor)
                    .cloned()
            }))
        }
    }
}

/// Change a project's name, root or pins.
///
/// # Errors
///
/// [`Error::NotFound`] when the row is gone, the two taken-column errors [`create`] gives, and
/// [`Error::Database`] when the row cannot be written.
pub async fn update(store: &Store, id: i64, change: &Change) -> Result<ProjectRecord> {
    let mut project = records(store)
        .await?
        .into_iter()
        .find(|project| project.id == id)
        .ok_or_else(|| missing(&id.to_string()))?;

    if let Some(name) = &change.name {
        project.name = validated_name(name)?;
    }

    if let Some(root) = &change.root {
        project.root = in_full(root);
    }

    if let Some(pins) = &change.pins {
        project.pins = pins.clone();
    }

    let root_column = project.root.display().to_string();
    let pins = encode(&project.pins);

    // The same reading `create` does, and for the same reason: the answer has to be able to name
    // the project in the way, and this one must not accuse the row of being in its own way.
    if let Some(holder) = holder(store, &root_column).await?
        && holder != project.name
    {
        return Err(Error::ProjectRootTaken {
            root: root_column,
            holder,
        });
    }

    let written = sqlx::query!(
        "UPDATE projects SET name = ?, root_path = ?, runtime_pins_json = ?
         WHERE id = ?",
        project.name,
        root_column,
        pins,
        id
    )
    .execute(store.pool())
    .await;

    match written {
        Ok(_) => {}
        // The name is the other unique column, and SQLite's own message names an index rather than
        // the project. Classified rather than passed through, so a client meets one vocabulary.
        Err(sqlx::Error::Database(failure)) if failure.is_unique_violation() => {
            return Err(Error::ProjectNameTaken { name: project.name });
        }
        Err(source) => return Err(store.failure("write", source)),
    }

    tracing::info!(name = %project.name, root = %root_column, "a project was changed");

    Ok(project)
}

/// Forget a project. The directory is not touched.
///
/// # Errors
///
/// [`Error::NotFound`] when there is no such row, and [`Error::Database`] when it cannot be
/// written.
pub async fn delete(store: &Store, id: i64) -> Result<()> {
    let removed = sqlx::query!("DELETE FROM projects WHERE id = ?", id)
        .execute(store.pool())
        .await
        .map_err(|source| store.failure("write", source))?;

    if removed.rows_affected() == 0 {
        return Err(missing(&id.to_string()));
    }

    tracing::info!(project = id, "a project was forgotten");

    Ok(())
}

/// The project already holding this exact directory, if any.
async fn holder(store: &Store, root: &str) -> Result<Option<String>> {
    sqlx::query_scalar!("SELECT name FROM projects WHERE root_path = ?", root)
        .fetch_optional(store.pool())
        .await
        .map_err(|source| store.failure("read", source))
}

/// The pins column, as a map this build can use.
///
/// **Read as strings and parsed one value at a time**, which is `resolve`'s rule and its reason: a
/// row written by a build that manages a fifth language must not stop this one reading the project.
/// A key or a value this build cannot read is left out rather than fatal.
fn decode(root: &str, column: &str) -> Result<BTreeMap<RuntimeKind, VersionConstraint>> {
    let raw: BTreeMap<String, String> =
        serde_json::from_str(column).map_err(|_| Error::UnreadableProjectRow {
            root: root.to_owned(),
            column: "runtime_pins_json",
            value: column.to_owned(),
        })?;

    Ok(raw
        .into_iter()
        .filter_map(|(kind, constraint)| {
            Some((
                RuntimeKind::parse(&kind)?,
                VersionConstraint::parse(constraint).ok()?,
            ))
        })
        .collect())
}

/// The map, as the column holds it.
///
/// Serialising a map of strings cannot fail; written as a fallback rather than an `expect` because
/// nothing in this crate panics, and an empty object is what a project with no pins already means —
/// which is what `packages::remember` says beside the same call.
fn encode(pins: &BTreeMap<RuntimeKind, VersionConstraint>) -> String {
    let raw: BTreeMap<&str, &str> = pins
        .iter()
        .map(|(kind, constraint)| (kind.as_str(), constraint.as_str()))
        .collect();

    serde_json::to_string(&raw).unwrap_or_else(|_| "{}".to_owned())
}

/// The failure of looking one up, named the way the wire mapping expects.
fn missing(id: &str) -> Error {
    Error::NotFound {
        kind: "project",
        id: id.to_owned(),
    }
}
```

- [ ] **Step 4: Declare the module and add the three error variants**

In `crates/mixengine-core/src/lib.rs`, beside the other `pub mod` lines:

```rust
pub mod projects;
```

And in `pub enum Error`, beside `UnreadableProjectRow`:

```rust
    /// A project name that cannot be one.
    ///
    /// Refused rather than corrected, because a name is a handle: it is typed on a command line,
    /// shown in a listing, and T39a takes a site's default domain from it — so a name silently
    /// changed on the way in is a name that does not work where the user next types it.
    #[error("{name} cannot be a project name: {because}")]
    InvalidProjectName {
        /// What was offered.
        name: String,
        /// Which rule it broke, as a phrase finishing "cannot be a project name: …".
        because: &'static str,
    },

    /// A directory that is already a project.
    ///
    /// `projects.root_path` is `UNIQUE` — one directory is one project — and this names the project
    /// holding it, which the unique index cannot. A root *inside* another project's root is not
    /// this: the walk takes the nearest, so nesting has a defined answer.
    #[error("{root} is already the project {holder}")]
    ProjectRootTaken {
        /// The directory, spelled the way the filesystem spells it.
        root: String,
        /// The project that got there first.
        holder: String,
    },

    /// A project name that is already registered.
    ///
    /// The other unique column, and the one whose repair is different: a name is not freed by
    /// moving a directory, only by renaming or deleting the project that holds it.
    #[error("a project called {name} is already registered")]
    ProjectNameTaken {
        /// The name that is taken.
        name: String,
    },
```

In `crates/mixengine-daemon/src/error.rs`, beside the other `AlreadyExists` arms:

```rust
            // The fifth way of saying "it is already here", and its repair is a different argument
            // rather than a different call: the message already names the project in the way, so
            // the hint spends itself on what to do about it.
            Core::ProjectRootTaken { holder, .. } => {
                Error::new(ErrorCode::AlreadyExists, chain(self)).with_hint(format!(
                    "`mix project show {holder}` is the one that has it — one directory is one \
                     project"
                ))
            }

            Core::ProjectNameTaken { .. } => Error::new(ErrorCode::AlreadyExists, chain(self))
                .with_hint("`mix project list` shows the names that are taken"),

            // The user's own argument, and the message already says which rule it broke.
            Core::InvalidProjectName { .. } => Error::new(ErrorCode::InvalidArgument, chain(self))
                .with_hint(
                    "a project name is a handle: up to sixty-four characters, no path separators \
                     and no control characters",
                ),
```

- [ ] **Step 5: Prepare the offline queries and run the tests**

```bash
cargo sqlx prepare --workspace -- --all-targets --all-features
cargo test -p mixengine-core --lib projects
```

Expected: PASS — eight tests.

- [ ] **Step 6: Lint and commit**

```bash
cargo fmt --all
cargo clippy --workspace --all-targets -- -D warnings
git add crates/mixengine-core crates/mixengine-daemon/src/error.rs .sqlx
git commit -m "feat(core): the projects table, and the walk that was inside resolve (T39)"
```

---

### Task 5: `resolve` asks `projects::find`, and step 3 runs for the first time

**Files:**
- Modify: `crates/mixengine-core/src/resolve.rs` (`in_a_project` becomes a call into `projects::find`; rewrite the path-comparison comment)
- Test: a new test in `crates/mixengine-core/src/resolve.rs`'s existing test module

**Interfaces:**
- Consumes: `crate::projects::{create, find, Registration}` and `ProjectRecord.pins` from Task 4.
- Produces: nothing new — `resolve::runtime` keeps its signature. What changes is that `RuntimeSource::Project` can now be produced by a row this build wrote.

- [ ] **Step 1: Write the failing test**

In `crates/mixengine-core/src/resolve.rs`'s `mod tests`, after `each_source_takes_precedence_over_the_one_below_it`:

```rust
    /// **The test this whole task exists for.** Step 3 has been covered until now only by tests
    /// that wrote a `projects` row by hand; this is the first time the row comes from the code that
    /// registers a project, and the first time the step runs the way a user's machine will run it.
    #[tokio::test]
    async fn a_project_registered_through_the_module_that_registers_them_decides_the_version() {
        let (_home, store) = store().await;
        install(&store, RuntimeKind::Php, &["8.1.30", "8.3.33"]).await;
        let (_root, cwd) = tree(&["blog", "public", "assets"]);
        let root = cwd
            .parent()
            .and_then(Path::parent)
            .expect("the project root");

        crate::projects::create(
            &store,
            &crate::projects::Registration {
                name: "blog".to_owned(),
                root: root.to_path_buf(),
                pins: [(
                    RuntimeKind::Php,
                    VersionConstraint::parse("^8.3").expect("a constraint"),
                )]
                .into_iter()
                .collect(),
            },
            NOW,
        )
        .await
        .expect("a project");

        let resolved = runtime(
            &store,
            &Question {
                kind: RuntimeKind::Php,
                cwd: Some(&cwd),
                explicit: None,
            },
        )
        .await
        .expect("the project's pin");

        assert_eq!(resolved.runtime.version.as_str(), "8.3.33");
        assert!(
            matches!(&resolved.source, RuntimeSource::Project { root: named }
                if Path::new(named) == mixengine_platform::paths::in_full(root)),
            "{:?} should be the project two directories up",
            resolved.source
        );
        assert_eq!(
            resolved.constraint.as_ref().map(VersionConstraint::as_str),
            Some("^8.3")
        );
    }
```

- [ ] **Step 2: Run it to verify it fails**

Run: `cargo test -p mixengine-core --lib resolve::tests::a_project_registered`
Expected: FAIL — the resolution falls through to the default `8.1.30`, because `in_a_project` compares the row's normalised path against an un-normalised `cwd`. On Linux and macOS it may already pass; **that is the point of D5** and the Windows run is the one that proves it.

- [ ] **Step 3: Rewire `in_a_project`**

Replace the whole function and its doc comment in `crates/mixengine-core/src/resolve.rs`:

```rust
/// The pin held by the nearest registered project at or above the directory.
///
/// **The walk itself is [`crate::projects::find`]**, so a directory finds the same project here as
/// it does through `project.show` — two implementations of that would be two answers to a question
/// that has one.
///
/// **Paths are compared spelled in full, on both sides.** `projects.root_path` is normalised
/// through `paths::in_full` before it is written, which is what makes one directory one project;
/// the caller's directory is normalised once before the walk, which is what makes the row findable
/// again afterwards. Normalising only the way *in* would leave a row and a `cwd` that are two
/// strings for one directory on Windows, and this step would miss on the very day it first had a
/// row to hit.
async fn in_a_project(store: &Store, kind: RuntimeKind, cwd: &Path) -> Result<Option<Asked>> {
    let Some(project) = crate::projects::find(
        store,
        &mixengine_proto::ProjectRef::Path(cwd.display().to_string()),
    )
    .await?
    else {
        return Ok(None);
    };

    Ok(project.pins.get(&kind).map(|constraint| Asked {
        constraint: constraint.clone(),
        source: RuntimeSource::Project {
            root: project.root.display().to_string(),
        },
    }))
}
```

> **Note the behaviour this makes explicit:** the nearest registered project answers, and if *it* is
> silent about this language the walk stops there rather than continuing to an outer project. That
> is the behaviour the code has always had — the old loop returned on the first row it matched and
> `continue`d only past rows with no pin for the kind — so keep the existing
> `each_source_takes_precedence_over_the_one_below_it` test green as the proof.

Check the old test that inserted a row by hand: it binds `root` from `cwd.parent()` unnormalised. Update that one line to `let root = mixengine_platform::paths::in_full(cwd.parent().expect("a parent")).display().to_string();` so the fixture writes what `create` would have written, and keep the assertion comparing against the same string.

- [ ] **Step 4: Run the whole resolve suite**

Run: `cargo test -p mixengine-core --lib resolve`
Expected: PASS — including the new test and the untouched precedence test.

- [ ] **Step 5: Lint and commit**

```bash
cargo fmt --all
cargo clippy -p mixengine-core --all-targets -- -D warnings
git add crates/mixengine-core/src/resolve.rs
git commit -m "feat(core): step 3 of the resolution order runs for the first time (T39)"
```

---

### Task 6: Effective pins, and the refusal a removal earns

**Files:**
- Modify: `crates/mixengine-core/src/projects.rs` (add `effective_pins`, `BrokenPin`, `pins_broken_by`)
- Test: the same inline `mod tests`

**Interfaces:**
- Consumes: `crate::manifest::{at, read}` (Task 2), `crate::runtimes::records`, `crate::resolve::install_command`, `mixengine_proto::{PinSource, ProjectPin}` (Task 1).
- Produces:

```rust
pub async fn effective_pins(store: &Store, project: &ProjectRecord) -> Result<Vec<ProjectPin>>;

pub struct BrokenPin {
    pub project: String,
    pub kind: RuntimeKind,
    pub constraint: VersionConstraint,
}

pub async fn pins_broken_by(
    store: &Store,
    kind: RuntimeKind,
    version: &PackageVersion,
) -> Result<Vec<BrokenPin>>;
```

- [ ] **Step 1: Write the failing tests**

Append to `crates/mixengine-core/src/projects.rs`'s `mod tests`. It needs the same `install` helper `resolve`'s tests use, so add it here too:

```rust
    /// Write the rows an install would have written, without the eighty megabytes.
    async fn install(store: &Store, kind: RuntimeKind, versions: &[&str]) {
        for version in versions {
            crate::runtimes::remember(
                store,
                &crate::runtimes::Installation {
                    kind,
                    version: PackageVersion::parse(*version).expect("a version"),
                    channel: mixengine_proto::PackageChannel::Stable,
                    path: PathBuf::from("/home/runtimes").join(kind.as_str()).join(version),
                    bytes: 41_000_000,
                    url: format!("https://example.invalid/{kind}-{version}.tar.zst"),
                    sha256: "00".to_owned(),
                    provides: [(kind.as_str().to_owned(), format!("bin/{kind}"))]
                        .into_iter()
                        .collect(),
                    extension_dir: None,
                    extensions: crate::index::Extensions::default(),
                },
                NOW,
            )
            .await
            .expect("a row");
        }
    }

    /// **D1 and D6 together.** The manifest outranks the row, so a contradicting row pin is inert —
    /// and saying which one is in charge is the whole reason this answer carries a source.
    #[tokio::test]
    async fn a_manifest_pin_outranks_the_rows_and_says_so() {
        let (_home, store) = store().await;
        install(&store, RuntimeKind::Php, &["8.2.20", "8.3.33"]).await;
        let (_root, blog) = tree(&["blog"]);
        std::fs::write(crate::manifest::at(&blog), "[runtimes]\nphp = \"8.3\"\n")
            .expect("a manifest");

        let mut asked = registration("blog", &blog);
        asked.pins = pins(&[(RuntimeKind::Php, "8.2"), (RuntimeKind::Node, "22")]);
        let project = create(&store, &asked, NOW).await.expect("a project");

        let effective = effective_pins(&store, &project).await.expect("the pins");

        let php = effective
            .iter()
            .find(|pin| pin.kind == RuntimeKind::Php)
            .expect("php is pinned");
        assert_eq!(php.constraint.as_str(), "8.3", "the file wins");
        assert!(matches!(php.source, PinSource::Manifest { .. }), "{php:?}");
        assert_eq!(
            php.resolved.as_ref().map(PackageVersion::as_str),
            Some("8.3.33")
        );
        assert_eq!(php.hint, None, "a pin that resolves needs no advice");

        let node = effective
            .iter()
            .find(|pin| pin.kind == RuntimeKind::Node)
            .expect("node is pinned by the row");
        assert_eq!(node.source, PinSource::Registered);
        assert_eq!(node.resolved, None, "no node is installed");
        assert!(
            node.hint.as_deref().is_some_and(|hint| hint.contains("runtime install node")),
            "a pin nothing satisfies carries the command that would fix it: {node:?}"
        );
    }

    /// **The test the refusal is worth having.** Same pin, same command, two outcomes — and only a
    /// re-resolution against what would be left produces both.
    #[tokio::test]
    async fn a_removal_breaks_a_pin_only_when_it_takes_the_last_version_that_answers_it() {
        let (_home, store) = store().await;
        install(&store, RuntimeKind::Php, &["8.3.33", "8.3.34"]).await;
        let (_root, blog) = tree(&["blog"]);

        let mut asked = registration("blog", &blog);
        asked.pins = pins(&[(RuntimeKind::Php, "^8.3")]);
        create(&store, &asked, NOW).await.expect("a project");

        let first = pins_broken_by(
            &store,
            RuntimeKind::Php,
            &PackageVersion::parse("8.3.33").expect("a version"),
        )
        .await
        .expect("a reading");
        assert!(
            first.is_empty(),
            "^8.3 still has 8.3.34 to answer it: {first:?}"
        );

        crate::runtimes::forget(
            &store,
            RuntimeKind::Php,
            &PackageVersion::parse("8.3.33").expect("a version"),
        )
        .await
        .expect("the row");

        let second = pins_broken_by(
            &store,
            RuntimeKind::Php,
            &PackageVersion::parse("8.3.34").expect("a version"),
        )
        .await
        .expect("a reading");
        assert_eq!(second.len(), 1, "{second:?}");
        assert_eq!(second[0].project, "blog");
        assert_eq!(second[0].constraint.as_str(), "^8.3");
    }

    /// A pin that is already unsatisfiable must refuse nothing — otherwise one stale pin would make
    /// unremovable a runtime it never mentions.
    #[tokio::test]
    async fn a_pin_nothing_already_satisfies_breaks_over_nothing() {
        let (_home, store) = store().await;
        install(&store, RuntimeKind::Php, &["8.3.33"]).await;
        let (_root, blog) = tree(&["blog"]);

        let mut asked = registration("blog", &blog);
        asked.pins = pins(&[(RuntimeKind::Php, "8.1")]);
        create(&store, &asked, NOW).await.expect("a project");

        let broken = pins_broken_by(
            &store,
            RuntimeKind::Php,
            &PackageVersion::parse("8.3.33").expect("a version"),
        )
        .await
        .expect("a reading");

        assert!(broken.is_empty(), "{broken:?}");
    }

    /// **D7 reads the pin in effective order**, so a refusal is never based on a row the file
    /// overrides.
    #[tokio::test]
    async fn a_row_pin_the_manifest_overrides_refuses_nothing() {
        let (_home, store) = store().await;
        install(&store, RuntimeKind::Php, &["8.2.20", "8.3.33"]).await;
        let (_root, blog) = tree(&["blog"]);
        std::fs::write(crate::manifest::at(&blog), "[runtimes]\nphp = \"8.2\"\n")
            .expect("a manifest");

        let mut asked = registration("blog", &blog);
        asked.pins = pins(&[(RuntimeKind::Php, "8.3.33")]);
        create(&store, &asked, NOW).await.expect("a project");

        let broken = pins_broken_by(
            &store,
            RuntimeKind::Php,
            &PackageVersion::parse("8.3.33").expect("a version"),
        )
        .await
        .expect("a reading");

        assert!(
            broken.is_empty(),
            "the file pins 8.2, so removing 8.3.33 breaks nothing that would ever take effect: \
             {broken:?}"
        );
    }
```

Add the imports the tests need at the top of `mod tests`:

```rust
    use mixengine_proto::{PackageVersion, PinSource};
```

- [ ] **Step 2: Run them to verify they fail**

Run: `cargo test -p mixengine-core --lib projects`
Expected: FAIL — `cannot find function 'effective_pins'`, `cannot find function 'pins_broken_by'`.

- [ ] **Step 3: Write the two functions**

Append to `crates/mixengine-core/src/projects.rs`, before `mod tests`:

```rust
/// One project's pins in **effective** order, with what each resolves to today.
///
/// Effective means the manifest's entry where the root has one and the row's where it does not,
/// because that is what the shim will actually do (spec D1) — a panel showing anything else is a
/// panel that lies. Nothing here refuses an unsatisfiable pin: `project.create` on a colleague's
/// freshly cloned repository has to succeed on the machine that most needs telling what to install
/// (spec D6).
///
/// # Errors
///
/// [`Error::Manifest`] for a manifest at the root that does not parse, and [`Error::Database`] when
/// the installed set cannot be read.
pub async fn effective_pins(store: &Store, project: &ProjectRecord) -> Result<Vec<ProjectPin>> {
    let manifest = crate::manifest::read(&crate::manifest::at(&project.root))?;
    let manifest_path = crate::manifest::at(&project.root).display().to_string();

    let mut effective: BTreeMap<RuntimeKind, (VersionConstraint, PinSource)> = project
        .pins
        .iter()
        .map(|(kind, constraint)| (*kind, (constraint.clone(), PinSource::Registered)))
        .collect();

    if let Some(manifest) = manifest {
        for (kind, constraint) in manifest.runtimes {
            effective.insert(
                kind,
                (
                    constraint,
                    PinSource::Manifest {
                        path: manifest_path.clone(),
                    },
                ),
            );
        }
    }

    let mut pins = Vec::with_capacity(effective.len());

    for (kind, (constraint, source)) in effective {
        let resolved = newest_matching(store, kind, &constraint, None).await?;

        pins.push(ProjectPin {
            hint: resolved
                .is_none()
                .then(|| crate::resolve::install_command(kind, &constraint)),
            kind,
            constraint,
            source,
            resolved,
        });
    }

    Ok(pins)
}

/// A project whose pin a removal would leave with no answer.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BrokenPin {
    /// Which project, by the name a person would type at it.
    pub project: String,

    /// Which language.
    pub kind: RuntimeKind,

    /// What it asks for.
    pub constraint: VersionConstraint,
}

/// The projects whose pin **goes from having an answer to having none** if this version is removed.
///
/// The transition is the whole of it (spec D7). A pin that is already unsatisfiable stays
/// unsatisfiable and refuses nothing, or one stale pin would make unremovable a runtime it never
/// mentions; and a pin three installed versions answer is not broken by losing one of them.
///
/// Reading every project's manifest here is affordable *here*: an uninstall deletes hundreds of
/// megabytes and runs perhaps monthly. It is not affordable anywhere near the shim, which is why
/// [`crate::resolve`] reads one file per ancestor and stops at the first answer.
///
/// # Errors
///
/// The errors [`effective_pins`] gives, and [`Error::Database`] when `projects` cannot be read.
pub async fn pins_broken_by(
    store: &Store,
    kind: RuntimeKind,
    version: &PackageVersion,
) -> Result<Vec<BrokenPin>> {
    let mut broken = Vec::new();

    for project in records(store).await? {
        let Some(pin) = effective_pins(store, &project)
            .await?
            .into_iter()
            .find(|pin| pin.kind == kind)
        else {
            continue;
        };

        // Already unsatisfiable, so this removal is not what breaks it.
        if pin.resolved.is_none() {
            continue;
        }

        if newest_matching(store, kind, &pin.constraint, Some(version))
            .await?
            .is_none()
        {
            broken.push(BrokenPin {
                project: project.name,
                kind,
                constraint: pin.constraint,
            });
        }
    }

    Ok(broken)
}

/// The newest installed version of `kind` that answers `constraint`, ignoring `without`.
///
/// The resolver's own choice — [`PackageVersion::cmp_precedence`], the newest as upstream means
/// newest rather than as ASCII does — so a pin is judged here exactly as it will be judged when
/// somebody `cd`s into the directory.
async fn newest_matching(
    store: &Store,
    kind: RuntimeKind,
    constraint: &VersionConstraint,
    without: Option<&PackageVersion>,
) -> Result<Option<PackageVersion>> {
    Ok(crate::runtimes::records(store, Some(kind))
        .await?
        .into_iter()
        .map(|runtime| runtime.version)
        .filter(|version| without != Some(version))
        .filter(|version| constraint.matches(version))
        .max_by(PackageVersion::cmp_precedence))
}
```

Extend the module's imports:

```rust
use mixengine_proto::{
    PackageVersion, PinSource, ProjectPin, ProjectRef, RuntimeKind, Timestamp, VersionConstraint,
};
```

- [ ] **Step 4: Run the tests to verify they pass**

Run: `cargo test -p mixengine-core --lib projects`
Expected: PASS — twelve tests.

- [ ] **Step 5: Lint and commit**

```bash
cargo fmt --all
cargo clippy -p mixengine-core --all-targets -- -D warnings
git add crates/mixengine-core/src/projects.rs
git commit -m "feat(core): effective pins, and the ones a removal would break (T39)"
```

---

### Task 7: `project.*` in the daemon

**Files:**
- Create: `crates/mixengine-daemon/src/projects.rs`
- Modify: `crates/mixengine-daemon/src/main.rs` (`mod projects;`)
- Modify: `crates/mixengine-daemon/src/api/mod.rs` (a `projects` field on `Api`, built in `Api::new`)
- Modify: `crates/mixengine-daemon/src/api/rpc.rs` (six arms)
- Test: create `crates/mixengine-daemon/tests/projects.rs`

**Interfaces:**
- Consumes: everything from Tasks 1, 2, 4 and 6.
- Produces: `pub(crate) struct Projects` with `new(store: &Store) -> Arc<Self>`, and the methods `create`, `list`, `show`, `update`, `delete`, `export`, each taking its proto params type and answering its proto result type.

- [ ] **Step 1: Write the failing integration test**

Create `crates/mixengine-daemon/tests/projects.rs`. It needs no registry and no index — a project is rows and files — so the fixture is a plain daemon:

```rust
//! `project.*` against a real `mixengined` over a real socket.
//!
//! Roadmap task **T39**. What the unit tests next to `core::projects` prove is that the rows and the
//! walk are right; what is proved here is the part only a daemon can be wrong about — that a create
//! that names only a directory picks up the manifest lying in it, that a manifest pin is reported as
//! outranking a contradicting row, and that the same directory under two spellings is one project.
//!
//! No registry and no index: nothing here installs anything, so the daemon needs neither.

use std::path::Path;
use std::process::{Child, Command, Stdio};

use http_body_util::{BodyExt as _, Full};
use hyper::body::Bytes;
use hyper::header::{CONTENT_TYPE, HOST};
use hyper::{Method, Request, StatusCode};
use hyper_util::rt::TokioIo;
use mixengine_platform::ipc::Connection;
use mixengine_testkit::Home;
use serde_json::{Value, json};

/// A home with a daemon in it, killed when the test ends.
struct Fixture {
    home: Home,
    _daemon: Daemon,
}

impl Fixture {
    async fn start() -> Self {
        let home = Home::new();
        let daemon = Daemon::start(&home);
        home.wait_until_listening().await;

        Self {
            home,
            _daemon: daemon,
        }
    }

    async fn client(&self) -> Client {
        Client::connect(&self.home).await
    }
}

struct Daemon(Child);

impl Daemon {
    fn start(home: &Home) -> Self {
        Self(
            Command::new(env!("CARGO_BIN_EXE_mixengined"))
                .arg("--home")
                .arg(home.path())
                .stdout(Stdio::null())
                .stderr(Stdio::null())
                .spawn()
                .expect("the daemon binary runs"),
        )
    }
}

impl Drop for Daemon {
    fn drop(&mut self) {
        mixengine_testkit::try_kill(&mut self.0);
    }
}

/// One connection to the daemon. The same three helpers `tests/runtimes.rs` carries.
struct Client {
    sender: hyper::client::conn::http1::SendRequest<Full<Bytes>>,
}

impl Client {
    async fn connect(home: &Home) -> Self {
        let connection = Connection::connect(home.endpoint())
            .await
            .expect("the daemon is listening");

        let (sender, driver) = hyper::client::conn::http1::handshake(TokioIo::new(connection))
            .await
            .expect("the daemon speaks HTTP/1.1");

        tokio::spawn(driver);

        Self { sender }
    }

    async fn call(&mut self, method: &str, params: Value) -> Value {
        let answer = self.ask(method, params).await;
        assert!(answer.get("error").is_none(), "{method}: {answer}");
        answer["result"].clone()
    }

    async fn refuse(&mut self, method: &str, params: Value) -> Value {
        let answer = self.ask(method, params).await;
        assert!(answer.get("result").is_none(), "{method}: {answer}");
        answer["error"].clone()
    }

    async fn ask(&mut self, method: &str, params: Value) -> Value {
        let body = json!({ "jsonrpc": "2.0", "method": method, "params": params, "id": 1 });

        let request = Request::builder()
            .method(Method::POST)
            .uri("/rpc")
            .header(HOST, "mixengine")
            .header(CONTENT_TYPE, "application/json")
            .body(Full::new(Bytes::from(
                serde_json::to_vec(&body).expect("a request serialises"),
            )))
            .expect("a well formed request");

        self.sender
            .ready()
            .await
            .expect("the connection is still open");

        let response = self.sender.send_request(request).await.expect("an answer");
        assert_eq!(response.status(), StatusCode::OK, "{method}");

        let bytes = response
            .into_body()
            .collect()
            .await
            .expect("a whole body")
            .to_bytes();

        serde_json::from_slice(&bytes).expect("a JSON-RPC response")
    }
}

/// A directory to register, and its manifest when it has one.
fn repository(body: Option<&str>) -> tempfile::TempDir {
    let directory = tempfile::Builder::new()
        .prefix("mixengine-project")
        .tempdir()
        .expect("a temporary directory");

    if let Some(body) = body {
        std::fs::write(directory.path().join("mixengine.toml"), body).expect("a manifest");
    }

    directory
}

fn as_string(path: &Path) -> String {
    path.display().to_string()
}

/// The whole life of a project, in the order somebody lives it.
#[tokio::test]
async fn a_project_is_created_listed_shown_changed_and_forgotten() {
    let fixture = Fixture::start().await;
    let mut client = fixture.client().await;
    let repository = repository(None);

    let empty = client.call("project.list", Value::Null).await;
    assert_eq!(empty["projects"], json!([]));

    let created = client
        .call(
            "project.create",
            json!({"root": as_string(repository.path()), "name": "blog"}),
        )
        .await;
    assert_eq!(created["project"]["name"], "blog");
    assert_eq!(created["pins"], json!([]));
    assert!(
        created["project"]["manifest"].is_null(),
        "there is no manifest in that directory: {created}"
    );

    // Addressed by any directory inside it, which is what a shell has.
    let inside = repository.path().join("public");
    std::fs::create_dir(&inside).expect("a directory");
    let shown = client
        .call("project.show", json!({"project": {"path": as_string(&inside)}}))
        .await;
    assert_eq!(shown["project"]["name"], "blog");

    let changed = client
        .call(
            "project.update",
            json!({
                "project": {"name": "blog"},
                "name": "weblog",
                "pins": {"php": "^8.3"},
            }),
        )
        .await;
    assert_eq!(changed["project"]["name"], "weblog");
    assert_eq!(changed["pins"][0]["constraint"], "^8.3");
    assert_eq!(changed["pins"][0]["source"]["from"], "registered");
    assert!(
        changed["pins"][0]["hint"]
            .as_str()
            .is_some_and(|hint| hint.contains("runtime install php")),
        "nothing is installed, so the pin says what would satisfy it: {changed}"
    );

    let removed = client
        .call("project.delete", json!({"project": {"name": "weblog"}}))
        .await;
    assert_eq!(removed["removed"]["name"], "weblog");
    assert!(
        Path::new(removed["root_kept"].as_str().expect("a path")).is_dir(),
        "the directory is kept, and named: {removed}"
    );

    let gone = client
        .refuse("project.show", json!({"project": {"name": "weblog"}}))
        .await;
    assert_eq!(gone["data"]["code"], "not_found", "{gone}");
}

/// **The import.** A create that names only a directory takes the name and the pins out of the
/// manifest a colleague checked in — no flag, no second method.
#[tokio::test]
async fn a_create_that_names_only_a_directory_reads_the_manifest_in_it() {
    let fixture = Fixture::start().await;
    let mut client = fixture.client().await;
    let repository = repository(Some(
        "[project]\nname = \"shop\"\n\n[runtimes]\nphp = \"^8.3\"\n",
    ));

    let created = client
        .call(
            "project.create",
            json!({"root": as_string(repository.path())}),
        )
        .await;

    assert_eq!(created["project"]["name"], "shop");
    assert_eq!(created["pins"][0]["kind"], "php");
    assert_eq!(created["pins"][0]["constraint"], "^8.3");
    assert_eq!(
        created["pins"][0]["source"]["from"], "manifest",
        "the file it came from is what decides, so that is what is reported: {created}"
    );
    assert!(
        created["project"]["manifest"]
            .as_str()
            .is_some_and(|path| path.ends_with("mixengine.toml")),
        "{created}"
    );
}

/// **D1's whole reason.** A row pin the manifest contradicts is inert, and the answer says so
/// rather than leaving somebody reading one version while their shell runs another.
#[tokio::test]
async fn a_manifest_pin_is_reported_as_outranking_the_row_it_contradicts() {
    let fixture = Fixture::start().await;
    let mut client = fixture.client().await;
    let repository = repository(Some("[runtimes]\nphp = \"8.4\"\n"));

    let created = client
        .call(
            "project.create",
            json!({
                "root": as_string(repository.path()),
                "name": "blog",
                "pins": {"php": "8.2"},
            }),
        )
        .await;

    assert_eq!(created["pins"].as_array().expect("pins").len(), 1);
    assert_eq!(created["pins"][0]["constraint"], "8.4");
    assert_eq!(created["pins"][0]["source"]["from"], "manifest");
}

/// One directory is one project, however this filesystem spells it.
#[tokio::test]
async fn the_same_directory_under_two_spellings_is_one_project() {
    let fixture = Fixture::start().await;
    let mut client = fixture.client().await;
    let repository = repository(None);

    client
        .call(
            "project.create",
            json!({"root": as_string(repository.path()), "name": "blog"}),
        )
        .await;

    let refused = client
        .refuse(
            "project.create",
            json!({
                "root": as_string(&mixengine_platform::paths::in_full(repository.path())),
                "name": "other",
            }),
        )
        .await;

    assert_eq!(refused["data"]["code"], "already_exists", "{refused}");
    assert!(
        refused["message"].as_str().is_some_and(|said| said.contains("blog")),
        "the refusal names the project that has it: {refused}"
    );
}

/// A root that is not a directory this machine can find is the caller's own bug, and is refused
/// before anything is written.
#[tokio::test]
async fn a_root_that_is_relative_or_missing_is_refused() {
    let fixture = Fixture::start().await;
    let mut client = fixture.client().await;
    let missing = repository(None).path().join("not-created-yet");

    for root in ["blog/public".to_owned(), as_string(&missing)] {
        let refused = client
            .refuse("project.create", json!({"root": root, "name": "blog"}))
            .await;

        assert_eq!(refused["data"]["code"], "invalid_argument", "{root}: {refused}");
    }
}

/// **D10.** An export merges into the file rather than rewriting it, and says which it did.
#[tokio::test]
async fn an_export_writes_the_project_into_the_manifest_and_keeps_the_rest() {
    let fixture = Fixture::start().await;
    let mut client = fixture.client().await;
    let repository = repository(Some("# mine\n[site]\ndomain = \"blog.test\"\n"));

    client
        .call(
            "project.create",
            json!({
                "root": as_string(repository.path()),
                "name": "blog",
                "pins": {"php": "^8.3"},
            }),
        )
        .await;

    let exported = client
        .call("project.export", json!({"project": {"name": "blog"}}))
        .await;

    assert_eq!(exported["created"], false, "the file was already there");
    let written = std::fs::read_to_string(exported["path"].as_str().expect("a path"))
        .expect("the manifest");
    assert!(written.contains("# mine"), "{written}");
    assert!(written.contains("[site]"), "{written}");
    assert!(written.contains("name = \"blog\""), "{written}");
    assert!(written.contains("php = \"^8.3\""), "{written}");
}
```

- [ ] **Step 2: Run it to verify it fails**

Run: `cargo test -p mixengine-daemon --test projects`
Expected: FAIL — every call answers `not_found` with "this daemon has no method `project.create`".

- [ ] **Step 3: Write the daemon module**

Create `crates/mixengine-daemon/src/projects.rs`:

```rust
//! `project.*`: the directories this home has been told about, and the versions they pin.
//!
//! Roadmap task **T39**. [`crate::packages`]' shape one namespace across, minus everything a
//! download needs: a project is rows and one file in somebody else's repository, so there is no
//! index, no fetcher and no job here.
//!
//! # The checks come first, in order of how specific they are
//!
//! `api/create.rs`' order and its reasoning: a root that is not an absolute directory is the
//! caller's own bug, a name that cannot be a handle is the user's typo, a manifest that does not
//! parse is the user's file — and only once all three have passed is a row written. The two unique
//! columns are still decided by the write, because whether a directory is free is a question about
//! the table.
//!
//! # A create does not write into the user's repository
//!
//! Spec D1. `project.export` is the one method that touches `mixengine.toml`, and it exists because
//! the point of that file is that a colleague gets it. A daemon that wrote to a checked-out working
//! tree on every update would be a daemon producing diffs nobody asked for, in a directory it does
//! not own.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::SystemTime;

use mixengine_core::{Store, manifest, projects};
use mixengine_proto::{
    Error, ErrorCode, ProjectCreate, ProjectDetail, ProjectExport, ProjectList, ProjectQuery,
    ProjectRef, ProjectRemoval, ProjectSummary, ProjectUpdate, RuntimeKind, Timestamp,
    VersionConstraint,
};

use crate::error::ToWire as _;

/// Everything `project.*` needs, which is the rows and nothing else.
#[derive(Debug)]
pub(crate) struct Projects {
    /// Where a project is written down.
    store: Store,
}

impl Projects {
    pub(crate) fn new(store: &Store) -> Arc<Self> {
        Arc::new(Self {
            store: store.clone(),
        })
    }

    /// `project.create` — register a directory, taking what it was not told from the manifest.
    ///
    /// # Errors
    ///
    /// `invalid_argument` for a root that is not an absolute directory this machine can find, a
    /// name that cannot be a handle, a pin whose syntax is not a constraint, and a manifest that
    /// does not parse; `already_exists` for a name or a directory that is already registered.
    pub(crate) async fn create(&self, create: &ProjectCreate) -> Result<ProjectDetail, Error> {
        let root = directory(&create.root)?;
        let manifest = manifest::read(&manifest::at(&root)).map_err(|error| error.to_wire())?;

        // **The fall-through, and the whole of what an import is** (spec D2): the argument, then
        // the manifest, then the directory's own name.
        let name = match &create.name {
            Some(name) => name.clone(),
            None => manifest
                .as_ref()
                .and_then(|manifest| manifest.project.as_ref())
                .and_then(|project| project.name.clone())
                .or_else(|| {
                    root.file_name()
                        .map(|name| name.to_string_lossy().into_owned())
                })
                .ok_or_else(|| {
                    Error::new(
                        ErrorCode::InvalidArgument,
                        format!("{} has no name to take", root.display()),
                    )
                    .with_hint("`--name` says what to call it")
                })?,
        };

        let pins = match &create.pins {
            Some(pins) => pins.clone(),
            None => manifest
                .map(|manifest| manifest.runtimes)
                .unwrap_or_default(),
        };

        let written = projects::create(
            &self.store,
            &projects::Registration { name, root, pins },
            Timestamp::from_system_time(SystemTime::now()),
        )
        .await
        .map_err(|error| error.to_wire())?;

        self.detail(written).await
    }

    /// `project.list` — every registered project, in name order.
    ///
    /// # Errors
    ///
    /// The wire error of a table that could not be read.
    pub(crate) async fn list(&self) -> Result<ProjectList, Error> {
        let records = projects::records(&self.store)
            .await
            .map_err(|error| error.to_wire())?;

        Ok(ProjectList {
            projects: records.iter().map(summary).collect(),
        })
    }

    /// `project.show` — one of them, with its pins in effective order.
    ///
    /// # Errors
    ///
    /// `not_found` for a reference matching nothing, `invalid_argument` for a manifest at the root
    /// that does not parse.
    pub(crate) async fn show(&self, query: &ProjectQuery) -> Result<ProjectDetail, Error> {
        let found = self.expect(&query.project).await?;

        self.detail(found).await
    }

    /// `project.update` — change a name, a root or the pins.
    ///
    /// # Errors
    ///
    /// `not_found`, and everything a create is refused for.
    pub(crate) async fn update(&self, update: &ProjectUpdate) -> Result<ProjectDetail, Error> {
        let found = self.expect(&update.project).await?;

        let root = match &update.root {
            Some(root) => Some(directory(root)?),
            None => None,
        };

        let changed = projects::update(
            &self.store,
            found.id,
            &projects::Change {
                name: update.name.clone(),
                root,
                pins: update.pins.clone(),
            },
        )
        .await
        .map_err(|error| error.to_wire())?;

        self.detail(changed).await
    }

    /// `project.delete` — forget the row, keep the directory, and say so.
    ///
    /// # Errors
    ///
    /// `not_found` for a reference matching nothing.
    pub(crate) async fn delete(&self, query: &ProjectQuery) -> Result<ProjectRemoval, Error> {
        let found = self.expect(&query.project).await?;
        let removed = summary(&found);

        projects::delete(&self.store, found.id)
            .await
            .map_err(|error| error.to_wire())?;

        Ok(ProjectRemoval {
            root_kept: found.root.display().to_string(),
            manifest_kept: removed.manifest.clone(),
            removed,
        })
    }

    /// `project.export` — put the project into `<root>/mixengine.toml`, keeping everything else.
    ///
    /// # Errors
    ///
    /// `not_found`; `invalid_argument` for a manifest that does not parse or cannot be edited; and
    /// the wire error of a file that cannot be written.
    pub(crate) async fn export(&self, query: &ProjectQuery) -> Result<ProjectExport, Error> {
        let found = self.expect(&query.project).await?;

        let created = manifest::write(&found.root, &found.name, &found.pins)
            .map_err(|error| error.to_wire())?;

        Ok(ProjectExport {
            path: manifest::at(&found.root).display().to_string(),
            created,
        })
    }

    /// The project a reference names, or the refusal for one that names nothing.
    async fn expect(&self, reference: &ProjectRef) -> Result<projects::ProjectRecord, Error> {
        projects::find(&self.store, reference)
            .await
            .map_err(|error| error.to_wire())?
            .ok_or_else(|| {
                let (said, hint) = match reference {
                    ProjectRef::Name(name) => (
                        format!("no such project: {name}"),
                        "`mix project list` shows what does exist".to_owned(),
                    ),
                    ProjectRef::Path(path) => (
                        format!("no project is registered at or above {path}"),
                        format!("`mix project create {path}` registers it"),
                    ),
                };

                Error::new(ErrorCode::NotFound, said).with_hint(hint)
            })
    }

    /// One record, with the pins it actually resolves by.
    async fn detail(&self, project: projects::ProjectRecord) -> Result<ProjectDetail, Error> {
        let pins = projects::effective_pins(&self.store, &project)
            .await
            .map_err(|error| error.to_wire())?;

        Ok(ProjectDetail {
            project: summary(&project),
            pins,
        })
    }
}

/// One record, as the sentence a client renders.
fn summary(project: &projects::ProjectRecord) -> ProjectSummary {
    let manifest = manifest::at(&project.root);

    ProjectSummary {
        name: project.name.clone(),
        root: project.root.display().to_string(),
        created_at: project.created_at.clone(),
        // Named only when it is there, on `ServiceRemoval::data_kept`'s rule — and because whether
        // the file exists is what decides whether the row's pins can take effect at all.
        manifest: manifest
            .is_file()
            .then(|| manifest.display().to_string()),
    }
}

/// A root a project can have: absolute, present, and a directory.
///
/// Checked here rather than left to the row, because the alternative is a project registered
/// against a path that means nothing on this machine — and step 3 of the resolution order walks
/// upwards from a directory, so a relative one would be walked from wherever the *daemon* was
/// started.
fn directory(root: &str) -> Result<PathBuf, Error> {
    let path = Path::new(root);

    if !path.is_absolute() {
        return Err(Error::new(
            ErrorCode::InvalidArgument,
            format!("{root} is not an absolute directory"),
        )
        .with_hint(
            "a project is found by walking up from a directory, so its root has to be one this \
             machine can find on its own",
        ));
    }

    if !path.is_dir() {
        return Err(Error::new(
            ErrorCode::InvalidArgument,
            format!("{root} is not a directory"),
        )
        .with_hint("make it first — a project is a directory that is already there"));
    }

    Ok(path.to_path_buf())
}

/// Silence the unused-import warning for the two types only the signatures name.
///
/// Delete this line if the module grows a use for them.
#[expect(unused_imports, reason = "named by the signatures above")]
use {BTreeMap as _Pins, RuntimeKind as _Kind, VersionConstraint as _Constraint};
```

> Trim that last `use` block and the unused imports it names once the module compiles — it is a
> placeholder for whichever of `BTreeMap`, `RuntimeKind` and `VersionConstraint` the final code does
> not actually reference. Do not leave an `#[expect]` in the committed file.

- [ ] **Step 4: Wire it into `Api` and the dispatcher**

In `crates/mixengine-daemon/src/main.rs`, beside the other module declarations:

```rust
mod projects;
```

In `crates/mixengine-daemon/src/api/mod.rs`, add a field to `Api` beside `packages`:

```rust
    /// The registered projects, and the only thing that writes one down.
    ///
    /// Built here rather than passed in [`Supervision`], on `extensions`' reasoning: it holds
    /// nothing of its own that outlives a call — the store beside it is the whole of it — so a
    /// field in `main` would be one more thing to keep in step for no reading of it.
    projects: Arc<crate::projects::Projects>,
```

And in `Api::new`, beside the `extensions` line:

```rust
        let projects = crate::projects::Projects::new(store);
```

adding `projects,` to the struct literal.

In `crates/mixengine-daemon/src/api/rpc.rs`, after the `PACKAGE_UNINSTALL` arm:

```rust
                rpc::method::PROJECT_CREATE => {
                    let create: ProjectCreate = arguments(params)?;
                    encode_result(&api.projects.create(&create).await.map_err(refused)?)
                }

                rpc::method::PROJECT_LIST => {
                    no_params(params.as_ref())?;
                    encode_result(&api.projects.list().await.map_err(refused)?)
                }

                rpc::method::PROJECT_SHOW => {
                    let query: ProjectQuery = arguments(params)?;
                    encode_result(&api.projects.show(&query).await.map_err(refused)?)
                }

                rpc::method::PROJECT_UPDATE => {
                    let update: ProjectUpdate = arguments(params)?;
                    encode_result(&api.projects.update(&update).await.map_err(refused)?)
                }

                rpc::method::PROJECT_DELETE => {
                    let query: ProjectQuery = arguments(params)?;
                    encode_result(&api.projects.delete(&query).await.map_err(refused)?)
                }

                rpc::method::PROJECT_EXPORT => {
                    let query: ProjectQuery = arguments(params)?;
                    encode_result(&api.projects.export(&query).await.map_err(refused)?)
                }
```

Extend that file's `use mixengine_proto::{…}` list with `ProjectCreate, ProjectQuery, ProjectUpdate`.

- [ ] **Step 5: Run the integration test**

Run: `cargo test -p mixengine-daemon --test projects`
Expected: PASS — seven tests.

- [ ] **Step 6: Lint and commit**

```bash
cargo fmt --all
cargo clippy --workspace --all-targets -- -D warnings
git add crates/mixengine-daemon
git commit -m "feat(daemon): project.* — create, list, show, update, delete, export (T39)"
```

---

### Task 8: `runtime.uninstall` refuses over a pin, and `--force` crosses that and nothing else

**Files:**
- Modify: `crates/mixengine-daemon/src/runtimes.rs` (`uninstall` takes `RuntimeUninstall`, calls `pins_broken_by`, loses the doc sentence that named T39)
- Modify: `crates/mixengine-daemon/src/api/rpc.rs` (the `RUNTIME_UNINSTALL` arm decodes `RuntimeUninstall`)
- Test: `crates/mixengine-daemon/tests/runtimes.rs` (two new tests using the fixture that already installs a real PHP)

**Interfaces:**
- Consumes: `mixengine_proto::RuntimeUninstall` (Task 1), `mixengine_core::projects::{pins_broken_by, BrokenPin}` (Task 6).
- Produces: `Runtimes::uninstall(&self, asked: &RuntimeUninstall) -> Result<RuntimeRemoval, Error>` — signature change; the CLI in Task 9 depends on it.

- [ ] **Step 1: Write the failing tests**

Append to `crates/mixengine-daemon/tests/runtimes.rs`:

```rust
/// **The other half of the promise [runtime-versions.md] made.** T32 delivered the running-pool
/// refusal and left this one written down in a doc comment; this is the test that comment named.
#[tokio::test]
async fn a_runtime_a_project_pins_is_not_removed_by_accident() {
    let fixture = Fixture::start().await;
    let mut client = fixture.client().await;
    client.install(VERSION).await;

    let repository = tempfile::tempdir().expect("a temporary directory");
    client
        .call(
            "project.create",
            json!({
                "root": repository.path().display().to_string(),
                "name": "blog",
                "pins": {"php": VERSION},
            }),
        )
        .await;

    let refused = client
        .refuse(
            "runtime.uninstall",
            json!({"kind": "php", "version": VERSION}),
        )
        .await;

    assert_eq!(refused["data"]["code"], "precondition_failed", "{refused}");
    assert!(
        refused["message"].as_str().is_some_and(|said| said.contains("blog")),
        "the refusal names the project, because that is what a person has to go and change: \
         {refused}"
    );
    assert!(
        refused["data"]["hint"]
            .as_str()
            .is_some_and(|hint| hint.contains("--force")),
        "{refused}"
    );

    // Still installed: a refusal that removed the directory anyway would be the worst of both.
    let listed = client
        .call("runtime.list_installed", json!({"kind": "php"}))
        .await;
    assert_eq!(listed["runtimes"][0]["version"], VERSION, "{listed}");

    // And the flag is what a person types once they have been shown what they are breaking.
    let removed = client
        .call(
            "runtime.uninstall",
            json!({"kind": "php", "version": VERSION, "force": true}),
        )
        .await;
    assert_eq!(removed["removed"]["version"], VERSION, "{removed}");

    // The project is untouched: what breaks is the next resolution, which says what to install.
    let shown = client
        .call("project.show", json!({"project": {"name": "blog"}}))
        .await;
    assert!(shown["pins"][0]["resolved"].is_null(), "{shown}");
    assert!(
        shown["pins"][0]["hint"]
            .as_str()
            .is_some_and(|hint| hint.contains("runtime install php")),
        "{shown}"
    );
}

/// **D8's asymmetry, which the schema does not enforce for free.** A stopped pool is deleted with
/// the runtime; a *running* one refuses, and `--force` does not buy a live process with no files.
#[tokio::test]
async fn a_running_pool_refuses_an_uninstall_even_when_it_is_forced() {
    let fixture = Fixture::start().await;
    let mut client = fixture.client().await;
    client.install(VERSION).await;

    let pool = format!("php-fpm@{VERSION}");
    let _: Value = client
        .call("service.start", json!({"service": pool}))
        .await;

    let refused = client
        .refuse(
            "runtime.uninstall",
            json!({"kind": "php", "version": VERSION, "force": true}),
        )
        .await;

    assert_eq!(refused["data"]["code"], "precondition_failed", "{refused}");
    assert!(
        refused["message"].as_str().is_some_and(|said| said.contains(&pool)),
        "{refused}"
    );
}
```

> If `service.start` on the pool is not reachable from this fixture on every platform, assert the
> refusal against the pool state the fixture *can* reach and say so in a comment — do not delete the
> test. The property under it is that `force` is consulted **only** at the pin check, and that is
> also provable by reading the order in step 3; the test is what stops it drifting.

- [ ] **Step 2: Run them to verify they fail**

Run: `cargo test -p mixengine-daemon --test runtimes a_runtime_a_project_pins`
Expected: FAIL — the uninstall succeeds, because nothing checks a pin yet.

- [ ] **Step 3: Add the refusal**

In `crates/mixengine-daemon/src/runtimes.rs`, change the signature and add the check as the **first** thing the method does after the record is read — before the pool check, because a refusal that costs nothing should come before one that reads a second table:

```rust
    /// `runtime.uninstall` — remove the directory, then the row.
    ///
    /// **In that order**, which is [`mixengine_core::runtimes`]' rule read backwards: a directory
    /// that could not be removed leaves a row that still describes it, and asking again repeats
    /// exactly this. The reverse would leave a runtime on disk that nothing knows about.
    ///
    /// **Two refusals, and `force` crosses exactly one of them.** A project whose pin this removal
    /// would leave with no answer is a statement about the future — the next `cd` into that
    /// directory fails with a message naming the install that fixes it — so somebody who has been
    /// shown the projects and typed `--force` has made a decision they are entitled to make. A
    /// running php-fpm pool is a fact about the present, and no flag buys a live process with no
    /// files under it. That asymmetry is decided here rather than by the schema: a **stopped** pool
    /// is deleted along with the runtime, deliberately, so the `ON DELETE RESTRICT` on
    /// `services.runtime_install_id` is never reached.
    ///
    /// # Errors
    ///
    /// `not_found` when it is not installed; `precondition_failed` when a registered project pins
    /// it and `force` was not asked for, and when the pool that runs out of it has not been
    /// stopped; and the wire error of a directory that could not be removed — on Windows, most
    /// often a process still running out of it.
    pub(crate) async fn uninstall(
        &self,
        asked: &RuntimeUninstall,
    ) -> Result<RuntimeRemoval, Error> {
        let target = &asked.target;

        let removed = runtimes::record(&self.store, target.kind, &target.version)
            .await
            .map_err(|error| error.to_wire())?;

        if !asked.force {
            let broken = mixengine_core::projects::pins_broken_by(
                &self.store,
                target.kind,
                &target.version,
            )
            .await
            .map_err(|error| error.to_wire())?;

            if !broken.is_empty() {
                let named = broken
                    .iter()
                    .map(|pin| format!("{} ({})", pin.project, pin.constraint))
                    .collect::<Vec<_>>()
                    .join(", ");

                return Err(Error::new(
                    ErrorCode::PreconditionFailed,
                    format!(
                        "removing {} {} would leave nothing for {named}",
                        target.kind, target.version
                    ),
                )
                .with_hint(
                    "install another version that answers the pin, change the pin, or \
                     `--force` to remove it anyway",
                ));
            }
        }

        // … the existing pool check, discard, extension cleanup and `forget` follow unchanged …
```

Add `RuntimeUninstall` to that file's `use mixengine_proto::{…}` list.

In `crates/mixengine-daemon/src/api/rpc.rs`:

```rust
                rpc::method::RUNTIME_UNINSTALL => {
                    let asked: RuntimeUninstall = arguments(params)?;
                    encode_result(&api.runtimes.uninstall(&asked).await.map_err(refused)?)
                }
```

and add `RuntimeUninstall` to its imports.

- [ ] **Step 4: Run the runtimes suite**

Run: `cargo test -p mixengine-daemon --test runtimes`
Expected: PASS — the two new tests, and every existing one, which is what proves the wire shape did not change for a client that sends no `force`.

- [ ] **Step 5: Lint and commit**

```bash
cargo fmt --all
cargo clippy --workspace --all-targets -- -D warnings
git add crates/mixengine-daemon
git commit -m "feat(daemon): runtime.uninstall refuses over a project pin, and --force crosses it (T39)"
```

---

### Task 9: `mix project …`, and `--force`

**Files:**
- Modify: `crates/mixengine-cli/src/main.rs` (a `Project` command with six subcommands, `import` aliasing `create`, `--force` on `runtime uninstall`, a `project` dispatcher)
- Modify: `crates/mixengine-cli/src/render.rs` (`project_list`, `project_detail`, `project_removal`, `project_export`)
- Test: create `crates/mixengine-cli/tests/project.rs`

**Interfaces:**
- Consumes: every proto type from Task 1 and every method the daemon answers from Tasks 7 and 8.
- Produces: the command surface `mix project create|import|list|show|update|delete|export` and `mix runtime uninstall --force`.

- [ ] **Step 1: Write the failing end-to-end test**

Create `crates/mixengine-cli/tests/project.rs`:

```rust
//! `mix project` against a real daemon.
//!
//! Roadmap task **T39**'s client half. What the daemon's own `tests/projects.rs` proves is that the
//! methods do what they say; what is proved here is the part that is only true of `mix` — that the
//! arguments a person types reach the right method, that `create` and `import` are one subcommand
//! under two names, that a command typed inside a project finds it without being told, and that the
//! human rendering says the sentence a person needs.

mod harness;

use harness::{Home, json, stdout};

/// A directory to register, and its manifest when it has one.
fn repository(body: Option<&str>) -> tempfile::TempDir {
    let directory = tempfile::Builder::new()
        .prefix("mixengine-project")
        .tempdir()
        .expect("a temporary directory");

    if let Some(body) = body {
        std::fs::write(directory.path().join("mixengine.toml"), body).expect("a manifest");
    }

    directory
}

/// The sequence a person actually types, in the order they type it.
#[tokio::test(flavor = "multi_thread")]
async fn a_project_is_created_shown_from_inside_and_exported_from_the_command_line() {
    let home = Home::new();
    let _daemon = home.start_daemon();
    let repository = repository(None);
    let root = repository.path().display().to_string();

    let empty = stdout(&home.mix(&["project", "list"]));
    assert!(
        empty.contains("no projects"),
        "an empty home says so rather than printing a heading with nothing under it: {empty}"
    );

    let created = json(&home.mix(&["project", "create", &root, "--name", "blog", "--json"]));
    assert_eq!(created["project"]["name"], "blog", "{created}");

    let listed = stdout(&home.mix(&["project", "list"]));
    assert!(listed.contains("blog"), "{listed}");

    // **From inside**, with nothing named: which project this is is the daemon's answer, and `mix`
    // only says which directory it is in.
    let inside = repository.path().join("public");
    std::fs::create_dir(&inside).expect("a directory");
    let shown = json(&home.mix_in(&inside, &[], &["project", "show", "--json"]));
    assert_eq!(shown["project"]["name"], "blog", "{shown}");

    let exported = json(&home.mix_in(&inside, &[], &["project", "export", "--json"]));
    assert_eq!(exported["created"], true, "{exported}");
    let written = std::fs::read_to_string(repository.path().join("mixengine.toml"))
        .expect("the manifest");
    assert!(written.contains("name = \"blog\""), "{written}");

    let removed = stdout(&home.mix(&["project", "delete", "blog"]));
    assert!(
        removed.contains(&root),
        "the directory that was kept is named: {removed}"
    );
}

/// **D2.** `import` is a second name for `create`, so both reach the same state.
#[tokio::test(flavor = "multi_thread")]
async fn create_and_import_are_one_subcommand_under_two_names() {
    let home = Home::new();
    let _daemon = home.start_daemon();
    let repository = repository(Some(
        "[project]\nname = \"shop\"\n\n[runtimes]\nphp = \"^8.3\"\n",
    ));

    let imported = json(&home.mix(&[
        "project",
        "import",
        &repository.path().display().to_string(),
        "--json",
    ]));

    assert_eq!(imported["project"]["name"], "shop", "{imported}");
    assert_eq!(imported["pins"][0]["constraint"], "^8.3", "{imported}");
    assert_eq!(imported["pins"][0]["source"]["from"], "manifest");
}

/// A name that is not a handle is refused by the daemon, and `mix` prints its sentence and exits
/// non-zero — which is what a script branches on.
#[tokio::test(flavor = "multi_thread")]
async fn a_refused_create_exits_non_zero_and_says_why() {
    let home = Home::new();
    let _daemon = home.start_daemon();
    let repository = repository(None);

    let output = home.mix(&[
        "project",
        "create",
        &repository.path().display().to_string(),
        "--name",
        "blog/site",
    ]);

    assert!(!output.status.success(), "{output:?}");
    assert!(
        String::from_utf8_lossy(&output.stderr).contains("path separator"),
        "{output:?}"
    );
}
```

- [ ] **Step 2: Run it to verify it fails**

Run: `cargo test -p mixengine-cli --test project`
Expected: FAIL — `error: unrecognized subcommand 'project'`.

- [ ] **Step 3: Add the command surface**

In `crates/mixengine-cli/src/main.rs`, add to `enum Command`:

```rust
    /// Register the directories this home knows about, and what they pin.
    Project {
        #[command(subcommand)]
        command: ProjectCommand,
    },
```

and the subcommand enum, beside `PackageCommand`:

```rust
/// `mix project …` — one subcommand per `project.*` method, and nothing that is not one.
///
/// `import` is an **alias** on `create` rather than a seventh subcommand: both produce one row, and
/// what makes a create an import is the `mixengine.toml` already lying in the directory rather than
/// a different call. An alias is the same subcommand under a second name, so the rule above holds.
#[derive(Debug, Subcommand)]
enum ProjectCommand {
    /// Register a directory as a project.
    ///
    /// With no `--name` and no `--php`/`--node`/…, whatever the `mixengine.toml` in that directory
    /// says is used — which is what adopting a colleague's checkout is.
    #[command(alias = "import")]
    Create {
        /// The project's root. Defaults to the current directory.
        #[arg(value_name = "DIR")]
        root: Option<PathBuf>,

        /// What to call it. Defaults to the manifest's name, then to the directory's own.
        #[arg(long, value_name = "NAME")]
        name: Option<String>,

        /// Pin a language, as `php=^8.3`. May be given more than once.
        #[arg(long = "pin", value_name = "RUNTIME=VERSION", value_parser = pin)]
        pins: Vec<(RuntimeKind, VersionConstraint)>,
    },

    /// List the projects this home has been told about.
    List,

    /// Show one, with its pins in the order they take effect.
    Show {
        #[command(flatten)]
        project: WhichProject,
    },

    /// Change a project's name, root or pins.
    ///
    /// `--pin` **replaces** every pin rather than adding to one: `--clear-pins` with no `--pin`
    /// removes them all, and leaving both out changes nothing.
    Update {
        #[command(flatten)]
        project: WhichProject,

        /// A new name.
        #[arg(long, value_name = "NAME")]
        name: Option<String>,

        /// A new root, for a repository that moved.
        #[arg(long, value_name = "DIR")]
        root: Option<PathBuf>,

        /// Pin a language, as `php=^8.3`. Replaces every pin the project had.
        #[arg(long = "pin", value_name = "RUNTIME=VERSION", value_parser = pin)]
        pins: Vec<(RuntimeKind, VersionConstraint)>,

        /// Remove every pin.
        #[arg(long, conflicts_with = "pins")]
        clear_pins: bool,
    },

    /// Forget a project. The directory is left exactly as it is.
    Delete {
        #[command(flatten)]
        project: WhichProject,
    },

    /// Write the project into `<root>/mixengine.toml`, keeping everything else in the file.
    Export {
        #[command(flatten)]
        project: WhichProject,
    },
}

/// Which project a command is about, which is the same question four times.
///
/// **The default is the directory you are in**, not a name this client invents: with no argument
/// `mix` sends the working directory and the daemon walks up to the nearest registered root — the
/// same walk the shim does.
#[derive(Debug, clap::Args)]
struct WhichProject {
    /// The project's name. Defaults to whichever project the current directory is in.
    #[arg(value_name = "PROJECT")]
    name: Option<String>,
}

/// `php=^8.3` — one pin, as a person types it.
fn pin(value: &str) -> Result<(RuntimeKind, VersionConstraint), String> {
    let (kind, constraint) = value
        .split_once('=')
        .ok_or_else(|| format!("`{value}` is not `<runtime>=<version>`"))?;

    Ok((
        runtime_kind(kind)?,
        version_constraint(constraint)?,
    ))
}
```

Add the dispatcher beside `package`:

```rust
/// `mix project …`: one call, one rendering.
async fn project(
    command: ProjectCommand,
    endpoint: &Endpoint,
    autostart: Option<&Autostart>,
    json: bool,
) -> Result<ExitCode, Error> {
    let mut client = Client::connect(endpoint, autostart).await?;

    match command {
        ProjectCommand::Create { root, name, pins } => {
            let create = ProjectCreate {
                root: here(root)?.display().to_string(),
                name,
                pins: (!pins.is_empty()).then(|| pins.into_iter().collect()),
            };
            let detail: ProjectDetail =
                ask(&mut client, rpc::method::PROJECT_CREATE, encode(&create)).await?;
            emit(&rendered(json, &detail, || render::project_detail(&detail)))?;
        }

        ProjectCommand::List => {
            let list: ProjectList =
                ask(&mut client, rpc::method::PROJECT_LIST, None).await?;
            emit(&rendered(json, &list, || render::project_list(&list)))?;
        }

        ProjectCommand::Show { project } => {
            let query = ProjectQuery {
                project: which(project)?,
            };
            let detail: ProjectDetail =
                ask(&mut client, rpc::method::PROJECT_SHOW, encode(&query)).await?;
            emit(&rendered(json, &detail, || render::project_detail(&detail)))?;
        }

        ProjectCommand::Update {
            project,
            name,
            root,
            pins,
            clear_pins,
        } => {
            let update = ProjectUpdate {
                project: which(project)?,
                name,
                root: root.map(|root| root.display().to_string()),
                pins: match (clear_pins, pins.is_empty()) {
                    (true, _) => Some(std::collections::BTreeMap::new()),
                    (false, true) => None,
                    (false, false) => Some(pins.into_iter().collect()),
                },
            };
            let detail: ProjectDetail =
                ask(&mut client, rpc::method::PROJECT_UPDATE, encode(&update)).await?;
            emit(&rendered(json, &detail, || render::project_detail(&detail)))?;
        }

        ProjectCommand::Delete { project } => {
            let query = ProjectQuery {
                project: which(project)?,
            };
            let removal: ProjectRemoval =
                ask(&mut client, rpc::method::PROJECT_DELETE, encode(&query)).await?;
            emit(&rendered(json, &removal, || {
                render::project_removal(&removal)
            }))?;
        }

        ProjectCommand::Export { project } => {
            let query = ProjectQuery {
                project: which(project)?,
            };
            let exported: ProjectExport =
                ask(&mut client, rpc::method::PROJECT_EXPORT, encode(&query)).await?;
            emit(&rendered(json, &exported, || {
                render::project_export(&exported)
            }))?;
        }
    }

    Ok(ExitCode::SUCCESS)
}

/// A name if one was typed, and this directory if none was.
///
/// **Not a default this client invents**: the path is sent as it stands and the daemon does the
/// walking, which is the same answer the shim gets.
fn which(project: WhichProject) -> Result<ProjectRef, Error> {
    match project.name {
        Some(name) => Ok(ProjectRef::Name(name)),
        None => Ok(ProjectRef::Path(here(None)?.display().to_string())),
    }
}

/// A directory argument, or the one this process is in.
fn here(given: Option<PathBuf>) -> Result<PathBuf, Error> {
    match given {
        Some(path) if path.is_absolute() => Ok(path),
        Some(path) => std::env::current_dir()
            .map(|cwd| cwd.join(path))
            .map_err(|error| {
                Error::new(
                    ErrorCode::Io,
                    format!("this process has no working directory: {error}"),
                )
            }),
        None => std::env::current_dir().map_err(|error| {
            Error::new(
                ErrorCode::Io,
                format!("this process has no working directory: {error}"),
            )
        }),
    }
}
```

Add the arm to `run`:

```rust
        Command::Project { command } => {
            project(command, &endpoint, autostart.as_ref(), args.json).await
        }
```

Add `--force` to `RuntimeCommand::Uninstall`:

```rust
    /// Remove one installed version.
    ///
    /// Refused while a registered project pins it, naming the projects, and while the php-fpm pool
    /// that runs out of it is running. `--force` crosses the first and never the second.
    Uninstall {
        #[command(flatten)]
        runtime: Which,

        /// Remove it even though a registered project pins it.
        #[arg(long)]
        force: bool,
    },
```

and change the call site:

```rust
        RuntimeCommand::Uninstall { runtime, force } => {
            let asked = RuntimeUninstall {
                target: target(runtime),
                force,
            };
            let removal: RuntimeRemoval = ask(
                &mut client,
                rpc::method::RUNTIME_UNINSTALL,
                encode(&asked),
            )
            .await?;
            emit(&rendered(json, &removal, || {
                render::runtime_removal(&removal)
            }))?;
        }
```

Extend the `use mixengine_proto::{…}` list with `ProjectCreate, ProjectDetail, ProjectExport, ProjectList, ProjectQuery, ProjectRef, ProjectRemoval, ProjectUpdate, RuntimeUninstall`.

- [ ] **Step 4: Add the renderings**

In `crates/mixengine-cli/src/render.rs`, following `package_list`'s shape (a heading row, then columns, and a sentence when the list is empty):

```rust
/// `mix project list` — every registered project, and whether it has a manifest.
pub(crate) fn project_list(list: &ProjectList) -> String {
    if list.projects.is_empty() {
        return "no projects are registered — `mix project create <dir>` adds one\n".to_owned();
    }

    let mut out = format!("{:<24}  {:<9}  {}\n", "PROJECT", "MANIFEST", "ROOT");

    for project in &list.projects {
        out.push_str(&format!(
            "{:<24}  {:<9}  {}\n",
            project.name,
            if project.manifest.is_some() { "yes" } else { "—" },
            project.root
        ));
    }

    out
}

/// `mix project show` — one project, and what each pin actually resolves to.
///
/// The **source** column is the whole value of the rendering: a pin read from the manifest outranks
/// the row, so a person looking at a version they did not expect is looking for which of the two is
/// in charge.
pub(crate) fn project_detail(detail: &ProjectDetail) -> String {
    let mut out = format!(
        "{}\n  root      {}\n  created   {}\n",
        detail.project.name, detail.project.root, detail.project.created_at
    );

    if let Some(manifest) = &detail.project.manifest {
        out.push_str(&format!("  manifest  {manifest}\n"));
    }

    if detail.pins.is_empty() {
        out.push_str("\nno runtimes are pinned\n");
        return out;
    }

    out.push_str(&format!(
        "\n{:<8}  {:<10}  {:<10}  {}\n",
        "RUNTIME", "PINNED", "RESOLVES", "FROM"
    ));

    for pin in &detail.pins {
        let from = match &pin.source {
            PinSource::Registered => "this home".to_owned(),
            PinSource::Manifest { path } => path.clone(),
        };

        out.push_str(&format!(
            "{:<8}  {:<10}  {:<10}  {}\n",
            pin.kind.as_str(),
            pin.constraint.as_str(),
            pin.resolved
                .as_ref()
                .map_or("—", PackageVersion::as_str),
            from
        ));
    }

    for hint in detail.pins.iter().filter_map(|pin| pin.hint.as_ref()) {
        out.push_str(&format!("\n{hint}\n"));
    }

    out
}

/// `mix project delete` — and the directory it did not touch.
pub(crate) fn project_removal(removal: &ProjectRemoval) -> String {
    let mut out = format!("{} is no longer registered\n", removal.removed.name);
    out.push_str(&format!("  the directory is kept: {}\n", removal.root_kept));

    if let Some(manifest) = &removal.manifest_kept {
        out.push_str(&format!("  so is its manifest:   {manifest}\n"));
    }

    out
}

/// `mix project export` — which file, and whether it had to be made.
pub(crate) fn project_export(exported: &ProjectExport) -> String {
    match exported.created {
        true => format!("wrote {}\n", exported.path),
        false => format!(
            "updated {} — everything else in it is untouched\n",
            exported.path
        ),
    }
}
```

Extend that file's `use mixengine_proto::{…}` list with `PinSource, ProjectDetail, ProjectExport, ProjectList, ProjectRemoval`.

- [ ] **Step 5: Run the CLI suite**

Run: `cargo test -p mixengine-cli --test project`
Expected: PASS — three tests.

- [ ] **Step 6: Run the whole workspace**

Run: `cargo test --workspace`
Expected: PASS. If `a_second_instance_of_a_recipe…` fails, it is the known port race — rerun that test on its own before treating it as this task's.

- [ ] **Step 7: Lint and commit**

```bash
cargo fmt --all
cargo clippy --workspace --all-targets -- -D warnings
git add crates/mixengine-cli
git commit -m "feat(cli): mix project, and --force on mix runtime uninstall (T39)"
```

---

### Task 10: The documents that would otherwise disagree with the code

**Files:**
- Modify: `.claude/roadmap/phase-4-sites-and-elevation.md` (split T39 into T39 and T39a; tick T39)
- Modify: `.claude/roadmap/todo.md` (the phase-4 row's task range and count; strike the deferred-refusal paragraph)
- Modify: `.claude/features/runtime-versions.md` (the second refusal is delivered)
- Modify: `.claude/architecture/data-model.md` (only if this task made something in it untrue — check, do not assume)

**Interfaces:**
- Consumes: nothing. This is the change that stops three documents describing a build that no longer exists.
- Produces: nothing the code reads.

- [ ] **Step 1: Split the roadmap task**

In `.claude/roadmap/phase-4-sites-and-elevation.md`, replace the single T39 bullet with:

```markdown
- [x] **T39** Project model: `project.create|list|show|update|delete|export`, `mixengine.toml`
      read and write, and the `runtime.uninstall` refusal a project pin earns.
      Design: [T39 spec](../../docs/superpowers/specs/2026-08-22-t39-project-model-design.md).
      **`create` is also the import**: with no `--name` and no `--pin`, both come from the manifest
      lying at the root, so a second method would have been a second code path for one outcome.
- [ ] **T39a** Site model: `sites`, `site_domains`, `site_service_links`, the four site kinds
      (`php-fpm`, `static`, `reverse-proxy`, `node-app`), doc roots, and the `[site]` and
      `[[services]]` halves of `mixengine.toml`.
      T39 left those sections opaque: `core::manifest` reads the file whole and its writer preserves
      them byte for byte, so this task gives them types rather than teaching a second reader about
      them. T43 renders what this declares.
```

- [ ] **Step 2: Correct the index**

In `.claude/roadmap/todo.md`, in the phase table, change phase 4's range from `T39–T47` to `T39–T47` **with the count `1 / 14`** (the split adds one task, and T39 is done).

Replace the paragraph beginning "**One promise is deferred rather than scaffolded.**" with:

```markdown
**Both promises are kept.** `runtime.uninstall` refuses over a running php-fpm pool (**T32**) and
over a registered project whose pin the removal would leave with no answer (**T39**), and `--force`
crosses the second and never the first — a broken pin is a statement about the next `cd`, a running
pool is a process serving requests now.
```

- [ ] **Step 3: Correct the feature spec**

In `.claude/features/runtime-versions.md`, find where it describes `runtime.uninstall`'s two refusals as promised-but-unbuilt and rewrite that passage to describe what ships: the pin refusal names each project and its constraint, `--force` crosses it, and the pin is read in effective order so a row the manifest overrides refuses nothing.

Run first, so this edit is aimed rather than guessed:

```bash
grep -n "uninstall\|Phase 4\|project pin" .claude/features/runtime-versions.md
```

- [ ] **Step 4: Check the data model for anything this made untrue**

```bash
grep -n "projects\|mixengine.toml\|Resolution order" .claude/architecture/data-model.md
```

Expected: the `projects` DDL, the manifest example and the resolution-order sentence are all still exactly true — this task added no column and changed no order. **Change nothing that is still true.** If the grep shows otherwise, fix that line and nothing else.

- [ ] **Step 5: Read the whole diff against the spec**

```bash
git diff master --stat
git log master..HEAD --oneline
```

Walk the spec's decision list D1–D10 and point at the commit that carries each. Anything with no commit behind it is a gap; add it before finishing.

- [ ] **Step 6: Commit**

```bash
git add .claude docs/superpowers
git commit -m "docs(roadmap): split T39 into projects and T39a sites, and record what shipped"
```

---

## Self-review

**Spec coverage.** Every decision has a task behind it: D1 → Tasks 6, 7 (`effective_pins`, and the daemon writing no manifest on create); D2 → Tasks 7, 9 (fall-through, `import` alias); D3 → Task 4 (`find`, `ProjectRef::Path` walking up); D4 → Tasks 1, 4 (`name` on the wire, `validated_name`); D5 → Tasks 4, 5 (`in_full` on both sides, the rewritten comment); D6 → Tasks 1, 6 (`ProjectPin`, `PinSource`, replace-not-merge); D7 → Task 6 (`pins_broken_by`'s transition rule); D8 → Task 8 (`force`, and the running-pool test); D9 → Tasks 2, 3 (one reader, and the bench that decides); D10 → Task 2 (`toml_edit` merge). The API surface is Task 1, the error table is Tasks 4 and 7, the crate-changes list is Tasks 2–9, and the four testing headlines are Tasks 5, 6 and 8. The out-of-scope list is honoured: no `sites` table, no `blueprint_id`, no `--purge`, no `schema = N`.

**Placeholders.** None: every step carries the code or the exact command. The one line that looks like one — the trailing `#[expect(unused_imports)]` block in Task 7 — is explicitly marked for deletion in the step that writes it, and is there because the final import list depends on which helpers the module ends up calling.

**Type consistency.** `ProjectRecord`, `Registration`, `Change`, `BrokenPin`, `ProjectPin`, `PinSource`, `RuntimeUninstall` and the six method constants are named identically wherever they appear. `projects::find` takes `&ProjectRef` in Task 4 and is called with one in Tasks 5 and 7. `Runtimes::uninstall` changes signature in Task 8 and its only two callers — the RPC arm and the CLI — change in Tasks 8 and 9 respectively.

**Ordering risk worth naming.** Task 3 (resolve reads through `core::manifest`) and Task 5 (resolve asks `projects::find`) both edit `crates/mixengine-core/src/resolve.rs`. They are separate tasks because they are separate gates — Task 3's gate is the shim's performance budget, Task 5's is a behaviour test — but they must run in order, and a subagent given Task 5 must start from Task 3's committed state.

---

## Execution Handoff

**Plan complete and saved to `docs/superpowers/plans/2026-08-22-t39-project-model.md`. Two execution options:**

**1. Subagent-Driven (recommended)** - I dispatch a fresh subagent per task, review between tasks, fast iteration

**2. Inline Execution** - Execute tasks in this session using executing-plans, batch execution with checkpoints

**Which approach?**
