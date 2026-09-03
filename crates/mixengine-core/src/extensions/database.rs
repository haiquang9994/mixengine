//! Which database a `web-app` extension is pointed at — roadmap task **T82**, the design's D4.
//!
//! **Resolved the way T81b resolves the PHP**: at install, before anything is fetched, refused by
//! name when nothing satisfies it, and frozen into a row rather than recomputed. The row is
//! `site_service_links`, which T81b left empty for an extension site and named this task to fill.
//!
//! # Writing the link is what arms the refusal
//!
//! Nothing here adds a check to `service.delete`, and the first draft of this design added one
//! before noticing why it should not. [`sites::declaring`](crate::sites::declaring) reads
//! `WHERE s.php_service_id = ? OR l.service_id = ?` — a *link* counts — so deleting the database an
//! extension administers is already refused, naming the site, and crossable only with `--force`.
//! Writing the row buys the refusal, and a second one would be a second thing to keep in step.
//!
//! What `--force` then leaves behind is a `web-app` whose declared engines resolve to nothing.
//! [`config`](super::config) answers that with a skip and a warning rather than by rewriting a
//! configuration to point nowhere: see its own note.
//!
//! # No password here, and no socket
//!
//! [`Endpoint`] carries a host, a port and an account name. It carries no credential, because the
//! design's D6 says a generated file does not hold one — where the password lives stays
//! [`Context::secret_address`](crate::generate::recipe::Context::secret_address)'s answer, in the
//! keyring. It carries no socket, because every database recipe binds a TCP port on all three
//! systems (T34c), so one address form is enough and a placeholder that rendered to nothing on
//! Windows would be a template nobody could write once.

use mixengine_proto::ServiceId;

use super::render::DatabaseEndpoint as Endpoint;
use crate::generate::Catalogue;
use crate::{Error, Result, Store};

/// The instance a machine picks when it has more than one of an engine.
///
/// `mariadb@main` is what `service.create` makes without being told otherwise, so it is what a
/// person means by "my database" — and the alternative, "whichever sorts first", would hand
/// `mariadb@archive` to phpMyAdmin on a machine that also runs `mariadb@main`.
const DEFAULT_INSTANCE: &str = "main";

/// The database an extension declaring `engines` is pointed at here.
///
/// **`engines` is in order of preference; the first engine with a declared service wins; among
/// instances of one engine `main` wins, and failing that the first by id.** Written down because
/// two machines have to answer it the same way, and because the answer is frozen into a row that
/// outlives the reasoning.
///
/// Nothing here starts anything: a stopped MariaDB is still the server phpMyAdmin is pointed at.
///
/// # Errors
///
/// [`Error::ExtensionNoDatabase`] when this machine declares none of them — a refusal *before* the
/// download, for [`Error::RuntimeUnresolved`]'s reason. [`Error::Database`] when the tables cannot
/// be read.
pub async fn resolve(store: &Store, id: &str, engines: &[String]) -> Result<Endpoint> {
    for engine in engines {
        if let Some(service) = first_instance_of(store, engine).await?
            && let Some(endpoint) = endpoint(store, &service).await?
        {
            return Ok(endpoint);
        }
    }

    Err(Error::ExtensionNoDatabase {
        id: id.to_owned(),
        engines: engines.to_vec(),
    })
}

/// Where one service listens and what to connect to it as, or [`None`] for a service that is not a
/// database.
///
/// **Two ways to be [`None`], and both are states rather than faults**: a package whose recipe names
/// no [`administrator`](crate::generate::recipe::Recipe::administrator) is not a database, and a
/// row with no port is a
/// service nothing can dial. A service that is *gone* is [`Error::NotFound`]'s answer and not this
/// one, because the caller wants to tell those apart.
///
/// # Errors
///
/// [`Error::NotFound`] when there is no such row, and [`Error::Database`] when it cannot be read.
pub async fn endpoint(store: &Store, service: &ServiceId) -> Result<Option<Endpoint>> {
    let id = service.as_str();

    let row = sqlx::query!(
        "SELECT p.name AS package, s.port, s.bind_addr
         FROM services s
         JOIN packages p ON p.id = s.package_id
         WHERE s.id = ?",
        id
    )
    .fetch_optional(store.pool())
    .await
    .map_err(|source| store.failure("read", source))?
    .ok_or_else(|| Error::NotFound {
        kind: "service",
        id: id.to_owned(),
    })?;

    let catalogue = Catalogue::builtin();

    let Some(recipe) = catalogue.recipe(&row.package) else {
        return Ok(None);
    };

    let (Some(user), Some(port)) = (recipe.administrator(), row.port) else {
        return Ok(None);
    };

    Ok(Some(Endpoint {
        service: service.clone(),
        host: crate::services::ports::bind_address(Some(row.bind_addr.as_str())),
        // The column is an `INTEGER`; a value outside a port's range is a row nothing wrote.
        port: u16::try_from(port).unwrap_or_default(),
        user: user.to_owned(),
    }))
}

