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
use std::pin::Pin;
use std::time::Duration;

use mixengine_core::config::PathOverrides;
use mixengine_core::{Paths, Store};
use mixengine_proto::{
    Millis, ReadyCheck, RestartPolicy, ServiceId, ServiceSpec, ServiceSpecBuilder, StopBehaviour,
};
use mixengine_testkit::{FakeService, Home};

use super::SpecSource;

/// How long a test waits for something on the other side of the machine.
///
/// Only ever waited out in full when something is wrong. Generous because starting a process on a
/// loaded Windows runner is measured in seconds.
pub(crate) const EVENTUALLY: Duration = Duration::from_secs(20);

/// The specs a test declares, answered as they are.
///
/// **The fixture half of the port**, and the reason the port exists: T30's generator is what will
/// build these from a `services` row and a package, and neither exists yet.
#[derive(Debug)]
pub(crate) struct Declared(pub(crate) Vec<ServiceSpec>);

impl SpecSource for Declared {
    fn declared(
        &self,
    ) -> Pin<Box<dyn Future<Output = anyhow::Result<Vec<ServiceSpec>>> + Send + '_>> {
        Box::pin(std::future::ready(Ok(self.0.clone())))
    }
}

/// A source that cannot answer, which is what a package that is not installed will look like.
#[derive(Debug)]
pub(crate) struct Unavailable;

impl SpecSource for Unavailable {
    fn declared(
        &self,
    ) -> Pin<Box<dyn Future<Output = anyhow::Result<Vec<ServiceSpec>>> + Send + '_>> {
        Box::pin(std::future::ready(Err(anyhow::anyhow!(
            "the package this service belongs to is not installed"
        ))))
    }
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
