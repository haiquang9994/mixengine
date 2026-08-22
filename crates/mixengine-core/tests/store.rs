//! The database a real `MIXENGINE_HOME` gets: the shipped migrations, against the shipped
//! connection settings.
//!
//! The unit tests in `store.rs` prove the machinery around a migration — when a backup is taken,
//! which failures are the user's and which are ours — using migrations written on the spot. These
//! prove the schema itself: the constraints that decide whether two sites can claim one domain are
//! only worth anything if they are the ones that actually reach a user's disk.

use mixengine_core::{Store, open_home};
use mixengine_platform::mock;
use sqlx::{Row as _, SqlitePool};
use tempfile::TempDir;

/// An opened home with its database, both in a directory the test owns.
async fn store() -> (TempDir, Store) {
    let temp = TempDir::new().expect("a temporary directory");
    let home = open_home(None, &mock::Host::with_home(temp.path().join("MixEngine")))
        .expect("a home to put the database in");

    let store = Store::open(home.paths.database_file())
        .await
        .expect("a database");
    (temp, store)
}

/// Every package a service can point at has to exist first — the schema says so, which is the
/// point of `a_service_cannot_name_a_package_that_is_not_installed`.
async fn insert_package(pool: &SqlitePool) -> i64 {
    sqlx::query_scalar(
        "INSERT INTO packages (name, version, install_path, installed_at, source_url, sha256)
         VALUES ('mariadb', '11.4.2', '/packages/mariadb/11.4.2', '2026-08-11T09:00:00Z',
                 'https://example.invalid/mariadb.tar.zst', 'abc')
         RETURNING id",
    )
    .fetch_one(pool)
    .await
    .expect("an installed package")
}

/// A site under `project`, claiming `domain` as its primary — the two writes that creating a site
/// is, since the domain lives in `site_domains` and nowhere else.
async fn insert_site(
    pool: &SqlitePool,
    project: i64,
    domain: &str,
    doc_root: &str,
) -> Result<(), sqlx::Error> {
    let site: i64 = sqlx::query_scalar(
        "INSERT INTO sites (project_id, doc_root, kind, state)
         VALUES (?, ?, 'php-fpm', 'enabled') RETURNING id",
    )
    .bind(project)
    .bind(doc_root)
    .fetch_one(pool)
    .await?;

    insert_domain(pool, site, domain, 1).await
}

/// One more domain for a site, primary or not.
async fn insert_domain(
    pool: &SqlitePool,
    site: i64,
    domain: &str,
    is_primary: i64,
) -> Result<(), sqlx::Error> {
    sqlx::query("INSERT INTO site_domains (site_id, domain, is_primary) VALUES (?, ?, ?)")
        .bind(site)
        .bind(domain)
        .bind(is_primary)
        .execute(pool)
        .await
        .map(|_| ())
}

/// A project to hang sites off.
async fn insert_project(pool: &SqlitePool, name: &str) -> i64 {
    sqlx::query_scalar(
        "INSERT INTO projects (name, root_path, created_at)
         VALUES (?, ?, '2026-08-11T09:00:00Z') RETURNING id",
    )
    .bind(name)
    .bind(format!("/home/dev/{name}"))
    .fetch_one(pool)
    .await
    .expect("a project")
}

/// An installed PHP, default or not.
async fn insert_php(pool: &SqlitePool, version: &str, is_default: i64) -> Result<(), sqlx::Error> {
    sqlx::query(
        "INSERT INTO runtime_installs
             (kind, version, channel, install_path, installed_at, size_bytes, source_url, sha256,
              is_default)
         VALUES ('php', ?, 'stable', ?, '2026-08-11T09:00:00Z', 1, 'https://x.invalid', 'a', ?)",
    )
    .bind(version)
    .bind(format!("/runtimes/php/{version}"))
    .bind(is_default)
    .execute(pool)
    .await
    .map(|_| ())
}

