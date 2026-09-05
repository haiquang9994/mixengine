//! An old `mixengine.db`, migrated by this build — roadmap task **T89**.
//!
//! Every other migration test in this workspace builds "the old database" out of today's migration
//! files: the unit tests in `store.rs` write two migrations at run time, and
//! `migration_extensions.rs` replays a prefix of the real ones. What none of them can be is a
//! database **this build did not write**, which is the only kind that can say whether a migration
//! that has already shipped was edited afterwards — the first rule in data-model.md's compatibility
//! list, and until this file the one nothing checked.
//!
//! The fixtures are committed, frozen, and copied before they are opened; see
//! [`mixengine_testkit::upgrade`].

use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};

use mixengine_core::Store;
use mixengine_testkit::upgrade::Fixture;
use sqlx::Row as _;
use sqlx::migrate::Migrator;
use sqlx::{ConnectOptions as _, Connection as _};
use tempfile::TempDir;

/// The migrations this build carries, as this suite's own handle on them.
///
/// Read from the same directory `Store`'s embedded set is read from, but declared here rather than
/// borrowed: what several tests below need is the *list*, and `Store` deliberately exposes none.
static MIGRATIONS: Migrator = sqlx::migrate!("./migrations");

/// A fixture copied into a directory this test owns.
///
/// Never the committed file itself: [`Store::open`] migrates what it is given, so a suite handed
/// the source would rewrite the repository's fixture on its first run.
fn laid_out(fixture: &Fixture) -> (TempDir, PathBuf) {
    let temp = TempDir::new().expect("a temporary directory");
    let file = fixture.copy_into(&temp.path().join("mixengine.db"));
    (temp, file)
}

/// Where `Store::back_up` puts the copy.
///
/// Restated here because it is private there, and because what this suite asserts is the *name* a
/// person has to be able to find afterwards.
fn backup_of(file: &Path) -> PathBuf {
    let mut name = file.as_os_str().to_os_string();
    name.push(format!(".bak-{}", env!("CARGO_PKG_VERSION")));
    PathBuf::from(name)
}

/// The versions applied to a database, read without migrating it.
async fn applied(file: &Path) -> Vec<i64> {
    let mut connection = sqlx::sqlite::SqliteConnectOptions::new()
        .filename(file)
        .create_if_missing(false)
        .connect()
        .await
        .expect("the database");

    let versions = sqlx::query_scalar("SELECT version FROM _sqlx_migrations ORDER BY version")
        .fetch_all(&mut connection)
        .await
        .expect("the bookkeeping");

    connection.close().await.expect("the reader closes");
    versions
}

/// Every migration version this build carries.
fn shipped() -> Vec<i64> {
    MIGRATIONS
        .iter()
        .filter(|migration| !migration.migration_type.is_down_migration())
        .map(|migration| migration.version)
        .collect()
}

/// Every migration in this tree that **empties** a table instead of carrying its rows across.
///
/// `0006_site_state.sql` opens with `DROP TABLE site_service_links; DROP TABLE site_domains;
/// DROP TABLE sites;` and then creates `sites` afresh — no `INSERT … SELECT` — so every site, every
/// domain and every link in a database older than migration 6 is gone. `0016_extensions.sql` does
/// the same to `extensions`, while the `services` rebuild beside it in the same file does carry its
/// rows over.
///
/// **Named rather than skipped**, and keyed by version, so a fifth destructive migration cannot
/// hide behind this list: a table emptied without an entry fails
/// [`an_upgrade_keeps_every_row_it_found`] like any other loss. And the entries here are *proved*
/// by [`the_tables_two_migrations_empty_really_are_emptied`] rather than merely excused — an
/// exception that quietly covered a partial loss would be worse than none.
///
/// **This is a finding, not a fix.** Nothing has ever been released from this repository, so the
/// set of databases in the world below schema 17 is empty and every user's first `mixengine.db` is
/// written at 17 or later. Rewriting a migration to repair an upgrade nobody will perform would
/// break data-model.md's first compatibility rule and invalidate every developer's local database,
/// in exchange for nothing.
const EMPTIED: &[(i64, &str)] = &[
    (6, "sites"),
    (6, "site_domains"),
    (6, "site_service_links"),
    (16, "extensions"),
];

/// The tables a fixture at `schema` will not carry across, per [`EMPTIED`].
fn exempt(schema: i64) -> BTreeSet<&'static str> {
    EMPTIED
        .iter()
        .filter(|(version, _)| *version > schema)
        .map(|(_, table)| *table)
        .collect()
}

