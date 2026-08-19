//! The service a runtime install creates for itself — roadmap task **T32**.
//!
//! Nobody calls `service.create` for a pool. `.claude/features/runtime-versions.md` decided this
//! before there was a pool to create: PHP's post-install hook makes the `php-fpm@<version>` record,
//! and an uninstall takes it away. What is here is that hook, and it is written **idempotent and run
//! at boot as well as after an install** — which is what gives a PHP installed before this task a
//! pool without a data migration, and what repairs a home whose row was deleted by hand.
//!
//! **Which runtimes get one is the catalogue's answer, not a list here.** A recipe says where its
//! binary comes from ([`Source`]), so this walks the recipes rather than the languages: the day
//! `node` grows a supervised service, its recipe says so and this needs no edit.

use mixengine_proto::{PackageVersion, RuntimeKind, ServiceId};

use super::{Declaration, Origin};
use crate::generate::{Catalogue, Source};
use crate::{Error, Result, Store};

/// The first port a pool that needs one is offered.
///
/// `.claude/features/services.md`'s own `127.0.0.1:9xxx`, and php-fpm's conventional 9000 — a number
/// somebody debugging a FastCGI connection will recognise on sight.
const FIRST_PORT: u16 = 9000;

/// Give every installed runtime the service its recipe says it should have, and say which were made.
///
/// **A no-op on a home that is already right**, which is what lets it run at boot: the cost of a
/// call with nothing to do is one query per runtime-backed recipe.
///
/// # Errors
///
/// [`Error::Database`] when the tables cannot be read or written, and whatever
/// [`create`](super::create) reports for a row that cannot be written — except
/// [`Error::ServiceAlreadyDeclared`], which is this function's own no-op and is swallowed: two
/// daemons racing to repair one home is not a failure of either.
pub async fn ensure(store: &Store, catalogue: &Catalogue) -> Result<Vec<ServiceId>> {
    let mut created = Vec::new();

    for package in catalogue.packages() {
        let Some(recipe) = catalogue.recipe(package) else {
            continue;
        };

        let Source::Runtime(kind) = recipe.source() else {
            continue;
        };

        // A pool listens on a socket where there are sockets, and on a port where there are not —
        // which is the recipe's own split, read here because the port has to be *allocated* at
        // creation rather than derived at every start. `cfg!` is a value, so both sides compile
        // everywhere.
        let listens_on_a_port = !cfg!(unix);
        let kind_column = kind.as_str();

        // Every installed version of that language that has no service pointing at it. One query
        // rather than one per version, because a boot on a home with six PHPs should not be six
        // round trips to answer "nothing to do".
        let missing = sqlx::query_scalar!(
            "SELECT r.version
             FROM runtime_installs r
             WHERE r.kind = ?
               AND NOT EXISTS (SELECT 1 FROM services s WHERE s.runtime_install_id = r.id)
             ORDER BY r.version",
            kind_column
        )
        .fetch_all(store.pool())
        .await
        .map_err(|source| store.failure("read", source))?;

        for version in missing {
            let version = PackageVersion::parse(&version).map_err(|_| Error::NotFound {
                kind: "runtime",
                id: format!("{kind_column} {version}"),
            })?;

            let service =
                ServiceId::parse(format!("{package}@{version}")).map_err(|_| Error::NotFound {
                    kind: "service",
                    id: format!("{package}@{version}"),
                })?;

            let port = if listens_on_a_port {
                Some(free_port(store).await?)
            } else {
                None
            };

            match super::create(
                store,
                &Declaration {
                    service: service.clone(),
                    origin: Origin::Runtime {
                        kind,
                        version: version.clone(),
                    },
                    instance_name: version.as_str().to_owned(),
                    port,
                    bind_addr: None,
                    data_dir: None,
                    // **Not on by default.** A user who installs four PHPs to test against has not
                    // asked for four pools at every boot, and `mix service` is one command.
                    autostart: false,
                    overrides: "{}".to_owned(),
                },
            )
            .await
            {
                Ok(()) => created.push(service),

                // Two daemons repairing one home, or an install racing a boot. The row it wanted is
                // there, which is what it wanted.
                Err(Error::ServiceAlreadyDeclared { .. }) => {}

                Err(error) => return Err(error),
            }
        }
    }

    if !created.is_empty() {
        tracing::info!(pools = ?created, "installed runtimes were given the services they need");
    }

    Ok(created)
}

/// The service that runs out of one installed runtime, if there is one.
///
/// What `runtime.uninstall` asks before it removes a directory.
///
/// # Errors
///
/// [`Error::Database`] when the tables cannot be read.
pub async fn of(
    store: &Store,
    kind: RuntimeKind,
    version: &PackageVersion,
) -> Result<Option<ServiceId>> {
    let (kind_column, version_column) = (kind.as_str(), version.as_str());

    let id = sqlx::query_scalar!(
        "SELECT s.id
         FROM services s
         JOIN runtime_installs r ON r.id = s.runtime_install_id
         WHERE r.kind = ? AND r.version = ?",
        kind_column,
        version_column
    )
    .fetch_optional(store.pool())
    .await
    .map_err(|source| store.failure("read", source))?;

    Ok(id.and_then(|id| ServiceId::parse(id).ok()))
}

