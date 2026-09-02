//! What the fourth rebuild of `sites` must not lose — roadmap task **T81b**, the design's D2.
//!
//! `0017_extension_sites.sql` rebuilds a table that holds real rows on every developer's machine,
//! that two tables cascade into, and that carries two triggers — and SQLite drops a table's triggers
//! with the table. The row counts are the easy half; the trigger is the half nothing about a row
//! would reveal afterwards, so one test here asserts the *refusal*, not the row.
//!
//! Seeded between two migrations on `migration_extensions.rs`' pattern, on one connection because
//! `PRAGMA foreign_keys` is per-connection.

use sqlx::sqlite::SqliteConnectOptions;
use sqlx::{ConnectOptions as _, SqliteConnection};
use tempfile::TempDir;

/// The version this task adds.
const EXTENSION_SITES: i64 = 17;

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
        .filter(|migration| migration.version < EXTENSION_SITES)
    {
        sqlx::raw_sql(migration.sql.clone())
            .execute(&mut connection)
            .await
            .unwrap_or_else(|error| panic!("migration {}: {error}", migration.version));
    }

    (temp, connection)
}

async fn apply_the_extension_sites_migration(connection: &mut SqliteConnection) {
    let migration = sqlx::migrate!("./migrations")
        .iter()
        .find(|migration| migration.version == EXTENSION_SITES)
        .expect("0017 exists")
        .clone();

    sqlx::raw_sql(migration.sql.clone())
        .execute(connection)
        .await
        .expect("the extension sites migration");
}

/// A package with a service, a project with a shared php-fpm site naming that service, its domains
/// and its link.
async fn seed(connection: &mut SqliteConnection) {
    for statement in [
        "INSERT INTO packages (name, version, install_path, installed_at, source_url, sha256)
         VALUES ('mariadb', '11.4.2', '/packages/mariadb/11.4.2', '2026-08-11T09:00:00Z',
                 'https://example.invalid/mariadb.tar.zst', 'abc')",
        "INSERT INTO services (id, package_id, instance_name, state, port)
         VALUES ('mariadb@main', 1, 'main', 'stopped', 3306)",
        "INSERT INTO projects (name, root_path, created_at)
         VALUES ('blog', '/home/dev/blog', '2026-08-11T09:00:00Z')",
        "INSERT INTO sites (project_id, doc_root, kind, php_service_id, state,
                            shared_interface, shared_address, shared_since, shared_until)
         VALUES (1, 'public', 'php-fpm', 'mariadb@main', 'enabled',
                 'Wi-Fi', '192.168.1.10', 1756900000000, 1756986400000)",
        "INSERT INTO site_domains (site_id, domain, is_primary) VALUES (1, 'blog.test', 1)",
        "INSERT INTO site_domains (site_id, domain, is_primary) VALUES (1, 'www.blog.test', 0)",
        "INSERT INTO site_service_links (site_id, service_id) VALUES (1, 'mariadb@main')",
    ] {
        sqlx::query(statement)
            .execute(&mut *connection)
            .await
            .unwrap_or_else(|error| panic!("{statement}: {error}"));
    }
}

/// Every row, every column that matters, and both children still attached.
#[tokio::test]
async fn the_rebuild_keeps_every_site_with_its_children_and_its_sharing() {
    let (_temp, mut connection) = migrated_to_the_previous_version().await;
    seed(&mut connection).await;

    apply_the_extension_sites_migration(&mut connection).await;

    /// `project_id, extension_id, doc_root, php_service_id, shared_interface, shared_until`.
    type SiteRow = (
        Option<i64>,
        Option<String>,
        String,
        Option<String>,
        Option<String>,
        Option<i64>,
    );

    let row: SiteRow = sqlx::query_as(
        "SELECT project_id, extension_id, doc_root, php_service_id, shared_interface, shared_until
         FROM sites WHERE id = 1",
    )
    .fetch_one(&mut connection)
    .await
    .expect("the site");
    assert_eq!(row.0, Some(1), "the project parent was lost");
    assert_eq!(row.1, None);
    assert_eq!(row.2, "public");
    assert_eq!(row.3.as_deref(), Some("mariadb@main"), "the pool was lost");
    assert_eq!(row.4.as_deref(), Some("Wi-Fi"), "the sharing was lost");
    assert_eq!(row.5, Some(1_756_986_400_000));

    let domains: i64 = sqlx::query_scalar("SELECT count(*) FROM site_domains WHERE site_id = 1")
        .fetch_one(&mut connection)
        .await
        .expect("a count");
    assert_eq!(domains, 2, "the cascade took the domains");

    let links: i64 = sqlx::query_scalar("SELECT count(*) FROM site_service_links")
        .fetch_one(&mut connection)
        .await
        .expect("a count");
    assert_eq!(links, 1, "the cascade took the link");

    let checked: Vec<(String,)> = sqlx::query_as("PRAGMA foreign_key_check")
        .fetch_all(&mut connection)
        .await
        .expect("the check runs");
    assert!(checked.is_empty(), "a foreign key is dangling: {checked:?}");
}

