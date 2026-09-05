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

use std::path::{Path, PathBuf};

use mixengine_core::Store;
use mixengine_testkit::upgrade::Fixture;
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
