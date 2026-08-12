//! Where a [`ServiceSpec`] comes from — the port, and nothing that answers it for real.
//!
//! **Nothing in this workspace produces a spec yet, and that is why this is a trait.** A `services`
//! row carries `package_id`, `port`, `data_dir`, `config_overrides_json` and `limits_json`, which is
//! the *input* to config generation rather than a spec; what turns it into one is the generator of
//! roadmap task **T30**, in Phase 3, against packages Phase 2 has not installed yet. The registry
//! needs the question answered now and the answer only later, so it depends on the question:
//! [`SpecSource`] here, a fixture in the tests, the generator when it exists.
//!
//! The alternative that was not taken is a `spec_json` column, which would duplicate three columns
//! the row already has and keep a second copy of what generation renders — `CLAUDE.md`'s
//! disposable-generated-config rule read backwards. See
//! `.claude/roadmap/phase-1-process-supervision.md`.

use std::future::Future;
use std::pin::Pin;

use mixengine_proto::ServiceSpec;

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
    /// Every service this home declares, built and validated.
    ///
    /// # Errors
    ///
    /// Whatever building them cost — a package that is not installed, a template that does not
    /// render, a database that cannot be read. [`anyhow::Error`] rather than a typed enum because
    /// this crate is not the one that will know: T30 owns the failures, and inventing their shape
    /// here would be guessing at a vocabulary a later phase has to live with.
    fn declared(
        &self,
    ) -> Pin<Box<dyn Future<Output = anyhow::Result<Vec<ServiceSpec>>> + Send + '_>>;
}

/// The source this build ships with: nothing is declared.
///
/// **Not a placeholder that lies.** A home has no services until Phase 3 can create one, so the
/// honest answer today is an empty set — and an empty set is a real answer that the registry, the
/// graph and `service.list` all handle without a special case. It is replaced by T30's generator,
/// which is the moment `services` rows begin to render into specs.
#[derive(Debug)]
pub(crate) struct Undeclared;

impl SpecSource for Undeclared {
    fn declared(
        &self,
    ) -> Pin<Box<dyn Future<Output = anyhow::Result<Vec<ServiceSpec>>> + Send + '_>> {
        Box::pin(std::future::ready(Ok(Vec::new())))
    }
}
