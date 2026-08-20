//! `runtime.list_extensions` and `runtime.set_extension` — roadmap task **T28**.
//!
//! Its own module rather than two more methods on [`crate::runtimes`] because of what it needs: a
//! toggle rewrites a file a *service* is reading, so this is the one runtime operation that has to
//! reach the registry. `runtimes.rs` is the index, the download and the row, and it holds none.
//!
//! # What a change answers about the pool
//!
//! **The daemon does not restart a pool nobody asked it to restart**; it reports. That is T32's own
//! policy for a changed override:
//!
//! | | Unix | Windows |
//! | --- | --- | --- |
//! | mechanism | `SIGUSR2`, which the pool's spec already carries | none — `php-cgi.exe` reads its ini at startup |
//! | answer | `Reloaded` | `RestartRequired` |
//!
//! A pool that is not running at all is `PoolNotRunning`, which is neither a failure nor a reload: it
//! reads the new set when it is started. The two are told apart by the *spec* and not by a `cfg` —
//! a pool whose recipe gave it no `ReloadBehaviour` is one nothing can hand a configuration to,
//! which is Windows today and is any future recipe that says the same.

use std::sync::Arc;

use mixengine_core::{Paths, Store};
use mixengine_proto::{
    Error, ErrorCode, ExtensionChange, ExtensionChoice, ExtensionList, ExtensionSource, Linkage,
    PoolOutcome, RuntimeExtension, RuntimeTarget,
};

use crate::error::ToWire as _;

/// The two extension methods, and everything they need to answer.
#[derive(Debug)]
pub(crate) struct Extensions {
    /// Where the generated set goes.
    paths: Paths,

    /// The rows the state is read from and the choice is written to.
    store: Store,

    /// What is running, for the one half of a change that is not a file.
    services: Arc<crate::services::Registry>,
}

impl Extensions {
    /// Build the door the two methods go through.
    pub(crate) fn new(
        paths: &Paths,
        store: &Store,
        services: Arc<crate::services::Registry>,
    ) -> Arc<Self> {
        Arc::new(Self {
            paths: paths.clone(),
            store: store.clone(),
            services,
        })
    }

    /// `runtime.list_extensions` — what this build has, and why each is in the state it is in.
    ///
    /// # Errors
    ///
    /// `not_found` when that version is not installed, and the wire error of a row that could not be
    /// read.
    pub(crate) async fn list(&self, target: &RuntimeTarget) -> Result<ExtensionList, Error> {
        let state =
            mixengine_core::runtimes::extensions::state(&self.store, target.kind, &target.version)
                .await
                .map_err(|error| error.to_wire())?;

        Ok(ExtensionList {
            extensions: state.listing().into_iter().map(wire).collect(),
        })
    }

    /// `runtime.set_extension` — turn one round, rewrite the set, and tell the pool.
    ///
    /// # Errors
    ///
    /// `unsupported_platform` for an extension this build compiles in, `not_found` for a name it
    /// does not ship or a version that is not installed, and the wire error of a row or a file that
    /// could not be written.
    pub(crate) async fn set(&self, choice: &ExtensionChoice) -> Result<ExtensionChange, Error> {
        let state = mixengine_core::runtimes::extensions::choose(
            &self.store,
            choice.runtime.kind,
            &choice.runtime.version,
            &choice.name,
            choice.enabled,
        )
        .await
        .map_err(|error| error.to_wire())?;

        let changed = mixengine_core::runtimes::extensions::render(&self.paths, &state)
            .await
            .map_err(|error| error.to_wire())?;

        // A set that did not move asks nothing of the pool: turning on what was already on is not a
        // reload, and `document::install`'s diff is what makes that true rather than a check here.
        let pool = if changed {
            self.tell_the_pool(&choice.runtime).await
        } else {
            PoolOutcome::PoolNotRunning
        };

        let extension = state
            .listing()
            .into_iter()
            .find(|extension| extension.name == choice.name)
            .map(wire)
            .ok_or_else(|| {
                Error::new(
                    ErrorCode::Internal,
                    "the extension that was just set is not in its own listing",
                )
            })?;

        Ok(ExtensionChange { extension, pool })
    }

    /// Which of the three things happened to the pool that runs this version.
    async fn tell_the_pool(&self, target: &RuntimeTarget) -> PoolOutcome {
        let pool =
            mixengine_core::services::pools::of(&self.store, target.kind, &target.version).await;

        let Ok(Some(id)) = pool else {
            // No pool declared for this runtime, or a table that could not be read. Neither is a
            // failure of the toggle — the file is written either way — and both mean nothing is
            // holding the old set.
            return PoolOutcome::PoolNotRunning;
        };

        if !self.services.ask_to_reload(&id) {
            return PoolOutcome::PoolNotRunning;
        }

        // **Decided by the spec and not by a `cfg`.** A pool whose recipe gave it no
        // `ReloadBehaviour` is one nothing can hand a configuration to; that is Windows today and is
        // any future recipe that says the same.
        match self.services.graph().await {
            Ok(graph) => match graph.spec(&id) {
                Some(spec) if spec.reload().is_some() => PoolOutcome::Reloaded,
                _ => PoolOutcome::RestartRequired,
            },

            // Answered as the outcome that tells somebody to do something, rather than as the one
            // that claims something was done.
            Err(error) => {
                // `?` and not `%`: `Undeclarable` is matched by its one other caller rather than
                // printed, and this is a line in `daemon.log` rather than a sentence for a person.
                tracing::warn!(%id, ?error, "could not tell whether this pool can be reloaded");
                PoolOutcome::RestartRequired
            }
        }
    }
}

/// One extension, as the wire spells it.
///
/// Two vocabularies rather than one shared type, on `index::Channel`'s precedent: what the domain
/// calls a linkage and what the API answers are allowed to move apart, and a variant added on either
/// side has to face this `match`.
fn wire(extension: mixengine_core::runtimes::extensions::Extension) -> RuntimeExtension {
    use mixengine_core::runtimes::extensions::{Linkage as Domain, Source as Decided};

    RuntimeExtension {
        name: extension.name,
        linkage: match extension.linkage {
            Domain::Static => Linkage::Static,
            Domain::Shared => Linkage::Shared,
        },
        enabled: extension.enabled,
        source: match extension.source {
            Decided::BuildDefault => ExtensionSource::BuildDefault,
            Decided::User => ExtensionSource::User,
        },
    }
}
