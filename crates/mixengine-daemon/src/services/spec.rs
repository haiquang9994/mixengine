//! Where a [`ServiceSpec`](mixengine_proto::ServiceSpec) comes from — the port, and the thing that
//! answers it.
//!
//! **T19 wrote the question and T30 wrote the answer.** A `services` row carries `package_id`,
//! `port`, `data_dir`, `config_overrides_json` and `limits_json`, which is the *input* to config
//! generation rather than a spec; what turns it into one is
//! [`mixengine_core::generate::Generator`], which renders the service's configuration into
//! `etc/<service-id>/` and builds the specification that points at it. The registry still depends on
//! the question rather than on the answer — [`SpecSource`] here, a fixture in the tests, the
//! generator in a running daemon — because the fixture is what lets the registry's own tests declare
//! a service without a database row, a recipe or a file on disk.
//!
//! The alternative that was not taken is a `spec_json` column, which would duplicate three columns
//! the row already has and keep a second copy of what generation renders — `CLAUDE.md`'s
//! disposable-generated-config rule read backwards. See
//! `.claude/roadmap/phase-1-process-supervision.md`.

use std::future::Future;
use std::pin::Pin;
use std::sync::Arc;

use mixengine_core::generate::{Catalogue, Generated, Generator};
use mixengine_core::{Paths, Store};

#[cfg(debug_assertions)]
use super::fakeservice;

/// The source this daemon runs with.
///
/// One function rather than a line in `main`, because the choice has a `cfg` in it and this is the
/// module that owns what a source is.
pub(crate) fn declared(paths: &Paths, store: &Store) -> Arc<dyn SpecSource> {
    Arc::new(Rendered(Generator::new(
        paths.clone(),
        store.clone(),
        catalogue(),
    )))
}

/// The recipes this daemon can find.
///
/// [`Catalogue::builtin`] is what MixEngine ships, and Phase 3 fills it one service at a time — Caddy
/// is in, the databases and caches follow. A **debug** build adds the `fakeservice` fixture beside
/// them, which is what a suite declares when the thing under test is the supervision and not the
/// server; see [`super::fakeservice`].
fn catalogue() -> Catalogue {
    let catalogue = Catalogue::builtin();

    #[cfg(debug_assertions)]
    let catalogue = catalogue.with(Arc::new(fakeservice::Fakeservice));

    catalogue
}

/// The declared services of this home, as runnable specifications.
///
/// **The whole set rather than one by id**, because the caller's next move is a
/// [`ServiceGraph`](mixengine_core::services::ServiceGraph): dependencies, cycles and start order
/// are properties of a set, and a source asked one spec at a time could not be checked for any of
/// them. Picking one out afterwards is [`ServiceGraph::spec`](mixengine_core::services::ServiceGraph::spec).
///
/// Boxed future rather than `async fn`, because this trait is used as `dyn SpecSource`: an
/// `async fn` in a trait is not dyn-compatible, and the registry holds one behind an [`Arc`] for the
/// whole life of the daemon.
///
/// [`Arc`]: std::sync::Arc
pub(crate) trait SpecSource: std::fmt::Debug + Send + Sync {
    /// Every service this home declares, built and validated — and what rendering it cost.
    ///
    /// **[`Generated`] rather than a bare `ServiceSpec`**, because asking the source is also when
    /// the configuration on disk is brought up to date, and whether any of it *moved* is knowledge
    /// that exists for exactly one instant: the answer is a comparison against what was there
    /// before, and by the time the caller has the spec the file has already been overwritten. It is
    /// what [`Registry::graph`](super::Registry::graph) hands to a running process as a reload.
    ///
    /// # Errors
    ///
    /// Whatever building them cost — a package with no recipe, an override that names nothing, a
    /// template that does not render, a database that cannot be read. [`anyhow::Error`] rather than
    /// a typed enum because this crate is not the one that knows: the vocabulary is
    /// [`mixengine_core::Error`]'s, and restating it here would be a second list to keep in step.
    fn declared(&self)
    -> Pin<Box<dyn Future<Output = anyhow::Result<Vec<Generated>>> + Send + '_>>;
}

/// The source a running daemon uses: the `services` table, rendered.
///
/// Asked at the top of every walk, which is also when the configuration on disk is brought up to
/// date — the two are one operation, and doing them apart is what lets a service be started with a
/// config file from before the change that prompted the start. Repeating it costs almost nothing:
/// a rendering identical to what is already there is not written, and nothing is reloaded.
#[derive(Debug)]
struct Rendered(Generator);

impl SpecSource for Rendered {
    fn declared(
        &self,
    ) -> Pin<Box<dyn Future<Output = anyhow::Result<Vec<Generated>>> + Send + '_>> {
        Box::pin(async move { Ok(self.0.declared().await?) })
    }
}