/// Every row of every table, rendered by SQLite itself.
type Census = BTreeMap<String, Vec<BTreeMap<String, String>>>;

/// Read `file` without migrating it.
///
/// `quote()` and not a typed read: it is SQLite's own faithful rendering of any value — `NULL` for
/// a null, `'x'` for text, `X'00ff'` for a blob, the numeral for a number — so one comparison
/// covers every column type without this suite knowing any of them.
///
/// `_sqlx_migrations` is excluded because it is supposed to grow.
async fn census(file: &Path) -> Census {
    let mut connection = sqlx::sqlite::SqliteConnectOptions::new()
        .filename(file)
        .create_if_missing(false)
        .connect()
        .await
        .expect("the database");

    let tables: Vec<String> = sqlx::query_scalar(
        "SELECT name FROM sqlite_master
         WHERE type = 'table' AND name NOT LIKE 'sqlite_%' AND name != '_sqlx_migrations'
         ORDER BY name",
    )
    .fetch_all(&mut connection)
    .await
    .expect("the tables");

    let mut census = Census::new();

    for table in tables {
        let columns: Vec<String> =
            sqlx::query_scalar("SELECT name FROM pragma_table_info(?) ORDER BY name")
                .bind(&table)
                .fetch_all(&mut connection)
                .await
                .expect("the columns");

        let projection = columns
            .iter()
            .map(|column| format!("quote(\"{column}\")"))
            .collect::<Vec<_>>()
            .join(", ");

        // `AssertSqlSafe`, audited: every name here came out of this database's own `sqlite_master`
        // and `pragma_table_info`, which is our schema and not anything a user typed.
        let rows = sqlx::query(sqlx::AssertSqlSafe(format!(
            "SELECT {projection} FROM \"{table}\""
        )))
        .fetch_all(&mut connection)
        .await
        .unwrap_or_else(|error| panic!("reading {table}: {error}"));

        let mut counted: Vec<BTreeMap<String, String>> = rows
            .iter()
            .map(|row| {
                columns
                    .iter()
                    .enumerate()
                    .map(|(index, column)| (column.clone(), row.get::<String, _>(index)))
                    .collect()
            })
            .collect();

        // A table rebuild is free to reorder; what must not change is the set of rows.
        counted.sort();
        census.insert(table, counted);
    }

    connection.close().await.expect("the reader closes");
    census
}

/// One table's rows, restricted to `columns`.
///
/// The caller passes the columns both censuses have: a migration that *adds* one is not a loss, and
/// the intersection is also what keeps `0014`'s `SET trusted = 1` and `0015`'s
/// `SET signature = 'verified'` out of the comparison — both write a column that did not exist on
/// the other side.
fn shared(
    rows: &[BTreeMap<String, String>],
    columns: &BTreeSet<String>,
) -> Vec<BTreeMap<String, String>> {
    let mut projected: Vec<BTreeMap<String, String>> = rows
        .iter()
        .map(|row| {
            row.iter()
                .filter(|(column, _)| columns.contains(*column))
                .map(|(column, value)| (column.clone(), value.clone()))
                .collect()
        })
        .collect();
    projected.sort();
    projected
}

/// The column names a table's census rows carry, which is empty for a table with no rows.
fn columns_of(rows: &[BTreeMap<String, String>]) -> BTreeSet<String> {
    rows.first()
        .map(|row| row.keys().cloned().collect())
        .unwrap_or_default()
}

/// What is in a directory, by file name, sorted.
fn contents(directory: &Path) -> Vec<String> {
    let mut names: Vec<String> = std::fs::read_dir(directory)
        .expect("the home")
        .filter_map(Result::ok)
        .map(|entry| entry.file_name().to_string_lossy().into_owned())
        .collect();
    names.sort();
    names
}

#[tokio::test]
async fn an_old_database_opens_and_ends_up_at_this_builds_schema() {
    for fixture in Fixture::all() {
        let (_temp, file) = laid_out(&fixture);

        let store = Store::open(&file).await.unwrap_or_else(|error| {
            panic!(
                "{} did not migrate: {error:?}\n\
                 IncompatibleDatabase means a shipped migration was edited; Migration means our \
                 SQL is wrong; Database means the file cannot be used",
                fixture.name()
            )
        });
        store.close().await;

        assert_eq!(
            applied(&file).await,
            shipped(),
            "{} did not end up at this build's schema",
            fixture.name()
        );
    }
}