#[tokio::test]
async fn a_first_run_gets_the_documented_schema() {
    let (_temp, store) = store().await;

    let tables: Vec<String> = sqlx::query_scalar(
        "SELECT name FROM sqlite_master
         WHERE type = 'table' AND name NOT LIKE 'sqlite_%' AND name != '_sqlx_migrations'
         ORDER BY name",
    )
    .fetch_all(store.pool())
    .await
    .unwrap();

    // The list in .claude/architecture/data-model.md. Both halves are the contract: a table added
    // here without a line there is a schema nobody agreed to.
    assert_eq!(
        tables,
        [
            "blueprints",
            "ca",
            "certificates",
            "events",
            "extensions",
            "jobs",
            "packages",
            "projects",
            "runtime_installs",
            "services",
            "settings",
            "site_domains",
            "site_service_links",
            "sites",
        ]
    );
}

#[tokio::test]
async fn the_database_lands_where_paths_says_it_does() {
    let temp = TempDir::new().unwrap();
    let home = open_home(None, &mock::Host::with_home(temp.path().join("MixEngine"))).unwrap();

    let store = Store::open(home.paths.database_file()).await.unwrap();

    assert_eq!(store.file(), home.paths.database_file());
    assert!(home.paths.database_file().is_file());
    // The home is private (T3a) and the database is inside it, so the file needs no permissions of
    // its own — but a `[paths]` override must never be able to move it out from under that.
    assert_eq!(home.paths.database_file().parent(), Some(home.paths.root()));
}

#[tokio::test]
async fn the_journal_is_write_ahead_and_stays_that_way() {
    let (_temp, store) = store().await;

    let mode: String = sqlx::query_scalar("PRAGMA journal_mode")
        .fetch_one(store.pool())
        .await
        .unwrap();

    assert_eq!(mode, "wal");
}

#[tokio::test]
async fn foreign_keys_are_enforced_on_every_connection() {
    let (_temp, store) = store().await;

    // The pragma is per connection, not per database, so checking it once proves nothing about the
    // pool. Hold them all at the same time and ask each. The count comes from the pool rather than
    // from a literal: asking for one more than exists does not fail here, it blocks until the
    // acquire timeout and then reports something that reads nothing like "the pool shrank".
    let mut connections = Vec::new();
    for _ in 0..store.pool().options().get_max_connections() {
        connections.push(store.pool().acquire().await.unwrap());
    }
    assert!(
        !connections.is_empty(),
        "a pool of no connections proves nothing"
    );

    for connection in &mut connections {
        let enabled: i64 = sqlx::query_scalar("PRAGMA foreign_keys")
            .fetch_one(&mut **connection)
            .await
            .unwrap();
        assert_eq!(enabled, 1);
    }
}

#[tokio::test]
async fn a_service_cannot_name_a_package_that_is_not_installed() {
    let (_temp, store) = store().await;

    let orphan = sqlx::query(
        "INSERT INTO services (id, package_id, instance_name, state) VALUES (?, ?, ?, ?)",
    )
    .bind("mariadb@main")
    .bind(9999_i64)
    .bind("main")
    .bind("stopped")
    .execute(store.pool())
    .await;

    assert!(orphan.is_err(), "there is no package 9999");

    let package = insert_package(store.pool()).await;
    sqlx::query("INSERT INTO services (id, package_id, instance_name, state) VALUES (?, ?, ?, ?)")
        .bind("mariadb@main")
        .bind(package)
        .bind("main")
        .bind("stopped")
        .execute(store.pool())
        .await
        .expect("the same row, once the package it names exists");
}

