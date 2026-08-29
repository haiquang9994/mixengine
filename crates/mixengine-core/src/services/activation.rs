//! The port an activator listens on, given to the rows that need one — roadmap task **T70**.
//!
//! **A repair rather than a step in `service.create`**, and for [`pools::ensure`](super::pools)'s
//! reason: run at boot as well as after a create, it gives a pool written by an earlier build its
//! activation port with no data migration, and repairs a row somebody cleared by hand. Migration
//! `0009` deliberately backfills nothing — a port chosen while the migration runs is a number
//! decided against a listening table months before anything binds it, which is the one thing
//! [`ports`](super::ports)' allocator exists to refuse.
//!
//! **Which services need one is the catalogue's answer**, not a list here:
//! [`Recipe::activation_port_needed`] says whether a recipe's activator is owed a number on this
//! system, so a service that listens on a socket derives its activator's address instead and takes
//! no port out of circulation.
//!
//! **A row that already has one keeps it.** T34c's rule holds for this column too: an allocated port
//! belongs to its row for as long as the row lives. Here the reason is narrower than a `.env` and no
//! weaker — the number is in a rendered site file, and moving it silently means every idle stop
//! rewrites `etc/` and reloads the front end.

use mixengine_proto::ServiceId;

use crate::generate::Catalogue;
use crate::{Result, Store};

/// Give every service whose recipe needs one an activation port, and say which were given one.
///
/// **A no-op on a home that is already right**, which is what lets it run at boot: the cost of a
/// call with nothing to do is one query.
///
/// # Errors
///
/// [`Error::Database`](crate::Error::Database) when the table cannot be read or written, and
/// whatever [`ports::allocate_activation`](super::ports::allocate_activation) reports — including
/// [`Error::PortsExhausted`](crate::Error::PortsExhausted) when there is no free number above the
/// service's own.
pub async fn ensure(
    store: &Store,
    host: &dyn mixengine_platform::Host,
    catalogue: &Catalogue,
) -> Result<Vec<ServiceId>> {
    // The whole search is one pass over a handful of rows, and the lock below is held per service
    // rather than across the read: two daemons repairing one home is not a failure of either.
    let rows = sqlx::query!(
        r#"SELECT s.id           AS "id!: String",
                  s.port         AS "port: i64",
                  s.bind_addr    AS "bind_addr: String",
                  p.name         AS "package: String"
           FROM services s
           LEFT JOIN packages p ON p.id = s.package_id
           WHERE s.activation_port IS NULL AND s.port IS NOT NULL"#
    )
    .fetch_all(store.pool())
    .await
    .map_err(|source| store.failure("read", source))?;

    let mut given = Vec::new();

    for row in rows {
        let Ok(service) = ServiceId::parse(&row.id) else {
            // A row somebody wrote by hand. Said rather than repaired against a name this build
            // cannot make sense of.
            tracing::warn!(
                service = row.id,
                "a services row carries an id this build cannot read"
            );
            continue;
        };

        // A pool's row names a runtime rather than a package, so the recipe is found by the id's own
        // half — the same lookup `pools` makes, and the reason a pool is `php-fpm@8.3.33`.
        let package = row.package.unwrap_or_else(|| service.name().to_owned());

        let Some(recipe) = catalogue.recipe(&package) else {
            continue;
        };

        if !recipe.activation_port_needed() {
            continue;
        }

        let Ok(port) = u16::try_from(row.port.unwrap_or_default()) else {
            tracing::warn!(
                service = row.id,
                "a services row carries a port this build cannot read, so its activator gets none"
            );
            continue;
        };

        // Held until the row is written, exactly as a create holds it: two allocations reading the
        // same table before either writes are two services handed one address.
        let _in_flight = super::ports::hold().await;

        let activation = super::ports::allocate_activation(
            store,
            host,
            super::ports::bind_address(Some(row.bind_addr.as_str())),
            port,
        )
        .await?;

        let written = i64::from(activation);

        sqlx::query!(
            "UPDATE services SET activation_port = ?
             WHERE id = ? AND activation_port IS NULL",
            written,
            row.id
        )
        .execute(store.pool())
        .await
        .map_err(|source| store.failure("write", source))?;

        given.push(service);
    }

    Ok(given)
}

