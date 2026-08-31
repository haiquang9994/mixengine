//! `blueprint.*`: what a project is made of, written down and read back.
//!
//! Roadmap task **T77**. [`crate::projects`]' shape one namespace across, and like it there is no
//! index, no fetcher and no job here — a capture is rows and one rendered file inside this home.
//!
//! # Applying is refused, by name
//!
//! `blueprint.apply { dry_run: false }` answers `Unsupported` naming **T78**, which is the task that
//! executes a plan. The refusal lives here rather than in `mix` for the rule in `CLAUDE.md`: a
//! client renders what the daemon answers and holds no rule of its own, so a `--dry-run` the CLI
//! silently insisted on would be the CLI deciding what the product can do.

use std::path::{Path, PathBuf};
use std::sync::Arc;

use mixengine_core::blueprints::{capture, plan, store as filed};
use mixengine_core::{Paths, Store, projects};
use mixengine_proto::{
    BlueprintApply, BlueprintCapture, BlueprintList, BlueprintPlan, BlueprintSource,
    BlueprintSummary, Error, ErrorCode, ProjectRef, Timestamp,
};

use crate::error::ToWire as _;

/// Everything `blueprint.*` needs: the rows, and the directory a rendering goes in.
#[derive(Debug)]
pub(crate) struct Blueprints {
    /// Where a blueprint is written down.
    store: Store,

    /// Where its rendering goes.
    paths: Paths,

    /// This build's version, for a captured manifest's provenance.
    version: &'static str,
}

impl Blueprints {
    /// The one of these the API holds.
    pub(crate) fn new(store: &Store, paths: &Paths, version: &'static str) -> Arc<Self> {
        Arc::new(Self {
            store: store.clone(),
            paths: paths.clone(),
            version,
        })
    }

    /// `blueprint.capture` — write down what a project is made of.
    ///
    /// # Errors
    ///
    /// `not_found` for a project nothing is registered as; `invalid_argument` for a name that
    /// cannot be a slug; `already_exists` for a name already taken without `overwrite`; `conflict`
    /// for a project holding more than one site.
    pub(crate) async fn capture(
        &self,
        asked: &BlueprintCapture,
    ) -> Result<BlueprintSummary, Error> {
        let project = self.expect(&asked.project).await?;

        // Refused before anything is read, because the name is what the rendering is filed under and
        // a capture that did the work and then failed on the name would be work thrown away.
        let slug = filed::validated_slug(&asked.name).map_err(|error| error.to_wire())?;

        let manifest = capture::capture(
            &self.store,
            &capture::Asked {
                project: &project,
                name: &slug,
                description: asked.description.as_deref().unwrap_or_default(),
                // A compile-time constant rather than a call into the operating system, which is
                // what `crate::diagnostics` already reads for the same field.
                os: std::env::consts::OS,
                version: self.version,
                created_at: &now(),
            },
        )
        .await
        .map_err(|error| error.to_wire())?;

        filed::save(
            &self.store,
            &self.paths,
            &manifest,
            &slug,
            BlueprintSource::Captured,
            asked.overwrite,
        )
        .await
        .map_err(|error| {
            error.to_wire().with_hint(
                "`mix blueprint capture --overwrite` replaces the one already filed under that name",
            )
        })
    }

    /// `blueprint.list` — every blueprint this home holds.
    ///
    /// # Errors
    ///
    /// `internal` when a row holds a source this build does not know.
    pub(crate) async fn list(&self) -> Result<BlueprintList, Error> {
        let blueprints = filed::records(&self.store, &self.paths)
            .await
            .map_err(|error| error.to_wire())?;

        Ok(BlueprintList { blueprints })
    }

    /// `blueprint.apply` — what applying one would do.
    ///
    /// # Errors
    ///
    /// `unsupported` for `dry_run: false`, which is T78's; `not_found` for a blueprint nothing is
    /// filed under; `invalid_argument` for a root that is not absolute.
    pub(crate) async fn apply(&self, asked: &BlueprintApply) -> Result<BlueprintPlan, Error> {
        if !asked.dry_run {
            // **`PreconditionFailed`, and deliberately not `UnsupportedPlatform`.** That code means
            // *this operating system genuinely cannot do it*, and saying so here would be a lie
            // about the machine: every OS this product ships to will execute a plan the moment T78
            // lands. What is missing is the build, which is a state the user can get out of.
            return Err(Error::new(
                ErrorCode::PreconditionFailed,
                "this build plans an apply but does not carry one out",
            )
            .with_hint("`--dry-run` prints the plan; executing it arrives with roadmap task T78"));
        }

        let root = absolute(&asked.root)?;
        let manifest = filed::manifest_of(&self.store, &asked.blueprint)
            .await
            .map_err(|error| {
                error
                    .to_wire()
                    .with_hint("`mix blueprint list` shows what does exist")
            })?;

        plan::plan(
            &self.store,
            &asked.blueprint,
            &manifest,
            &asked.project,
            &root,
        )
        .await
        .map_err(|error| error.to_wire())
    }

    /// The project a reference names, or the refusal that says which kind of miss it was.
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
}

/// The moment a capture happened, as this schema writes one down.
///
/// `to_rfc3339` for the reason `0001_initial.sql` gives: a moment a person reads is text, and this
/// one is read back by nobody.
fn now() -> String {
    Timestamp::from_system_time(std::time::SystemTime::now()).to_rfc3339()
}

/// A root the daemon can act on: absolute, because this daemon has no idea what the client's
/// current directory is and a relative path here would be resolved against the wrong one.
fn absolute(given: &str) -> Result<PathBuf, Error> {
    let path = Path::new(given);

    match path.is_absolute() {
        true => Ok(path.to_path_buf()),
        false => Err(Error::new(
            ErrorCode::InvalidArgument,
            format!("{given} is not an absolute path"),
        )
        .with_hint("the client resolves a root against its own directory before sending it")),
    }
}