#[tokio::test]
async fn a_column_typed_integer_never_ends_up_holding_text() {
    let (_temp, store) = store().await;
    let package = insert_package(store.pool()).await;

    // What STRICT is for. Without it SQLite keeps whatever it is handed, and the next
    // `WHERE port < 1024` compares a string against a number and quietly disagrees with
    // arithmetic. With it, text that *is* an integer is converted...
    sqlx::query(
        "INSERT INTO services (id, package_id, instance_name, state, port)
         VALUES ('mariadb@main', ?, 'main', 'stopped', '3306')",
    )
    .bind(package)
    .execute(store.pool())
    .await
    .expect("'3306' is losslessly an integer");

    let stored: String = sqlx::query_scalar("SELECT typeof(port) FROM services WHERE id = ?")
        .bind("mariadb@main")
        .fetch_one(store.pool())
        .await
        .unwrap();
    assert_eq!(stored, "integer");

    // ...and text that is not is refused, which is the half that matters.
    let wrong = sqlx::query(
        "INSERT INTO services (id, package_id, instance_name, state, port)
         VALUES ('mariadb@wrong', ?, 'wrong', 'stopped', 'default')",
    )
    .bind(package)
    .execute(store.pool())
    .await;

    assert!(wrong.is_err(), "a port is not a word");
}

#[tokio::test]
async fn two_sites_cannot_claim_one_domain() {
    let (_temp, store) = store().await;
    let project = insert_project(store.pool(), "blog").await;

    insert_site(store.pool(), project, "blog.test", "/home/dev/blog/public")
        .await
        .expect("the first site");
    assert!(
        insert_site(store.pool(), project, "blog.test", "/home/dev/other/public")
            .await
            .is_err(),
        "the web server would answer with whichever import it read last"
    );
}

#[tokio::test]
async fn an_alias_cannot_take_a_domain_another_site_is_primary_on() {
    let (_temp, store) = store().await;
    let project = insert_project(store.pool(), "blog").await;

    insert_site(store.pool(), project, "blog.test", "/home/dev/blog/public")
        .await
        .expect("the site that owns blog.test");
    insert_site(store.pool(), project, "shop.test", "/home/dev/shop/public")
        .await
        .expect("an unrelated site");

    let shop: i64 =
        sqlx::query_scalar("SELECT site_id FROM site_domains WHERE domain = 'shop.test'")
            .fetch_one(store.pool())
            .await
            .unwrap();

    // The half a `sites.primary_domain` column could not express: with the domains split across two
    // tables, each unique index would be satisfied and blog.test would belong to both sites.
    assert!(
        insert_domain(store.pool(), shop, "blog.test", 0)
            .await
            .is_err(),
        "an alias is a claim on a domain like any other"
    );
}

#[tokio::test]
async fn a_site_has_at_most_one_primary_domain() {
    let (_temp, store) = store().await;
    let project = insert_project(store.pool(), "blog").await;

    insert_site(store.pool(), project, "blog.test", "/home/dev/blog/public")
        .await
        .expect("the site and its primary");

    let site: i64 =
        sqlx::query_scalar("SELECT site_id FROM site_domains WHERE domain = 'blog.test'")
            .fetch_one(store.pool())
            .await
            .unwrap();

    insert_domain(store.pool(), site, "www.blog.test", 0)
        .await
        .expect("any number of aliases");
    assert!(
        insert_domain(store.pool(), site, "old.blog.test", 1)
            .await
            .is_err(),
        "two primaries is the bug that makes a redirect target depend on row order"
    );
}

#[tokio::test]
async fn a_kind_has_one_default_runtime_and_any_number_of_others() {
    let (_temp, store) = store().await;

    insert_php(store.pool(), "8.3.12", 1)
        .await
        .expect("the default PHP");
    insert_php(store.pool(), "8.2.20", 0)
        .await
        .expect("a second PHP, not default");
    insert_php(store.pool(), "8.1.29", 0)
        .await
        .expect("a third, also not default");

    assert!(
        insert_php(store.pool(), "8.4.0", 1).await.is_err(),
        "two defaults for one kind is the bug that makes `php -v` depend on row order"
    );
}

