//! The scaffolding two test modules need: a home with rows in it, and a source of specs.
//!
//! **It lives in this crate rather than in `mixengine-testkit` because [`SpecSource`] is this
//! crate's own trait** — nothing outside the daemon can implement one. What it shares with the
//! testkit is `FakeService`, which is what every spec here is built around.
//!
//! Two callers, which is why it is a module and not a helper in one of them: the registry's own
//! tests walk plans directly, and the `service.*` handlers (T19a) go through the same registry from
//! above. A second copy of "a home with a `services` row" would be a second thing to keep in step
//! with `0001_initial.sql`.

use std::future::Future;
use std::path::PathBuf;
use std::pin::Pin;
use std::time::Duration;

use mixengine_core::config::PathOverrides;
use mixengine_core::generate::{Generated, Settings, Written};
use mixengine_core::{Paths, Store};
use mixengine_proto::{
    Millis, ReadyCheck, RestartPolicy, ServiceId, ServiceSpec, ServiceSpecBuilder, StopBehaviour,
};
use mixengine_testkit::{FakeService, Home};

use std::sync::Arc;

use tokio_util::sync::CancellationToken;

use super::{Events, Registry, SpecSource};

/// How long a test waits for something on the other side of the machine.
///
/// Only ever waited out in full when something is wrong. Generous because starting a process on a
/// loaded Windows runner is measured in seconds.
pub(crate) const EVENTUALLY: Duration = Duration::from_secs(20);

/// The specs a test declares, answered as they are, with nothing on disk having moved.
///
/// **The fixture half of the port**, and the reason the port exists: the real source renders a
/// `services` row through a recipe and a template, and a registry test is about neither.
#[derive(Debug)]
pub(crate) struct Declared(pub(crate) Vec<ServiceSpec>);

impl SpecSource for Declared {
    fn declared(
        &self,
    ) -> Pin<Box<dyn Future<Output = mixengine_core::Result<Vec<Generated>>> + Send + '_>> {
        Box::pin(std::future::ready(Ok(generated(
            &self.0,
            Written::Unchanged,
        ))))
    }

    /// **Nothing, and that is the truthful answer here** — roadmap task **T53**. This fixture holds
    /// specs a test wrote by hand; there is no recipe behind them and so no setting to merge.
    fn settings(
        &self,
        _service: &ServiceId,
    ) -> Pin<Box<dyn Future<Output = mixengine_core::Result<Option<Settings>>> + Send + '_>> {
        Box::pin(std::future::ready(Ok(None)))
    }
}

/// The same, from a home whose configuration is rewritten every time it is asked — roadmap task
/// **T31**.
///
/// What the real generator reports for a service somebody has just reconfigured, and what the
/// registry acts on by asking that service to re-read its file. Every walk, deliberately: a test
/// that had to arrange for exactly one changed answer would be testing its own bookkeeping.
#[derive(Debug)]
pub(crate) struct Rerendered(pub(crate) Vec<ServiceSpec>);

impl SpecSource for Rerendered {
    fn declared(
        &self,
    ) -> Pin<Box<dyn Future<Output = mixengine_core::Result<Vec<Generated>>> + Send + '_>> {
        Box::pin(std::future::ready(Ok(generated(&self.0, Written::Updated))))
    }

    /// **Nothing, and that is the truthful answer here** — roadmap task **T53**. This fixture holds
    /// specs a test wrote by hand; there is no recipe behind them and so no setting to merge.
    fn settings(
        &self,
        _service: &ServiceId,
    ) -> Pin<Box<dyn Future<Output = mixengine_core::Result<Option<Settings>>> + Send + '_>> {
        Box::pin(std::future::ready(Ok(None)))
    }
}

/// A source that cannot answer, which is what a package that is not installed will look like.
#[derive(Debug)]
pub(crate) struct Unavailable;

impl SpecSource for Unavailable {
    fn declared(
        &self,
    ) -> Pin<Box<dyn Future<Output = mixengine_core::Result<Vec<Generated>>> + Send + '_>> {
        Box::pin(std::future::ready(Err(mixengine_core::Error::NotFound {
            kind: "package",
            id: "the one this service belongs to".to_owned(),
        })))
    }

    /// **Nothing, and that is the truthful answer here** — roadmap task **T53**. This fixture holds
    /// specs a test wrote by hand; there is no recipe behind them and so no setting to merge.
    fn settings(
        &self,
        _service: &ServiceId,
    ) -> Pin<Box<dyn Future<Output = mixengine_core::Result<Option<Settings>>> + Send + '_>> {
        Box::pin(std::future::ready(Ok(None)))
    }
}