#[tokio::test]
async fn the_copy_is_taken_when_there_is_something_to_lose_and_not_otherwise() {
    let head = *shipped().last().expect("this build carries a migration");

    for fixture in Fixture::all() {
        let (_temp, file) = laid_out(&fixture);
        let behind = fixture.schema() < head;

        Store::open(&file)
            .await
            .expect("the database migrates")
            .close()
            .await;

        assert_eq!(
            backup_of(&file).exists(),
            behind,
            "{} is at schema {} and this build is at {head}",
            fixture.name(),
            fixture.schema()
        );
    }
}

#[tokio::test]
async fn opening_it_a_second_time_changes_nothing() {
    for fixture in Fixture::all() {
        let (temp, file) = laid_out(&fixture);

        Store::open(&file).await.expect("the upgrade").close().await;
        let after_the_upgrade = contents(temp.path());

        Store::open(&file)
            .await
            .expect("the second open")
            .close()
            .await;

        // The daemon starts many times a day. A backup per start would fill a person's home with
        // copies of a database nothing migrated.
        assert_eq!(
            after_the_upgrade,
            contents(temp.path()),
            "{} gained a file on a no-op open",
            fixture.name()
        );
    }
}

#[tokio::test]
async fn an_upgrade_keeps_every_row_it_found() {
    for fixture in Fixture::all() {
        let (_temp, file) = laid_out(&fixture);
        let before = census(&file).await;

        Store::open(&file).await.expect("the upgrade").close().await;

        let after = census(&file).await;
        let exempt = exempt(fixture.schema());
        let mut compared = 0;

        for (table, rows) in &before {
            if exempt.contains(table.as_str()) || rows.is_empty() {
                continue;
            }
            compared += 1;

            let migrated = after
                .get(table)
                .unwrap_or_else(|| panic!("{}: the migration dropped {table}", fixture.name()));

            let columns: BTreeSet<String> = columns_of(rows)
                .intersection(&columns_of(migrated))
                .cloned()
                .collect();

            assert_eq!(
                shared(rows, &columns),
                shared(migrated, &columns),
                "{}: {table} is not what it was",
                fixture.name()
            );
        }

        // Ten of the fourteen tables `0001_initial.sql` creates, at the least. A census over a
        // fixture that seeded nothing would compare nothing and pass, which is the shape of failure
        // this whole file exists to stop.
        assert!(
            compared >= 10,
            "{} carried rows in only {compared} tables, which is not a fixture worth having",
            fixture.name()
        );
    }
}

#[tokio::test]
async fn the_tables_two_migrations_empty_really_are_emptied() {
    for fixture in Fixture::all() {
        let exempt = exempt(fixture.schema());
        if exempt.is_empty() {
            continue;
        }

        let (_temp, file) = laid_out(&fixture);
        let before = census(&file).await;

        Store::open(&file).await.expect("the upgrade").close().await;

        let after = census(&file).await;

        for table in &exempt {
            assert!(
                before.get(*table).is_some_and(|rows| !rows.is_empty()),
                "{}: {table} is exempt from the census but the fixture seeds nothing into it, so \
                 the exemption proves nothing",
                fixture.name()
            );
            assert_eq!(
                after.get(*table).map(Vec::len),
                Some(0),
                "{}: {table} is listed in EMPTIED, so the loss must be total — a partial one is a \
                 wrong entry, not an excused table",
                fixture.name()
            );
        }
    }
}

#[tokio::test]
async fn the_copy_taken_first_is_the_database_as_it_was() {
    for fixture in Fixture::all() {
        let (_temp, file) = laid_out(&fixture);
        let before = census(&file).await;

        Store::open(&file).await.expect("the upgrade").close().await;

        let backup = backup_of(&file);
        if !backup.exists() {
            // Nothing to migrate, so nothing to copy — asserted by
            // `the_copy_is_taken_when_there_is_something_to_lose_and_not_otherwise`.
            continue;
        }

        assert_eq!(
            census(&backup).await,
            before,
            "{}: the copy is of the state *after* the upgrade, which is not a backup",
            fixture.name()
        );
    }
}