#[tokio::test]
async fn a_second_open_finds_it_current_and_leaves_no_backup() {
    let temp = TempDir::new().unwrap();
    let home = open_home(None, &mock::Host::with_home(temp.path().join("MixEngine"))).unwrap();

    let first = Store::open(home.paths.database_file()).await.unwrap();
    sqlx::query("INSERT INTO settings (key, value_json) VALUES ('greeting', '\"hello\"')")
        .execute(first.pool())
        .await
        .unwrap();
    first.close().await;

    let second = Store::open(home.paths.database_file()).await.unwrap();

    let value: String = sqlx::query("SELECT value_json FROM settings WHERE key = 'greeting'")
        .fetch_one(second.pool())
        .await
        .unwrap()
        .get(0);
    assert_eq!(value, "\"hello\"");

    // The daemon starts many times a day; a backup per start would fill the home with copies of a
    // database nothing migrated.
    let copies: Vec<_> = std::fs::read_dir(home.paths.root())
        .unwrap()
        .filter_map(Result::ok)
        .map(|entry| entry.file_name().to_string_lossy().into_owned())
        .filter(|name| name.contains(".bak-"))
        .collect();
    assert!(copies.is_empty(), "{copies:?}");
}

#[tokio::test]
async fn a_database_that_cannot_be_created_names_the_path() {
    let temp = TempDir::new().unwrap();
    // A directory that does not exist — a `[paths]` root on a disk nobody mounted, in miniature.
    let file = temp.path().join("nowhere").join("mixengine.db");

    let error = Store::open(&file).await.expect_err("there is no directory");

    assert!(
        matches!(&error, mixengine_core::Error::Database { path, .. } if path == &file),
        "{error:?}"
    );
}

/// The three columns T28 adds are additive, and a row written before them says so rather than
/// failing to be read.
#[tokio::test]
async fn a_runtime_installed_before_extensions_existed_offers_none() {
    let (_temp, store) = store().await;

    sqlx::query(
        "INSERT INTO runtime_installs
             (kind, version, channel, install_path, installed_at, size_bytes, source_url, sha256,
              is_default)
         VALUES ('php', '7.4.33', 'stable', '/runtimes/php/7.4.33', '2026-08-11T09:00:00Z',
                 1, 'https://example.invalid/php.tar.zst', 'abc', 1)",
    )
    .execute(store.pool())
    .await
    .expect("a row from a build that had never heard of extensions");

    let row = sqlx::query(
        "SELECT extension_dir, extensions_json, extension_choices_json
         FROM runtime_installs WHERE version = '7.4.33'",
    )
    .fetch_one(store.pool())
    .await
    .expect("the row");

    assert_eq!(row.get::<String, _>("extension_dir"), "");
    assert_eq!(row.get::<String, _>("extensions_json"), "{}");
    assert_eq!(row.get::<String, _>("extension_choices_json"), "{}");
}

/// Every index the three site tables carry, by name. A rebuild that hand-copies the tables and
/// forgets one is a table scan on every delete, and nothing else would notice.
#[tokio::test]
async fn the_site_tables_keep_every_index_they_were_given() {
    let (_temp, store) = store().await;

    let indexes: Vec<String> = sqlx::query_scalar(
        "SELECT name FROM sqlite_master
         WHERE type = 'index' AND tbl_name IN ('sites', 'site_domains', 'site_service_links')
           AND name NOT LIKE 'sqlite_%'
         ORDER BY name",
    )
    .fetch_all(store.pool())
    .await
    .unwrap();

    assert_eq!(
        indexes,
        [
            "site_domains_domain",
            "site_domains_one_primary_per_site",
            "site_domains_site",
            "site_service_links_service",
            "sites_project",
        ]
    );
}

/// Deleting a project takes its sites and their domains with it. The path is
/// `projects` -> `sites` -> `site_domains`, and the second hop is the one a rebuild can lose.
#[tokio::test]
async fn forgetting_a_project_takes_its_sites_and_their_domains() {
    let (_temp, store) = store().await;
    let project = insert_project(store.pool(), "blog").await;

    insert_site(store.pool(), project, "blog.test", "public")
        .await
        .expect("a site");
    let site: i64 =
        sqlx::query_scalar("SELECT site_id FROM site_domains WHERE domain = 'blog.test'")
            .fetch_one(store.pool())
            .await
            .unwrap();
    insert_domain(store.pool(), site, "www.blog.test", 0)
        .await
        .expect("an alias");

    sqlx::query("DELETE FROM projects WHERE id = ?")
        .bind(project)
        .execute(store.pool())
        .await
        .expect("the project goes");

    let sites: i64 = sqlx::query_scalar("SELECT count(*) FROM sites")
        .fetch_one(store.pool())
        .await
        .unwrap();
    let domains: i64 = sqlx::query_scalar("SELECT count(*) FROM site_domains")
        .fetch_one(store.pool())
        .await
        .unwrap();

    assert_eq!(
        (sites, domains),
        (0, 0),
        "a domain nothing owns is a domain no site can ever claim again"
    );
}