/// The service this machine would use for one engine, preferring the default instance.
async fn first_instance_of(store: &Store, engine: &str) -> Result<Option<ServiceId>> {
    let rows = sqlx::query_scalar!(
        "SELECT s.id FROM services s
         JOIN packages p ON p.id = s.package_id
         WHERE p.name = ?
         ORDER BY s.id",
        engine
    )
    .fetch_all(store.pool())
    .await
    .map_err(|source| store.failure("read", source))?;

    let preferred = format!("{engine}@{DEFAULT_INSTANCE}");

    let chosen = rows
        .iter()
        .find(|id| *id == &preferred)
        .or_else(|| rows.first());

    // An id in `services` that will not parse is a row nothing this build wrote, which is the same
    // answer as no row at all: there is nothing to point an extension at.
    chosen
        .map(|id| {
            ServiceId::parse(id.clone()).map_err(|_| Error::NotFound {
                kind: "service",
                id: id.clone(),
            })
        })
        .transpose()
}

#[cfg(test)]
mod tests {
    use super::*;

    /// **`engines` is in order of preference, and `main` wins among instances.**
    ///
    /// Both halves in one test because they are one rule, and a machine that got the first half
    /// right and the second wrong would hand an administrative interface to whichever server
    /// happened to sort first.
    #[tokio::test]
    async fn the_first_engine_with_a_service_wins_and_main_wins_among_instances() {
        let (_temp, store) = home().await;
        a_database(&store, "mysql@archive", "mysql", 3307).await;
        a_database(&store, "mysql@main", "mysql", 3306).await;

        let chosen = resolve(&store, "pma", &["mariadb".to_owned(), "mysql".to_owned()])
            .await
            .expect("mysql answers when mariadb is not here");

        assert_eq!(chosen.service.as_str(), "mysql@main");
        assert_eq!(chosen.port, 3306);
        assert_eq!(
            chosen.user, "root",
            "the recipe's answer, not the manifest's"
        );
    }

    /// Refused before anything is fetched, naming what would satisfy it — T81b's shape for the PHP,
    /// arriving a second time for the database.
    #[tokio::test]
    async fn a_machine_with_none_of_them_is_a_refusal_that_names_the_engines() {
        let (_temp, store) = home().await;

        let refusal = resolve(&store, "pma", &["mariadb".to_owned(), "mysql".to_owned()])
            .await
            .expect_err("nothing answers");

        let said = refusal.to_string();
        assert!(said.contains("mariadb"), "{said}");
        assert!(said.contains("mysql"), "{said}");
        assert!(said.contains("pma"), "{said}");
    }

    /// A service that is not a database is [`None`] rather than an error: the caller is walking the
    /// engines a manifest named, and "this one is a cache" is an answer.
    #[tokio::test]
    async fn a_service_whose_recipe_names_no_administrator_has_no_endpoint() {
        let (_temp, store) = home().await;
        a_database(&store, "redis@main", "redis", 6379).await;

        let service = ServiceId::parse("redis@main").expect("an id");

        assert!(
            endpoint(&store, &service)
                .await
                .expect("it reads")
                .is_none()
        );
    }

    /// An empty home with the migrations applied.
    async fn home() -> (tempfile::TempDir, Store) {
        let temp = tempfile::tempdir().expect("a temporary home");
        let store = Store::open(&temp.path().join("mixengine.db"))
            .await
            .expect("a store");

        (temp, store)
    }

    /// A `packages` row and the `services` row that runs out of it.
    async fn a_database(store: &Store, service: &str, package: &str, port: i64) {
        let instance = service.split('@').nth(1).unwrap_or(DEFAULT_INSTANCE);

        // One `packages` row per package: two instances of MySQL run out of one installed MySQL,
        // and `UNIQUE (name, version)` is the table saying so.
        let package_id = sqlx::query_scalar!(
            "INSERT INTO packages (name, version, install_path, installed_at, source_url, sha256)
             VALUES (?, '1.0.0', '/packages/x', '2026-09-03T00:00:00Z',
                     'https://example.invalid/x.zip', 'ab')
             ON CONFLICT (name, version) DO UPDATE SET name = excluded.name
             RETURNING id",
            package
        )
        .fetch_one(store.pool())
        .await
        .expect("a package row");

        sqlx::query!(
            "INSERT INTO services (id, package_id, instance_name, state, port, bind_addr)
             VALUES (?, ?, ?, 'stopped', ?, '127.0.0.1')",
            service,
            package_id,
            instance,
            port
        )
        .execute(store.pool())
        .await
        .expect("a service row");
    }
}
