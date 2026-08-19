//! `service.create` and `service.delete`: the two ends of a `services` row's life.
//!
//! Roadmap task **T31a**. Their own file rather than more of [`super::rpc`], which is already the
//! longest in this crate, and together rather than beside their namespake neighbours because they
//! are one decision read forwards and backwards: what a create writes is exactly what a delete
//! takes away, and the second is the first's rollback when a rendering fails.
//!
//! # A create renders before it answers
//!
//! [`declared`](crate::services::declared) fails the **whole** declared set when one row cannot be
//! rendered — a graph is a property of a set, so there is no such thing as walking the good rows and
//! skipping the bad one. A row inserted and left behind would therefore take `service.list`,
//! `service.start` and every other `service.*` call down with it, for a service the caller was told
//! had failed. So the render happens inside the call, and a failure deletes the row it just wrote.
//!
//! # A delete keeps the data directory, and names it
//!
//! Generated configuration is disposable: `etc/<service-id>/` is rendered from the row and can be
//! rendered again, so it goes. A data directory is somebody's databases, there is no undo behind a
//! local development tool, and nothing about deleting a service says anything about wanting the data
//! gone. It is *named* in the answer rather than silently left, because a directory nobody was told
//! about is a directory nobody ever cleans up.

use std::path::{Path, PathBuf};

use mixengine_core::generate::Instancing;
use mixengine_core::services::Declaration;
use mixengine_proto::{
    Error, ErrorCode, ServiceCreate, ServiceId, ServiceRemoval, ServiceState, ServiceSummary,
};

use super::Api;
use crate::error::ToWire as _;

impl Api {
    /// `service.create` — write the row, render its configuration, and answer with the service.
    ///
    /// The checks are in this order because each is cheaper and more specific than the next: a
    /// package with no recipe is a typo, an id whose shape does not suit the recipe is a
    /// misunderstanding of the package, and a version that is not installed is a missing step. Only
    /// then is anything written.
    ///
    /// # Errors
    ///
    /// `invalid_argument` for a package this build cannot run and for an id whose shape does not
    /// suit its recipe; `precondition_failed` when that version is not installed, with the install
    /// command in the hint; `already_exists` for a service that is already declared; and whatever
    /// the rendering refused, after the row it wrote has been taken back out.
    pub(crate) async fn service_create(
        &self,
        create: &ServiceCreate,
    ) -> Result<ServiceSummary, Error> {
        let catalogue = crate::services::catalogue();
        let package = create.id.name();

        let Some(recipe) = catalogue.recipe(package) else {
            let known = catalogue.packages().collect::<Vec<_>>().join(", ");

            return Err(Error::new(
                ErrorCode::InvalidArgument,
                format!("this build of MixEngine cannot run {package}"),
            )
            .with_hint(format!(
                "it knows how to configure and run: {known} — the part of an id before `@` is the \
                 package it is an instance of"
            )));
        };

        // The recipe's answer and not a rule here: how many Caddys a home may have is a fact about
        // Caddy, and the id's shape is where a person meets it.
        let instance_name = match (recipe.instancing(), create.id.instance()) {
            (Instancing::Named, Some(instance)) => instance.to_owned(),
            (Instancing::Single, None) => package.to_owned(),

            (Instancing::Named, None) => {
                return Err(Error::new(
                    ErrorCode::InvalidArgument,
                    format!("{package} runs as named instances, so an id needs one"),
                )
                .with_hint(format!(
                    "`{package}@main` — the name after the `@` is yours, and it is what tells two \
                     of them apart"
                )));
            }

            (Instancing::Single, Some(_)) => {
                return Err(Error::new(
                    ErrorCode::InvalidArgument,
                    format!("there is one {package}, so its id carries no `@`"),
                )
                .with_hint(format!(
                    "`{package}` — a second instance would be two processes contending for the \
                     same port"
                )));
            }
        };

        match mixengine_core::packages::record(&self.store, package, &create.version).await {
            Ok(_) => {}
            Err(mixengine_core::Error::NotFound { .. }) => {
                return Err(Error::new(
                    ErrorCode::PreconditionFailed,
                    format!("{package} {} is not installed", create.version),
                )
                .with_hint(format!(
                    "`mix package install {package} {}` first",
                    create.version
                )));
            }
            Err(error) => return Err(error.to_wire()),
        }

        // Serialising a map of JSON values cannot fail — the failure modes of `to_string` are a type
        // that refuses to serialise and a map with non-string keys, and this is neither. Written as
        // a fallback rather than an `expect` because nothing in this crate panics, and an empty
        // object is what a service that overrides nothing already means.
        let overrides = create
            .overrides
            .as_ref()
            .and_then(|overrides| serde_json::to_string(overrides).ok())
            .unwrap_or_else(|| "{}".to_owned());

        mixengine_core::services::create(
            &self.store,
            &Declaration {
                service: create.id.clone(),
                package: package.to_owned(),
                version: create.version.clone(),
                instance_name,
                port: create.port,
                bind_addr: create.bind_addr.clone(),
                data_dir: create.data_dir.clone(),
                autostart: create.autostart.unwrap_or(false),
                overrides,
            },
        )
        .await
        .map_err(|error| error.to_wire())?;

        // The render, and the rollback if it refuses. Asking the registry for the graph is what
        // renders every declared service, this one included — so a template that will not render or
        // an override the recipe does not know is refused *here*, with the row already gone.
        match self.services.graph().await {
            Ok(graph) => Ok(super::rpc::summary(
                &graph,
                &create.id,
                mixengine_core::services::record(&self.store, &create.id)
                    .await
                    .ok()
                    .as_ref(),
                &self.services.supervised(),
            )),

            Err(error) => {
                self.roll_back(&create.id).await;

                Err(error.to_wire())
            }
        }
    }