#[cfg(test)]
mod tests {
    use std::net::{Ipv4Addr, TcpListener};
    use std::sync::Arc;

    use mixengine_platform::mock;

    use super::*;
    use crate::generate::recipe::Recipe;
    use crate::generate::{Catalogue, PhpFpm};

    /// The home the mock host is given. Nothing here touches it.
    const HOME: &str = "/mixengine";

    async fn store() -> (tempfile::TempDir, Store) {
        let home = tempfile::tempdir().expect("a temporary directory");
        let store = Store::open(&home.path().join(crate::paths::DATABASE_FILE_NAME))
            .await
            .expect("a database");
        (home, store)
    }

    /// A port this machine is free to give, out of the band `ports`' own tests keep to — below every
    /// ephemeral floor, so nothing the OS hands round can be underneath it.
    fn a_free_port() -> u16 {
        (23_000..23_900)
            .find(|port| TcpListener::bind((Ipv4Addr::LOCALHOST, *port)).is_ok())
            .expect("a free port in the window")
    }

    /// A pool row, as `pools::ensure` would have left it on a system where a pool listens on TCP.
    async fn pool(store: &Store, version: &str, port: u16) {
        sqlx::query(
            "INSERT INTO runtime_installs
                 (kind, version, channel, install_path, installed_at, size_bytes, source_url, sha256)
             VALUES ('php', ?, 'release', '/runtimes/php', '2026-08-29T00:00:00Z', 0,
                     'https://example.invalid/php', 'abc')",
        )
        .bind(version)
        .execute(store.pool())
        .await
        .expect("a runtime");

        sqlx::query(
            "INSERT INTO services (id, runtime_install_id, instance_name, state, port)
             SELECT ?, id, ?, 'stopped', ? FROM runtime_installs WHERE version = ?",
        )
        .bind(format!("php-fpm@{version}"))
        .bind(version)
        .bind(i64::from(port))
        .bind(version)
        .execute(store.pool())
        .await
        .expect("a pool");
    }

    fn catalogue() -> Catalogue {
        Catalogue::default().with(Arc::new(PhpFpm))
    }

    async fn activation_port(store: &Store, id: &str) -> Option<i64> {
        sqlx::query_scalar::<_, Option<i64>>("SELECT activation_port FROM services WHERE id = ?")
            .bind(id)
            .fetch_one(store.pool())
            .await
            .expect("the row")
    }

    /// **A pool that can be woken is given a second number, and never its own.**
    ///
    /// The whole reason the column exists rather than `port + 1` being computed where it is needed.
    #[tokio::test]
    async fn a_pool_is_given_an_activation_port_that_is_not_its_own() {
        let (_home, store) = store().await;
        let port = a_free_port();
        pool(&store, "8.3.33", port).await;

        let given = ensure(&store, &mock::Host::with_home(HOME), &catalogue())
            .await
            .expect("a repair");

        let activation = activation_port(&store, "php-fpm@8.3.33").await;

        if PhpFpm.activation_port_needed() {
            assert_eq!(given.len(), 1, "the one pool that needed a port");
            assert!(activation.is_some(), "a pool on TCP is owed a number");
            assert_ne!(
                activation,
                Some(i64::from(port)),
                "an activator on the pool's own address has nothing to fall back to"
            );
        } else {
            assert!(
                given.is_empty(),
                "a pool that listens on a socket derives its activator and takes no port"
            );
            assert_eq!(activation, None);
        }
    }

    /// **Running twice allocates nothing the second time** — the property that lets this run at
    /// boot. A number that moved would be a rendered site file that moved, and every idle stop
    /// would reload the front end.
    #[tokio::test]
    async fn a_row_that_already_has_one_keeps_it() {
        let (_home, store) = store().await;
        pool(&store, "8.3.33", a_free_port()).await;

        let host = mock::Host::with_home(HOME);

        ensure(&store, &host, &catalogue())
            .await
            .expect("the first");
        let first = activation_port(&store, "php-fpm@8.3.33").await;

        let again = ensure(&store, &host, &catalogue())
            .await
            .expect("the second");

        assert!(again.is_empty(), "the second call had nothing to do");
        assert_eq!(
            activation_port(&store, "php-fpm@8.3.33").await,
            first,
            "an allocated port belongs to its row for as long as the row lives"
        );
    }
}
