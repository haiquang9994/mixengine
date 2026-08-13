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
use std::sync::Arc;

use mixengine_proto::ServiceSpec;

/// The variable a debug build reads its declarations from. See [`declared`].
const DEV_SPECS: &str = "MIXENGINE_DEV_SPECS";

/// The source this daemon runs with.
///
/// One function rather than a line in `main`, because the choice has a `cfg` in it and this is the
/// module that owns what a source is. It becomes T30's generator, unconditionally, the day one
/// exists.
pub(crate) fn declared() -> Arc<dyn SpecSource> {
    #[cfg(debug_assertions)]
    if let Some(path) = std::env::var_os(DEV_SPECS) {
        // Exported and empty is how a shell leaves a variable it did not manage to fill in, and
        // taking it at face value would answer every `service.*` call with "cannot read the
        // declarations in " — a failure that names no file and looks like a broken daemon rather
        // than a broken variable. `crates/mixengine-cli/src/home.rs` refuses an empty `--home` for
        // the same reason; this one falls through instead, because unlike a home, having no
        // declarations at all is the normal state of a build before T30.
        if path.is_empty() {
            tracing::warn!("{DEV_SPECS} is set to an empty path and is being ignored");
        } else {
            let path = std::path::PathBuf::from(path);
            tracing::warn!(
                path = %path.display(),
                "reading service declarations from {DEV_SPECS} — a debug build only"
            );
            return Arc::new(DevSpecs { path });
        }
    }

    // A release build has no `DevSpecs` at all, so a variable set against one would otherwise be
    // silently ignored — and "my services are not there" is a bad way to find that out.
    #[cfg(not(debug_assertions))]
    if std::env::var_os(DEV_SPECS).is_some() {
        tracing::warn!("{DEV_SPECS} is set and this is a release build, which ignores it");
    }

    Arc::new(Undeclared)
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

/// Declarations read from a JSON file named by `MIXENGINE_DEV_SPECS`.
///
/// **A debug build only, and it exists for one reason**: until T30 there is no way to declare a
/// service to a *real* `mixengined` process, so nothing outside this crate's own unit tests can
/// drive a service through the daemon at all — which is exactly what T19b's `mix service` has to be
/// proved against, over a real endpoint, with a real process on the end of it.
///
/// **Why it is gated rather than shipped.** The file names a program and its arguments, so a
/// release binary that read one would be a supervisor that runs whatever a variable points at; a
/// debug build is a machine somebody is already developing MixEngine on. [`declared`] says so out
/// loud on startup either way, because a variable that is quietly ignored is worse than one that is
/// refused.
///
/// It is **not** a config format and nothing should grow around it: the file is
/// `serde_json`'s view of [`ServiceSpec`], whatever that happens to be today, and T30's generator
/// deletes this type.
#[cfg(debug_assertions)]
#[derive(Debug)]
pub(crate) struct DevSpecs {
    /// What `MIXENGINE_DEV_SPECS` pointed at.
    path: std::path::PathBuf,
}

#[cfg(debug_assertions)]
impl SpecSource for DevSpecs {
    /// Read on every call rather than once at startup, so a test — or a person — can add a service
    /// without restarting the daemon. The registry asks for the set at the top of every walk, which
    /// is the same cadence T30's generator will be asked at.
    fn declared(
        &self,
    ) -> Pin<Box<dyn Future<Output = anyhow::Result<Vec<ServiceSpec>>> + Send + '_>> {
        use anyhow::Context as _;

        Box::pin(async move {
            let file = self.path.display();

            let text = tokio::fs::read_to_string(&self.path)
                .await
                .with_context(|| format!("cannot read the declarations in {file}"))?;

            let specs: Vec<ServiceSpec> = serde_json::from_str(&text)
                .with_context(|| format!("{file} is not a JSON array of service specifications"))?;

            // `Deserialize` checks nothing by design — `ServiceSpec` says so — and this is the
            // loader, which is the layer that knows which file to name.
            for spec in &specs {
                spec.validate()
                    .with_context(|| format!("{file} declares a service that cannot be run"))?;
            }

            Ok(specs)
        })
    }
}
