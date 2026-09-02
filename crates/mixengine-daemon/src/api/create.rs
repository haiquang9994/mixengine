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

use mixengine_core::generate::{Instancing, Role};
use mixengine_core::services::{Declaration, Port};
use mixengine_proto::{
    Error, ErrorCode, ServiceCreate, ServiceCreation, ServiceId, ServiceRemoval, ServiceState,
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
    /// command in the hint, and when this home already has a front end and is being asked for a
    /// second (T37); `already_exists` for a service that is already declared **and for a
    /// `data_dir` another service already holds** (T36) — the second is decided by the write rather
    /// than here, because whether a directory is free is a question about the table; and whatever
    /// the rendering refused, after the row it wrote has been taken back out.
    pub(crate) async fn service_create(
        &self,
        create: &ServiceCreate,
    ) -> Result<ServiceCreation, Error> {
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

        // A pool is created by the install that puts the PHP on disk, not by hand: a `services` row
        // pointing at a `runtime_installs` row that this call has no way to name would be a row with
        // no parent, and the `CHECK` on the table refuses it. What a person gets instead is the
        // command that does work.
        if let mixengine_core::generate::Source::Runtime(kind) = recipe.source() {
            let kind = kind.as_str();

            return Err(Error::new(
                ErrorCode::InvalidArgument,
                format!("{package} is created by installing a {kind}, not by hand"),
            )
            .with_hint(format!(
                "`mix runtime install {kind} <version>` gives that version its own {package}, and \
                 `mix runtime uninstall` takes it away again"
            )));
        }

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

        // **One front end, whichever two programs are asked to be it** — roadmap task **T37**. The
        // recipe's answer again, and a different question from the one above: `Instancing` is about
        // how many rows may name `nginx`, and both front ends answer `Single`, so a home obeying
        // both recipes still ends up with a Caddy and an nginx generated against the same 80 and
        // 443. Refused before the package check, because installing the second one would not help.
        if matches!(recipe.role(), Role::FrontEnd(_))
            && let Some(holder) =
                mixengine_core::services::front_end::held_by(&self.store, &catalogue)
                    .await
                    .map_err(|error| error.to_wire())?
            && holder != create.id.as_str()
        {
            return Err(Error::new(
                ErrorCode::PreconditionFailed,
                format!("{holder} is this home's front end, and there is only one"),
            )
            .with_hint(format!(
                "every site is reached through one program: `mix service delete {holder}` before \
                 creating {package}, or keep the one that is there"
            )));
        }

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

        // **The recipe's wish, and only when the caller expressed none.** Which port a service would
        // like is a fact about the service (`Recipe::preferred_port`); a caller that named one has
        // already decided and is taken at its word, port and all — see [`Port`].
        let port = match (create.port, recipe.preferred_port()) {
            (Some(port), _) => Port::Fixed(port),
            (None, Some(preferred)) => Port::Allocate { preferred },
            (None, None) => Port::None,
        };

        let written = mixengine_core::services::create(
            &self.store,
            self.services.host(),
            &Declaration {
                service: create.id.clone(),
                origin: mixengine_core::services::Origin::Package {
                    name: package.to_owned(),
                    version: create.version.clone(),
                },
                instance_name,
                port,
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
            Ok(graph) => Ok(ServiceCreation {
                service: super::rpc::summary(
                    &graph,
                    &create.id,
                    mixengine_core::services::record(&self.store, &create.id)
                        .await
                        .ok()
                        .as_ref(),
                    &self.services.supervised(),
                ),
                // Said once, here, and never again: what the service *is* outlives the call and is
                // the summary beside this, while why it is not on 3306 is true only of this moment.
                moved_from: written.moved_from,
            }),

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
    /// supervised — a row deleted out from under a live process would leave the process with
    /// nothing describing it — and when a site declares it and `force` was not asked for; and the
    /// wire error of a directory that could not be removed.
    pub(crate) async fn service_delete(
        &self,
        id: &ServiceId,
        force: bool,
    ) -> Result<ServiceRemoval, Error> {
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

        // **The fourth refusal, and the second one `force` is allowed to cross** (spec D4). A site
        // naming this service — as its php-fpm pool or as a link — is a statement about the next
        // `site.start`, which somebody who has been shown the sites is entitled to overrule. The
        // check above is a fact about now and stays above this one, so `--force` at a running
        // service is still told to stop it first.
        if !force {
            let declared = mixengine_core::sites::declaring(&self.store, id)
                .await
                .map_err(|error| error.to_wire())?;

            if !declared.is_empty() {
                return Err(Error::new(
                    ErrorCode::PreconditionFailed,
                    format!("{id} is declared by {}", declared.join(", ")),
                )
                .with_hint(format!(
                    "`mix site update <site> --service …` drops it, or `mix service delete {id}                      --force` deletes it anyway — the sites keep running, with the declaration gone"
                )));
            }
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
