//! The service a runtime install creates for itself — roadmap task **T32**.
//!
//! Nobody calls `service.create` for a pool. `.claude/features/runtime-versions.md` decided this
//! before there was a pool to create: PHP's post-install hook makes the `php-fpm@<version>` record,
//! and an uninstall takes it away. What is here is that hook, and it is written **idempotent and run
//! at boot as well as after an install** — which is what gives a PHP installed before this task a
//! pool without a data migration, and what repairs a home whose row was deleted by hand.
//!
//! **The port a pool needs is no longer this module's arithmetic** — roadmap task **T34c**. It used
//! to keep an allocator of its own that walked the `services` table from 9000 and deliberately
//! asked the machine nothing, on the reasoning that a port free at install time may be taken by the
//! time the pool starts. That is still true and is no longer the whole story: what the two
//! databases forced is that a number this home writes down while an XAMPP is listening on it is a
//! service configured to fail, and a bind costs nothing here. So there is one allocator for every
//! service ([`ports`](super::ports)), the wish belongs to the recipe, and a pool gets its 9000 the
//! same way `mariadb@main` gets its 3306.
//!
//! **Which runtimes get one is the catalogue's answer, not a list here.** A recipe says where its
//! binary comes from ([`Source`]), so this walks the recipes rather than the languages: the day
//! `node` grows a supervised service, its recipe says so and this needs no edit.

use mixengine_proto::{PackageVersion, RuntimeKind, ServiceId};

