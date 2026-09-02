//! What the rebuild of `services` must not lose — roadmap task **T81**, the design's D6.
//!
//! `0016_extensions.sql` is the first migration in this tree that rebuilds a table holding real
//! rows. Two tables point at `services` and each would be damaged differently by a drop with
//! foreign keys enforced: `sites.php_service_id` is `ON DELETE SET NULL`, so every site would
//! quietly lose the pool it names, and `site_service_links.service_id` is `ON DELETE CASCADE`, so
//! every "this site needs that database" row would be deleted outright — the worse of the two, and
//! the one nothing about a site's own row would reveal afterwards.
//!
//! **These tests seed between two migrations**, which is why they apply them by hand rather than
//! through [`Store::open`]: the interesting question is what happens to rows that were already
//! there, and a database migrated in one go has none. Everything runs on a single connection
//! because `PRAGMA foreign_keys` is per-connection, and the migration's own pragma is worth nothing
//! if the statement after it lands somewhere else.

use sqlx::sqlite::SqliteConnectOptions;
use sqlx::{ConnectOptions as _, SqliteConnection};
use tempfile::TempDir;

/// The version this task adds, and the line the seeding happens on.
const EXTENSIONS: i64 = 16;

/// A connection to a fresh database, migrated up to but not including [`EXTENSIONS`].
///
/// Foreign keys are enforced exactly as [`mixengine_core::Store`] enforces them, because a
/// migration that only works with them off is one that works nowhere real.
async fn migrated_to_the_previous_version() -> (TempDir, SqliteConnection) {
    let temp = TempDir::new().expect("a temporary directory");

    let mut connection = SqliteConnectOptions::new()
        .filename(temp.path().join("mixengine.db"))
        .create_if_missing(true)
        .foreign_keys(true)
        .connect()
        .await
        .expect("a database");

    for migration in sqlx::migrate!("./migrations")
        .iter()
        .filter(|migration| migration.version < EXTENSIONS)
    {
        sqlx::raw_sql(migration.sql.clone())
            .execute(&mut connection)
            .await
            .unwrap_or_else(|error| panic!("migration {}: {error}", migration.version));
    }

    (temp, connection)
}

/// Apply the migration under test.
async fn apply_the_extensions_migration(connection: &mut SqliteConnection) {
    let migration = sqlx::migrate!("./migrations")
        .iter()
        .find(|migration| migration.version == EXTENSIONS)
        .expect("0016 exists")
        .clone();

    sqlx::raw_sql(migration.sql.clone())
        .execute(connection)
        .await
        .expect("the extensions migration");
}

/// A pool, a site that names it, and a link between a site and a service.
async fn seed(connection: &mut SqliteConnection) {
    sqlx::query(
        "INSERT INTO packages (name, version, install_path, installed_at, source_url, sha256)
         VALUES ('mariadb', '11.4.2', '/packages/mariadb/11.4.2', '2026-08-11T09:00:00Z',
                 'https://example.invalid/mariadb.tar.zst', 'abc')",
    )
    .execute(&mut *connection)
    .await
    .expect("a package");

    sqlx::query(
        "INSERT INTO services (id, package_id, instance_name, state, port)
         VALUES ('mariadb@main', 1, 'main', 'stopped', 3306)",
    )
    .execute(&mut *connection)
    .await
    .expect("a service");

    sqlx::query(
        "INSERT INTO projects (name, root_path, created_at)
         VALUES ('blog', '/home/dev/blog', '2026-08-11T09:00:00Z')",
    )
    .execute(&mut *connection)
    .await
    .expect("a project");

    sqlx::query(
        "INSERT INTO sites (project_id, doc_root, kind, php_service_id, state)
         VALUES (1, '/home/dev/blog/public', 'php-fpm', 'mariadb@main', 'enabled')",
    )
    .execute(&mut *connection)
    .await
    .expect("a site");

    sqlx::query("INSERT INTO site_service_links (site_id, service_id) VALUES (1, 'mariadb@main')")
        .execute(&mut *connection)
        .await
        .expect("a link");
}

/// **Everything pointing at `services` still points at it afterwards.**
///
/// The link row is the one worth the test: a cascade would take it with no trace left anywhere a
/// person would look.
#[tokio::test]
async fn the_rebuild_keeps_what_points_at_services() {
    let (_temp, mut connection) = migrated_to_the_previous_version().await;
    seed(&mut connection).await;

    apply_the_extensions_migration(&mut connection).await;

    let services: i64 = sqlx::query_scalar("SELECT count(*) FROM services")
        .fetch_one(&mut connection)
        .await
        .expect("a count");
    assert_eq!(services, 1, "the copy lost a row");

    let pool: Option<String> = sqlx::query_scalar("SELECT php_service_id FROM sites WHERE id = 1")
        .fetch_one(&mut connection)
        .await
        .expect("the site");
    assert_eq!(
        pool.as_deref(),
        Some("mariadb@main"),
        "the site lost the service it names"
    );

    let links: i64 = sqlx::query_scalar("SELECT count(*) FROM site_service_links")
        .fetch_one(&mut connection)
        .await
        .expect("a count");
    assert_eq!(
        links, 1,
        "a cascade took the link with nothing to show for it"
    );
}

