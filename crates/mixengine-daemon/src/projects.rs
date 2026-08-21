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

use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::SystemTime;

use mixengine_core::{Store, manifest, projects};
use mixengine_proto::{
    Error, ErrorCode, ProjectCreate, ProjectDetail, ProjectExport, ProjectList, ProjectQuery,
    ProjectRef, ProjectRemoval, ProjectSummary, ProjectUpdate, Timestamp,
};

use crate::error::ToWire as _;

/// Everything `project.*` needs, which is the rows and nothing else.
#[derive(Debug)]
pub(crate) struct Projects {
    /// Where a project is written down.
    store: Store,
}

impl Projects {
    /// The one of these the API holds.
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
    /// name that cannot be a handle, and a manifest that does not parse; `already_exists` for a
    /// name or a directory that is already registered.
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
        manifest: manifest.is_file().then(|| manifest.display().to_string()),
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