/// **The triggers are back.** A table drop takes its triggers with it, and a missing trigger looks
/// exactly like a present one until a row is written wrong.
#[tokio::test]
async fn the_sharing_trigger_is_recreated() {
    let (_temp, mut connection) = migrated_to_the_previous_version().await;
    seed(&mut connection).await;

    apply_the_extension_sites_migration(&mut connection).await;

    let refused = sqlx::query(
        "UPDATE sites SET shared_interface = NULL, shared_address = '10.0.0.1',
                          shared_since = 1, shared_until = NULL
         WHERE id = 1",
    )
    .execute(&mut connection)
    .await
    .expect_err("an address without an interface is refused");
    assert!(
        refused
            .to_string()
            .contains("a shared site carries an interface"),
        "{refused}"
    );

    let refused = sqlx::query(
        "INSERT INTO sites (project_id, doc_root, kind, state, shared_address)
         VALUES (1, '', 'static', 'enabled', '10.0.0.1')",
    )
    .execute(&mut connection)
    .await
    .expect_err("the insert trigger is back too");
    assert!(
        refused
            .to_string()
            .contains("a shared site carries an interface"),
        "{refused}"
    );
}

/// One owner, exactly: neither parent and both parents are refused; an extension parent is
/// accepted once and refused a second time; and forgetting the extension takes its site.
#[tokio::test]
async fn a_site_has_exactly_one_owner() {
    let (_temp, mut connection) = migrated_to_the_previous_version().await;
    seed(&mut connection).await;

    apply_the_extension_sites_migration(&mut connection).await;

    sqlx::query(
        "INSERT INTO extensions (id, name, version, kind, manifest_json, install_dir, data_dir,
                                 source, signed, installed_at)
         VALUES ('phpmyadmin', 'phpMyAdmin', '5.2.1', 'web-app', '{}', '/ext/phpmyadmin',
                 '/data/extensions/phpmyadmin', 'path', 0, '2026-09-03T00:00:00Z')",
    )
    .execute(&mut connection)
    .await
    .expect("an extension");

    let neither =
        sqlx::query("INSERT INTO sites (doc_root, kind, state) VALUES ('', 'static', 'enabled')")
            .execute(&mut connection)
            .await;
    assert!(neither.is_err(), "a site with no owner was written");

    let both = sqlx::query(
        "INSERT INTO sites (project_id, extension_id, doc_root, kind, state)
         VALUES (1, 'phpmyadmin', '', 'static', 'enabled')",
    )
    .execute(&mut connection)
    .await;
    assert!(both.is_err(), "a site with two owners was written");

    sqlx::query(
        "INSERT INTO sites (extension_id, doc_root, kind, php_service_id, state)
         VALUES ('phpmyadmin', 'app', 'php-fpm', 'mariadb@main', 'enabled')",
    )
    .execute(&mut connection)
    .await
    .expect("an extension-owned site");

    let second = sqlx::query(
        "INSERT INTO sites (extension_id, doc_root, kind, state)
         VALUES ('phpmyadmin', 'other', 'static', 'enabled')",
    )
    .execute(&mut connection)
    .await;
    assert!(second.is_err(), "an extension was given a second site");

    sqlx::query("DELETE FROM extensions WHERE id = 'phpmyadmin'")
        .execute(&mut connection)
        .await
        .expect("the delete");
    let left: i64 = sqlx::query_scalar("SELECT count(*) FROM sites WHERE extension_id IS NOT NULL")
        .fetch_one(&mut connection)
        .await
        .expect("a count");
    assert_eq!(left, 0, "the cascade did not take the extension's site");
}