/// The lowest port from [`FIRST_PORT`] that no `services` row already holds.
///
/// **The table and not the machine**, deliberately: what this is avoiding is two pools configured on
/// one number, which is a fact about this home and is stable across reboots. Whether something else
/// on the machine holds it is a different question with a different answer every day, and the one
/// this cannot usefully ask — a port free at install time may be taken by the time the pool starts,
/// and a start that fails says so with the port in it.
///
/// # Errors
///
/// [`Error::Database`] when the table cannot be read.
async fn free_port(store: &Store) -> Result<u16> {
    let taken: Vec<i64> =
        sqlx::query_scalar!("SELECT port FROM services WHERE port IS NOT NULL ORDER BY port")
            .fetch_all(store.pool())
            .await
            .map_err(|source| store.failure("read", source))?
            .into_iter()
            .flatten()
            .collect();

    let mut port = FIRST_PORT;
    for held in taken {
        if i64::from(port) == held {
            port = port.saturating_add(1);
        }
    }

    Ok(port)
}

#[cfg(test)]
mod tests {
    use super::*;

    async fn store() -> (tempfile::TempDir, Store) {
        let home = tempfile::tempdir().expect("a temporary directory");
        let store = Store::open(&home.path().join(crate::paths::DATABASE_FILE_NAME))
            .await
            .expect("a database");
        (home, store)
    }

    /// A PHP of `version`, as `runtime.install` would have left it.
    async fn install(store: &Store, version: &str) {
        sqlx::query(
            "INSERT INTO runtime_installs
                 (kind, version, channel, install_path, installed_at, size_bytes, source_url,
                  sha256, provides_json)
             VALUES ('php', ?, 'stable', ?, '2026-08-19T00:00:00Z', 1,
                     'https://example.invalid/php', 'abc', '{}')",
        )
        .bind(version)
        .bind(format!("/runtimes/php/{version}"))
        .execute(store.pool())
        .await
        .expect("a runtime install");
    }

    /// Every installed PHP ends up with a pool, and asking twice creates nothing.
    ///
    /// **Idempotence is the whole design**, not a nicety: this runs at every boot as well as after
    /// every install, which is what gives a PHP installed before T32 a pool without a data migration
    /// and repairs a home whose row somebody deleted by hand.
    #[tokio::test]
    async fn every_installed_php_gets_one_pool_and_only_one() {
        let (_home, store) = store().await;
        install(&store, "8.3.33").await;
        install(&store, "8.4.1").await;

        let created = ensure(&store, &Catalogue::builtin())
            .await
            .expect("pools for both");

        assert_eq!(
            created.iter().map(ServiceId::as_str).collect::<Vec<_>>(),
            ["php-fpm@8.3.33", "php-fpm@8.4.1"],
            "one pool each, named by the full version"
        );

        let again = ensure(&store, &Catalogue::builtin())
            .await
            .expect("nothing to do");

        assert!(again.is_empty(), "a second pass created {again:?}");
    }

    /// A pool on a system that listens on TCP is given a port when it is created, and keeps it.
    ///
    /// Allocated here rather than derived from the version, because two PHPs whose versions differ
    /// in a digit nobody looks at would otherwise collide — and written into the row rather than
    /// recomputed, because a port that moved between restarts is a Caddy pointed at nothing.
    #[tokio::test]
    async fn a_pool_that_needs_a_port_is_given_a_free_one() {
        let (_home, store) = store().await;
        install(&store, "8.3.33").await;
        install(&store, "8.4.1").await;

        ensure(&store, &Catalogue::builtin()).await.expect("pools");

        let ports: Vec<Option<i64>> = sqlx::query_scalar("SELECT port FROM services ORDER BY id")
            .fetch_all(store.pool())
            .await
            .expect("the rows");

        if cfg!(unix) {
            assert_eq!(ports, [None, None], "a socket needs no port");
        } else {
            assert_eq!(ports, [Some(9000), Some(9001)]);
        }
    }

    /// The pool a runtime is running out of, found by the pair `runtime.uninstall` has in hand.
    #[tokio::test]
    async fn a_runtime_can_be_asked_which_service_runs_out_of_it() {
        let (_home, store) = store().await;
        install(&store, "8.3.33").await;

        let version = PackageVersion::parse("8.3.33").expect("a version");

        assert_eq!(
            of(&store, RuntimeKind::Php, &version)
                .await
                .expect("a lookup"),
            None,
            "nothing runs out of it until the hook has run"
        );

        ensure(&store, &Catalogue::builtin()).await.expect("a pool");

        assert_eq!(
            of(&store, RuntimeKind::Php, &version)
                .await
                .expect("a lookup")
                .as_ref()
                .map(ServiceId::as_str),
            Some("php-fpm@8.3.33")
        );
    }
}