use super::{Declaration, Origin, Port};
use crate::generate::{Catalogue, Source};
use crate::{Error, Result, Store};

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
pub async fn ensure(
    store: &Store,
    host: &dyn mixengine_platform::Host,
    catalogue: &Catalogue,
) -> Result<Vec<ServiceId>> {
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
        //
        // **The *shared* pool, which is the one whose instance is the version** — roadmap task
        // **T82a**, its design's D6. `NOT EXISTS (… WHERE s.runtime_install_id = r.id)` was enough
        // while a runtime had at most one service; a `web-app` extension's pool now satisfies it
        // while being no use at all to a project site, and would stop this repair from ever making
        // the shared one again.
        let missing = sqlx::query_scalar!(
            "SELECT r.version
             FROM runtime_installs r
             WHERE r.kind = ?
               AND NOT EXISTS (SELECT 1 FROM services s
                               WHERE s.runtime_install_id = r.id
                                 AND s.instance_name = r.version)
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

            // The wish is the recipe's, and the allocation is `create`'s — a pool is the one
            // service created without a caller, so this is where the two meet for it.
            let port = match recipe.preferred_port().filter(|_| listens_on_a_port) {
                Some(preferred) => Port::Allocate { preferred },
                None => Port::None,
            };

            match super::create(
                store,
                host,
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
                Ok(_) => created.push(service),

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

/// Every service that runs out of one installed runtime, in id order.
///
/// **Plural since roadmap task T82a, and the reason is that a runtime now has more than one pool.**
/// A `web-app` extension owns `php-fpm@<extension-id>` on the same `runtime_installs` row as the
/// shared `php-fpm@<version>` (that task's D1), so a lookup answering one of them answers an
/// arbitrary one — and both callers want all of them. `runtime.uninstall` refuses over a pool that
/// is running and deletes the stopped ones; the php-extension toggle reloads whatever reads the ini
/// set it just rewrote, which is every pool of that version because `PHP_INI_SCAN_DIR` is per
/// runtime.
///
/// **One function and not two.** A singular `of` beside this, answering "the shared one", would be a
/// near-identical lookup for a caller to reach for by mistake — and what a mistake costs here is a
/// PHP removed from under a running process.
///
/// An id in `services` that will not parse is left out, which is the same answer as no row: there is
/// nothing this build could do with it.
///
/// # Errors
///
/// [`Error::Database`] when the tables cannot be read.
pub async fn of_runtime(
    store: &Store,
    kind: RuntimeKind,
    version: &PackageVersion,
) -> Result<Vec<ServiceId>> {
    let (kind_column, version_column) = (kind.as_str(), version.as_str());

    let ids = sqlx::query_scalar!(
        "SELECT s.id
         FROM services s
         JOIN runtime_installs r ON r.id = s.runtime_install_id
         WHERE r.kind = ? AND r.version = ?
         ORDER BY s.id",
        kind_column,
        version_column
    )
    .fetch_all(store.pool())
    .await
    .map_err(|source| store.failure("read", source))?;

    Ok(ids
        .into_iter()
        .filter_map(|id| ServiceId::parse(id).ok())
        .collect())
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A host that answers nothing about the machine, which is all a pool's allocation asks it.
    fn host() -> mixengine_platform::mock::Host {
        mixengine_platform::mock::Host::with_home("/mixengine")
    }

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

        let created = ensure(&store, &host(), &Catalogue::builtin())
            .await
            .expect("pools for both");

        assert_eq!(
            created.iter().map(ServiceId::as_str).collect::<Vec<_>>(),
            ["php-fpm@8.3.33", "php-fpm@8.4.1"],
            "one pool each, named by the full version"
        );

        let again = ensure(&store, &host(), &Catalogue::builtin())
            .await
            .expect("nothing to do");

        assert!(again.is_empty(), "a second pass created {again:?}");
    }

    /// A pool on a system that listens on TCP is given a port when it is created, and keeps it.
    ///
    /// The number itself is [`ports::allocate`](super::ports::allocate)'s since T34c; what this
    /// asserts is the half that belongs here — that a pool asks for one at all on the systems where
    /// it listens on TCP, and asks for none where it listens on a socket.
    ///
    /// Allocated here rather than derived from the version, because two PHPs whose versions differ
    /// in a digit nobody looks at would otherwise collide — and written into the row rather than
    /// recomputed, because a port that moved between restarts is a Caddy pointed at nothing.
    #[tokio::test]
    async fn a_pool_that_needs_a_port_is_given_a_free_one() {
        let (_home, store) = store().await;
        install(&store, "8.3.33").await;
        install(&store, "8.4.1").await;

        ensure(&store, &host(), &Catalogue::builtin())
            .await
            .expect("pools");

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

    /// **Every pool a runtime is running, found by the pair `runtime.uninstall` has in hand** —
    /// roadmap task **T82a**, its design's D6.
    ///
    /// A runtime used to have at most one, and the lookup answered one. A `web-app` extension owns a
    /// second on the same install, and a caller shown one of two would delete the shared pool, leave
    /// the extension's, and then meet `runtime_installs`' `ON DELETE RESTRICT` as a foreign-key
    /// message about a column.
    #[tokio::test]
    async fn a_runtime_answers_with_every_pool_that_runs_out_of_it() {
        let (_home, store) = store().await;
        install(&store, "8.3.33").await;

        let version = PackageVersion::parse("8.3.33").expect("a version");

        assert!(
            of_runtime(&store, RuntimeKind::Php, &version)
                .await
                .expect("a lookup")
                .is_empty(),
            "nothing runs out of it until the hook has run"
        );

        ensure(&store, &host(), &Catalogue::builtin())
            .await
            .expect("a pool");

        let extension = mixengine_proto::ExtensionId::parse("phpmyadmin").expect("an id");
        an_extension_row(&store, &extension).await;
        crate::extensions::pools::create(&store, &host(), &extension, &version)
            .await
            .expect("the extension's own pool");

        let pools = of_runtime(&store, RuntimeKind::Php, &version)
            .await
            .expect("a lookup");

        assert_eq!(
            pools.iter().map(ServiceId::as_str).collect::<Vec<_>>(),
            ["php-fpm@8.3.33", "php-fpm@phpmyadmin"]
        );
    }

    /// **A runtime holding only an extension's pool still gets the shared one** — roadmap task
    /// **T82a**, D6.
    ///
    /// The predicate used to be "any service on this runtime", which an extension's pool satisfies
    /// while being no use at all to a project site: a home whose shared pool somebody deleted would
    /// never have had it repaired.
    #[tokio::test]
    async fn a_runtime_with_only_an_extensions_pool_still_gets_the_shared_one() {
        let (_home, store) = store().await;
        install(&store, "8.3.33").await;

        let version = PackageVersion::parse("8.3.33").expect("a version");
        let extension = mixengine_proto::ExtensionId::parse("phpmyadmin").expect("an id");
        an_extension_row(&store, &extension).await;
        crate::extensions::pools::create(&store, &host(), &extension, &version)
            .await
            .expect("the extension's own pool");

        let created = ensure(&store, &host(), &Catalogue::builtin())
            .await
            .expect("the shared pool");

        assert_eq!(
            created.iter().map(ServiceId::as_str).collect::<Vec<_>>(),
            ["php-fpm@8.3.33"]
        );
    }

    /// An `extensions` row, which is all
    /// [`extensions::pools::create`](crate::extensions::pools::create) needs one for.
    async fn an_extension_row(store: &Store, id: &mixengine_proto::ExtensionId) {
        let column = id.as_str();

        sqlx::query(
            "INSERT INTO extensions
                 (id, name, version, kind, manifest_json, install_dir, data_dir, source, signed,
                  installed_at)
             VALUES (?1, ?1, '1.0.0', 'web-app', '{}', '/extensions/x', '/data/x', 'registry', 1,
                     '2026-09-03T00:00:00Z')",
        )
        .bind(column)
        .execute(store.pool())
        .await
        .expect("an extensions row");
    }
}