/// `specs`, each reported as having cost one file that was written this way.
///
/// The file is named rather than left out because [`Generated::changed`] is read off the list: an
/// empty one is a service that rendered nothing, which is a different claim from one that rendered
/// something and found it identical.
fn generated(specs: &[ServiceSpec], written: Written) -> Vec<Generated> {
    specs
        .iter()
        .map(|spec| Generated {
            spec: spec.clone(),
            files: vec![(PathBuf::from("fakeservice.args"), written)],
            // Nothing this fixture renders lives in a swept directory, so there is never anything
            // for a walk to take away.
            removed: Vec::new(),
            // A fixture service has nothing to do once before it starts: the recipes that do are
            // the databases, and they are driven against a real server rather than against this.
            first_run: None,
            // And nothing to make a database on, for the same reason — roadmap task T77a.
            provisioning: None,
        })
        .collect()
}

/// A `fakeservice` that announces itself and then waits to be stopped.
pub(crate) fn spec(id: &str) -> ServiceSpecBuilder {
    ServiceSpec::builder(service(id), FakeService::program())
        .cwd(std::env::temp_dir())
        .ready(ReadyCheck::LogPattern {
            regex: mixengine_testkit::service::READY_LINE.to_owned(),
            timeout: Millis::from_secs(20),
        })
        // Nothing under test here restarts on purpose, and a policy that did would turn a failing
        // assertion into a test that takes a minute to fail.
        .restart(RestartPolicy::Never)
        .stop(StopBehaviour::Signal { grace: Millis(500) })
}

/// An id, or the test's own bug.
pub(crate) fn service(id: &str) -> ServiceId {
    ServiceId::parse(id).expect("a valid service id")
}

/// A configured `FakeService` as the arguments a spec passes it.
pub(crate) fn arguments(fake: &FakeService) -> Vec<String> {
    fake.args()
        .iter()
        .map(|arg| arg.to_string_lossy().into_owned())
        .collect()
}

/// A home with a database, and a `services` row for each of `ids`.
///
/// The foreign key to `packages` is `NOT NULL` and enforced, so a package row comes first — which is
/// the constraint doing its job rather than an obstacle to route around. Phase 3's `service.create`
/// is what will do this for real.
pub(crate) async fn home(ids: &[&str]) -> (Home, Paths, Store) {
    let home = Home::new();
    let paths = Paths::new(home.path().to_path_buf(), &PathOverrides::default());
    let store = Store::open(paths.database_file())
        .await
        .expect("a database");

    for id in ids {
        sqlx::query(
            "INSERT INTO packages (name, version, install_path, installed_at, source_url, sha256)
             VALUES (?, '1.0.0', '/packages/x', '2026-08-12T00:00:00Z', 'https://example', 'ab')",
        )
        .bind(id)
        .execute(store.pool())
        .await
        .expect("a package for the service to belong to");

        sqlx::query(
            "INSERT INTO services (id, package_id, instance_name, state)
             VALUES (?, (SELECT id FROM packages WHERE name = ?), 'main', 'stopped')",
        )
        .bind(id)
        .bind(id)
        .execute(store.pool())
        .await
        .expect("the service row");
    }

    (home, paths, store)
}

/// A registry over `specs`, on a mock host that answers for this home.
///
/// **Moved here from the registry's own test module** when the activator became a third caller —
/// this module exists for exactly the scaffolding more than one test module needs.
pub(crate) fn registry(paths: &Paths, store: &Store, specs: Arc<dyn SpecSource>) -> Registry {
    registry_on(
        paths,
        store,
        specs,
        Arc::new(mixengine_platform::mock::Host::with_home(paths.root())),
    )
}

/// [`registry`], for the one test whose subject is what the machine answers.
pub(crate) fn registry_on(
    paths: &Paths,
    store: &Store,
    specs: Arc<dyn SpecSource>,
    host: Arc<dyn mixengine_platform::Host>,
) -> Registry {
    let events = Events::new();
    // A real one, because it is what a start reaches for when a service declares a first-run
    // ritual — and none of these fixtures does, so nothing here ever begins a job through it.
    let jobs = Arc::new(crate::jobs::Jobs::new(
        store,
        events.clone(),
        CancellationToken::new(),
    ));

    Registry::new(
        paths,
        store,
        host,
        events,
        specs,
        CancellationToken::new(),
        jobs,
    )
}