    /// `service.delete` — take the row and its generated configuration, and leave the data.
    ///
    /// # Errors
    ///
    /// `not_found` when there is no such service; `precondition_failed` when it is running or being
    /// supervised — a row deleted out from under a live process would leave the process with nothing
    /// describing it; and the wire error of a directory that could not be removed.
    pub(crate) async fn service_delete(&self, id: &ServiceId) -> Result<ServiceRemoval, Error> {
        let graph = self
            .services
            .graph()
            .await
            .map_err(|error| error.to_wire())?;
        let supervised = self.services.supervised();

        let record = mixengine_core::services::record(&self.store, id)
            .await
            .map_err(|error| error.to_wire())?;

        let live = matches!(
            record.state,
            ServiceState::Running
                | ServiceState::Starting
                | ServiceState::Restarting
                | ServiceState::Degraded
                | ServiceState::Stopping
        );

        if live || supervised.contains(id) {
            return Err(Error::new(
                ErrorCode::PreconditionFailed,
                format!("{id} is {}", record.state.as_str()),
            )
            .with_hint(format!("`mix service stop {id}` first")));
        }

        // Read before anything is removed, because afterwards there is nothing left to describe.
        let removed = super::rpc::summary(&graph, id, Some(&record), &supervised);

        let column = mixengine_core::services::delete(&self.store, id)
            .await
            .map_err(|error| error.to_wire())?;

        let data = column.map_or_else(|| self.data_directory(id), PathBuf::from);

        discard(&self.paths.etc().join(id.as_str()))
            .await
            .map_err(|error| error.to_wire())?;

        Ok(ServiceRemoval {
            removed,
            // Named only when it is there: a service that never started has no data directory, and
            // telling somebody to look after a path that does not exist is noise.
            data_kept: data.is_dir().then(|| data.display().to_string()),
        })
    }

    /// Where a row that named no `data_dir` would have had its data placed.
    ///
    /// The generator's own fallback, spelled again here because the row it belongs to is gone by the
    /// time this is asked — and because what a delete is reporting is a directory on disk rather
    /// than a value in a table.
    fn data_directory(&self, id: &ServiceId) -> PathBuf {
        let package = id.name();
        let base = self.paths.data().join(package);

        match crate::services::catalogue()
            .recipe(package)
            .map(|recipe| recipe.instancing())
        {
            Some(Instancing::Single) | None => base,
            Some(Instancing::Named) => base.join(id.instance().unwrap_or(package)),
        }
    }

    /// Undo a create whose configuration would not render.
    ///
    /// Best effort by construction: the caller is already returning the failure that explains why
    /// nothing was created, and a rollback that failed too would only replace a useful message with
    /// a confusing one. What it cannot leave behind is the row — that is the one thing that would
    /// break every later call — so a failure to remove it is logged loudly.
    async fn roll_back(&self, id: &ServiceId) {
        if let Err(error) = mixengine_core::services::delete(&self.store, id).await {
            tracing::error!(
                service = id.as_str(),
                %error,
                "a service whose configuration could not be rendered could not be removed either; \
                 every later service call will fail until its row is deleted by hand"
            );
        }

        if let Err(error) = discard(&self.paths.etc().join(id.as_str())).await {
            tracing::warn!(
                service = id.as_str(),
                %error,
                "a partly rendered configuration directory could not be removed"
            );
        }
    }
}

/// Remove a directory, treating one that is not there as the outcome that was wanted.
async fn discard(path: &Path) -> mixengine_core::Result<()> {
    mixengine_core::runtimes::discard(path).await
}