/// Deleting a service takes its links and leaves the sites. The other cascade, and the reason
/// `sites.php_service_id` is `ON DELETE SET NULL` rather than a third one.
#[tokio::test]
async fn deleting_a_service_unlinks_it_and_leaves_the_site_standing() {
    let (_temp, store) = store().await;
    let project = insert_project(store.pool(), "blog").await;
    let package = insert_package(store.pool()).await;

    sqlx::query("INSERT INTO services (id, package_id, instance_name, state) VALUES (?, ?, ?, ?)")
        .bind("mariadb@main")
        .bind(package)
        .bind("main")
        .bind("stopped")
        .execute(store.pool())
        .await
        .expect("a service");

    insert_site(store.pool(), project, "blog.test", "public")
        .await
        .expect("a site");
    let site: i64 =
        sqlx::query_scalar("SELECT site_id FROM site_domains WHERE domain = 'blog.test'")
            .fetch_one(store.pool())
            .await
            .unwrap();

    sqlx::query("INSERT INTO site_service_links (site_id, service_id) VALUES (?, 'mariadb@main')")
        .bind(site)
        .execute(store.pool())
        .await
        .expect("a link");
    sqlx::query("UPDATE sites SET php_service_id = 'mariadb@main' WHERE id = ?")
        .bind(site)
        .execute(store.pool())
        .await
        .expect("a pool, for the SET NULL half");

    sqlx::query("DELETE FROM services WHERE id = 'mariadb@main'")
        .execute(store.pool())
        .await
        .expect("the service goes");

    let links: i64 = sqlx::query_scalar("SELECT count(*) FROM site_service_links")
        .fetch_one(store.pool())
        .await
        .unwrap();
    let pool: Option<String> = sqlx::query_scalar("SELECT php_service_id FROM sites WHERE id = ?")
        .bind(site)
        .fetch_one(store.pool())
        .await
        .unwrap();
    let sites: i64 = sqlx::query_scalar("SELECT count(*) FROM sites")
        .fetch_one(store.pool())
        .await
        .unwrap();

    assert_eq!(links, 0, "the link goes with the service");
    assert_eq!(
        pool, None,
        "and the pool becomes a site that names none — spec D3"
    );
    assert_eq!(
        sites, 1,
        "the site itself is not a dependent of its database"
    );
}

/// The CHECK `0001_initial.sql` deferred. A site is a server block that is there or is not; the
/// seven states beside it belong to the services it uses.
#[tokio::test]
async fn a_site_is_enabled_or_disabled_and_nothing_else() {
    let (_temp, store) = store().await;
    let project = insert_project(store.pool(), "blog").await;

    let refused = sqlx::query(
        "INSERT INTO sites (project_id, doc_root, kind, state)
         VALUES (?, 'public', 'php-fpm', 'running')",
    )
    .bind(project)
    .execute(store.pool())
    .await;
    assert!(
        refused.is_err(),
        "a site does not run; the php-fpm pool under it does"
    );

    // And the default is the state a site is created in, so no writer has to name it.
    sqlx::query("INSERT INTO sites (project_id, doc_root, kind) VALUES (?, 'public', 'static')")
        .bind(project)
        .execute(store.pool())
        .await
        .expect("a site with no state named");

    let state: String = sqlx::query_scalar("SELECT state FROM sites WHERE kind = 'static'")
        .fetch_one(store.pool())
        .await
        .unwrap();
    assert_eq!(state, "enabled");
}