/// Every column a row held is still there, with the value it held.
///
/// A rebuild copies a column list written by hand, and the failure mode is a column left out of
/// it — which no constraint reports and no `SELECT *` in a test notices unless it looks.
#[tokio::test]
async fn the_rebuild_keeps_every_column() {
    let (_temp, mut connection) = migrated_to_the_previous_version().await;
    seed(&mut connection).await;

    sqlx::query(
        "UPDATE services
            SET autostart = 1, activation_port = 9001, idle_stopped = 1, idle_minutes = 30,
                bind_addr = '0.0.0.0', data_dir = '/data/mariadb/main',
                config_overrides_json = '{\"port\":3306}', limits_json = '{\"memory_mb\":512}',
                last_started_at = 1234, last_exit_code = 0, pid = 42, pid_start_time = 99
          WHERE id = 'mariadb@main'",
    )
    .execute(&mut connection)
    .await
    .expect("a fully populated row");

    apply_the_extensions_migration(&mut connection).await;

    let row = sqlx::query_as::<
        _,
        (
            i64,
            i64,
            i64,
            i64,
            String,
            String,
            String,
            String,
            i64,
            i64,
            i64,
            i64,
        ),
    >(
        "SELECT autostart, activation_port, idle_stopped, idle_minutes, bind_addr, data_dir,
                config_overrides_json, limits_json, last_started_at, last_exit_code, pid,
                pid_start_time
           FROM services WHERE id = 'mariadb@main'",
    )
    .fetch_one(&mut connection)
    .await
    .expect("the row, with every column it had");

    assert_eq!(
        row,
        (
            1,
            9001,
            1,
            30,
            "0.0.0.0".to_owned(),
            "/data/mariadb/main".to_owned(),
            "{\"port\":3306}".to_owned(),
            "{\"memory_mb\":512}".to_owned(),
            1234,
            0,
            42,
            99
        )
    );
}

/// **The third origin exists, and a row may have exactly one parent.**
#[tokio::test]
async fn a_service_may_belong_to_an_extension_and_to_nothing_else() {
    let (_temp, mut connection) = migrated_to_the_previous_version().await;
    seed(&mut connection).await;
    apply_the_extensions_migration(&mut connection).await;

    sqlx::query(
        "INSERT INTO extensions
           (id, name, version, kind, manifest_json, install_dir, data_dir, source, signed,
            installed_at)
         VALUES ('mailpit', 'Mailpit', '1.20.0', 'service', '{}', '/x/extensions/mailpit',
                 '/x/data/extensions/mailpit', 'registry', 1, '2026-09-02T09:00:00Z')",
    )
    .execute(&mut connection)
    .await
    .expect("an extension");

    sqlx::query(
        "INSERT INTO services (id, extension_id, instance_name, state)
         VALUES ('mailpit@default', 'mailpit', 'default', 'stopped')",
    )
    .execute(&mut connection)
    .await
    .expect("a service belonging to an extension");

    let two_parents = sqlx::query(
        "INSERT INTO services (id, extension_id, package_id, instance_name, state)
         VALUES ('mailpit@second', 'mailpit', 1, 'second', 'stopped')",
    )
    .execute(&mut connection)
    .await;
    assert!(two_parents.is_err(), "two parents at once must be refused");

    let no_parent = sqlx::query(
        "INSERT INTO services (id, instance_name, state)
         VALUES ('orphan@default', 'default', 'stopped')",
    )
    .execute(&mut connection)
    .await;
    assert!(
        no_parent.is_err(),
        "a service with no parent must be refused"
    );
}

/// An extension holding a port cannot be removed while a service still names it, and its ports go
/// when it does.
#[tokio::test]
async fn ports_belong_to_the_extension_that_holds_them() {
    let (_temp, mut connection) = migrated_to_the_previous_version().await;
    apply_the_extensions_migration(&mut connection).await;

    sqlx::query(
        "INSERT INTO extensions
           (id, name, version, kind, manifest_json, install_dir, data_dir, source, signed,
            installed_at)
         VALUES ('mailpit', 'Mailpit', '1.20.0', 'service', '{}', '/x/extensions/mailpit',
                 '/x/data/extensions/mailpit', 'path', 0, '2026-09-02T09:00:00Z')",
    )
    .execute(&mut connection)
    .await
    .expect("an extension");

    sqlx::query(
        "INSERT INTO extension_ports (extension_id, name, port)
         VALUES ('mailpit', 'ui_port', 8025), ('mailpit', 'smtp_port', 1025)",
    )
    .execute(&mut connection)
    .await
    .expect("two ports");

    let clash = sqlx::query(
        "INSERT INTO extension_ports (extension_id, name, port) VALUES ('mailpit', 'other', 8025)",
    )
    .execute(&mut connection)
    .await;
    assert!(clash.is_err(), "two names must not hold one port");

    sqlx::query("DELETE FROM extensions WHERE id = 'mailpit'")
        .execute(&mut connection)
        .await
        .expect("the extension goes");

    let held: i64 = sqlx::query_scalar("SELECT count(*) FROM extension_ports")
        .fetch_one(&mut connection)
        .await
        .expect("a count");
    assert_eq!(held, 0, "a port outlived the extension that held it");
}