#[tokio::test]
async fn no_shipped_migration_has_been_edited_since_a_fixture_recorded_it() {
    for fixture in Fixture::all() {
        let (_temp, file) = laid_out(&fixture);

        let mut connection = sqlx::sqlite::SqliteConnectOptions::new()
            .filename(&file)
            .create_if_missing(false)
            .connect()
            .await
            .expect("the fixture");

        let recorded: Vec<(i64, Vec<u8>)> =
            sqlx::query_as("SELECT version, checksum FROM _sqlx_migrations ORDER BY version")
                .fetch_all(&mut connection)
                .await
                .expect("the bookkeeping");

        connection.close().await.expect("the reader closes");

        assert!(
            !recorded.is_empty(),
            "{} records no migration at all",
            fixture.name()
        );

        for (version, checksum) in recorded {
            let shipped = MIGRATIONS
                .iter()
                .find(|migration| migration.version == version)
                .unwrap_or_else(|| {
                    panic!(
                        "{} was captured at a schema including migration {version}, which this \
                         build no longer carries",
                        fixture.name()
                    )
                });

            // `Store::open` would catch this as `IncompatibleDatabase` — a sentence about a
            // database from another build, which is the wrong paragraph to send the reader to.
            assert_eq!(
                checksum,
                shipped.checksum.as_ref(),
                "migration {version} has been edited since {} recorded it. data-model.md: never \
                 rewrite an existing migration file.",
                fixture.name()
            );
        }
    }
}

#[tokio::test]
async fn an_upgraded_database_takes_the_writes_a_current_build_makes() {
    for fixture in Fixture::all() {
        let (_temp, file) = laid_out(&fixture);
        let store = Store::open(&file).await.expect("the upgrade");

        // A file that opens and then refuses every write is not a migrated database. The three
        // statements a site is, against the schema this build ships.
        let project: i64 = sqlx::query_scalar(
            "INSERT INTO projects (name, root_path, created_at)
             VALUES ('after-the-upgrade', '/home/dev/after-the-upgrade', '2026-09-05T09:00:00Z')
             RETURNING id",
        )
        .fetch_one(store.pool())
        .await
        .unwrap_or_else(|error| panic!("{}: a project: {error}", fixture.name()));

        let site: i64 = sqlx::query_scalar(
            "INSERT INTO sites (project_id, doc_root, kind) VALUES (?, 'public', 'static')
             RETURNING id",
        )
        .bind(project)
        .fetch_one(store.pool())
        .await
        .unwrap_or_else(|error| panic!("{}: a site: {error}", fixture.name()));

        sqlx::query("INSERT INTO site_domains (site_id, domain, is_primary) VALUES (?, ?, 1)")
            .bind(site)
            .bind("after-the-upgrade.test")
            .execute(store.pool())
            .await
            .unwrap_or_else(|error| panic!("{}: a domain: {error}", fixture.name()));

        store.close().await;
    }
}

/// What [`Store::open_read_only`] — *"the shim's door"* — does to a database older than the binary
/// asking, measured rather than reasoned about.
///
/// It neither creates nor migrates, deliberately: a schema upgrade decided by whichever `php -v`
/// ran first is the one moment `mixengine.db` can least afford a surprise. The consequence is a
/// **window**: after a binary upgrade and before the next daemon start, the file on disk is at the
/// old schema while every query in the shim was compiled against the new one, so a column added by
/// the pending migration is one the shim asks for and does not get.
///
/// **This records the fact; it does not close the window.** Closing it is a question about start-up
/// ordering and about what a shim should say when it finds a database older than itself, which is
/// somebody's design and not a line slipped into a test. See
/// `.claude/architecture/data-model.md`.
#[tokio::test]
async fn the_shims_door_opens_an_old_database_and_leaves_it_old() {
    let oldest = Fixture::all()
        .into_iter()
        .find(|fixture| fixture.schema() == 1)
        .expect("a fixture at schema 1 — see the testkit's own suite");

    let (_temp, file) = laid_out(&oldest);

    let reader = Store::open_read_only(&file)
        .await
        .expect("a shim reads a home a daemon has not caught up with yet");

    // The column `0005_runtime_extensions.sql` added, asked for on a database that predates it.
    assert!(
        sqlx::query("SELECT extension_dir FROM runtime_installs")
            .fetch_optional(reader.pool())
            .await
            .is_err(),
        "a database at schema 1 does not have this column, and the shim's queries are compiled \
         against the schema that does"
    );

    reader.close().await;

    assert_eq!(
        applied(&file).await,
        vec![1],
        "reading a home must never migrate it"
    );
}
