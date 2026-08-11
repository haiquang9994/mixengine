//! `mixengine.db`: the declared state, and the only thing here that cannot be regenerated.
//!
//! Everything else MixEngine owns is disposable — `etc/` is rendered from this database, `runtimes/`
//! can be downloaded again, `logs/` is history. This file is the one whose loss costs the user their
//! sites, so the rules around it are stricter than the rest: it is opened once, by the daemon, and
//! it is copied aside before any migration touches it.
//!
//! The connection settings are spelled out rather than inherited from sqlx's defaults. They are the
//! difference between a database that survives a power cut and one that does not, and a default is
//! something the next release is allowed to change.

use std::collections::BTreeSet;
use std::path::{Path, PathBuf};
use std::time::Duration;

use sqlx::SqlitePool;
use sqlx::migrate::{Migrate as _, MigrateError, Migrator};
use sqlx::sqlite::{SqliteConnectOptions, SqliteJournalMode, SqlitePoolOptions, SqliteSynchronous};

use crate::{Error, Result};

/// The migrations, compiled into the binary.
///
/// Embedded rather than read from disk at run time: an installed `mixengined` has no source tree
/// next to it, and a schema that could go missing is a daemon that cannot start. `build.rs` is what
/// makes an edit here reach the next build — see the note there.
static MIGRATIONS: Migrator = sqlx::migrate!("./migrations");

/// Connections in the pool.
///
/// SQLite serialises writers, so a larger number buys nothing there — but WAL lets readers run
/// while a write is in flight, and a single connection would put every status query behind whatever
/// slow write is currently going on. Four is enough for a daemon whose concurrency is a handful of
/// RPC handlers and a supervisor.
const MAX_CONNECTIONS: u32 = 4;

/// How long a connection waits for another one to release the write lock before giving up.
///
/// Without it SQLite returns `SQLITE_BUSY` immediately and the caller sees a failed request during
/// perfectly ordinary contention — two RPCs writing at once, which is a Tuesday, not an error.
const BUSY_TIMEOUT: Duration = Duration::from_secs(5);

/// The open database.
///
/// Cheap to clone: every clone shares one connection pool, which is what makes it the `Arc<Store>`
/// the standards describe handing to whatever needs state.
#[derive(Debug, Clone)]
pub struct Store {
    pool: SqlitePool,
    file: PathBuf,
}

impl Store {
    /// Open `file`, creating it if it is not there, and bring its schema up to date.
    ///
    /// Idempotent, and that is load-bearing: the daemon runs this on every start, and all but the
    /// first find a database that is already current and change nothing about it.
    ///
    /// # Errors
    ///
    /// [`Error::Database`] when the file cannot be opened or used as one — a directory that is not
    /// there, a disk that is not mounted, a file another account owns, a volume mounted read-only;
    /// [`Error::Backup`] when the copy that has to exist before a migration cannot be written,
    /// which stops the migration rather than running it unprotected; [`Error::Migration`] when a
    /// statement in one of our own migrations fails; and [`Error::IncompatibleDatabase`] when the
    /// file was written by a build whose migrations are not this build's.
    pub async fn open(file: &Path) -> Result<Self> {
        Self::open_with(file, &MIGRATIONS).await
    }

    /// [`Store::open`], with the migrations named explicitly.
    ///
    /// Private because there is exactly one real set of migrations. It exists because the tests
    /// need a database that is *behind* — with a single embedded migration, every database is
    /// either empty or current, and the interesting path (back up, then upgrade) has no way to
    /// happen.
    async fn open_with(file: &Path, migrations: &Migrator) -> Result<Self> {
        let options = SqliteConnectOptions::new()
            .filename(file)
            // The daemon is the only process that creates this file, and a first run has to work
            // without a separate "initialise" step.
            .create_if_missing(true)
            // WAL is what lets a reader run during a write. It is a property of the file, not of
            // the connection, so this converts an older database on first open.
            .journal_mode(SqliteJournalMode::Wal)
            // `Normal` is the recommended companion to WAL: fsync at each checkpoint rather than at
            // every commit. A power cut can lose the last transactions; it cannot corrupt the file.
            // `Full` would fsync per commit and make a bulk install crawl for a guarantee a
            // development environment does not need.
            .synchronous(SqliteSynchronous::Normal)
            // Off by default in SQLite itself, per connection rather than per file, and the reason
            // the foreign keys in the schema are worth writing at all.
            .foreign_keys(true)
            .busy_timeout(BUSY_TIMEOUT);

        let pool = SqlitePoolOptions::new()
            .max_connections(MAX_CONNECTIONS)
            .connect_with(options)
            .await
            .map_err(|source| Error::Database {
                action: "open",
                path: file.to_path_buf(),
                source,
            })?;

        let store = Self {
            pool,
            file: file.to_path_buf(),
        };

        store.migrate(migrations).await?;

        Ok(store)
    }

    /// The connection pool.
    ///
    /// Queries belong in `core`'s own modules — `sites`, `runtimes`, `services` — and not in the
    /// daemon, which is where "no business logic outside core" would start to erode. This is public
    /// so the daemon can hand the store around and so tests can look at what a migration produced.
    #[must_use]
    pub fn pool(&self) -> &SqlitePool {
        &self.pool
    }

    /// The file this store was opened from.
    #[must_use]
    pub fn file(&self) -> &Path {
        &self.file
    }

    /// A failed query, named as this file.
    ///
    /// Every domain module maps `sqlx::Error` the same way and would otherwise restate the path at
    /// each call site — which is how one of them ends up reporting a failure without saying which
    /// database it was, on the machine where `[paths]` moved it somewhere surprising.
    pub(crate) fn failure(&self, action: &'static str, source: sqlx::Error) -> Error {
        Error::Database {
            action,
            path: self.file.clone(),
            source,
        }
    }

    /// Close every connection and checkpoint the write-ahead log.
    ///
    /// Worth awaiting on the way out: dropping the pool closes the connections without waiting, and
    /// a `-wal` file left beside the database makes a backup taken with a file copy — the user's,
    /// not ours — silently miss the most recent commits.
    pub async fn close(&self) {
        self.pool.close().await;
    }

    /// Apply whatever is not applied yet, having first copied the database aside if there is
    /// anything to lose.
    async fn migrate(&self, migrations: &Migrator) -> Result<()> {
        if self.schema_state(migrations).await? == Schema::Behind {
            self.back_up().await?;
        }

        migrations
            .run(&self.pool)
            .await
            .map_err(|source| self.migration_failure(source))
    }

    /// Where this database stands relative to the migrations this build carries.
    async fn schema_state(&self, migrations: &Migrator) -> Result<Schema> {
        let mut connection = self
            .pool
            .acquire()
            .await
            .map_err(|source| Error::Database {
                action: "read",
                path: self.file.clone(),
                source,
            })?;

        // Creating the bookkeeping table is the first thing `run` would do anyway; doing it here
        // means "nothing applied" and "never opened" are the same answer rather than an error.
        connection
            .ensure_migrations_table(&migrations.table_name)
            .await
            .map_err(|source| self.migration_failure(source))?;

        let applied: BTreeSet<i64> = connection
            .list_applied_migrations(&migrations.table_name)
            .await
            .map_err(|source| self.migration_failure(source))?
            .into_iter()
            .map(|migration| migration.version)
            .collect();

        if applied.is_empty() {
            return Ok(Schema::Empty);
        }

        let outstanding = migrations
            .iter()
            .filter(|migration| !migration.migration_type.is_down_migration())
            .any(|migration| !applied.contains(&migration.version));

        Ok(if outstanding {
            Schema::Behind
        } else {
            Schema::Current
        })
    }

    /// Copy the database to `mixengine.db.bak-<version>` before a migration changes it.
    ///
    /// `VACUUM INTO` rather than `std::fs::copy`, and the difference is not cosmetic: under WAL the
    /// most recent commits live in the `-wal` sidecar until a checkpoint moves them, so copying the
    /// main file alone produces a backup that is missing exactly the work the user did most
    /// recently. `VACUUM INTO` asks SQLite for the committed *content*, which is what a backup is.
    ///
    /// A backup that already exists is kept rather than replaced. Same-version backups only happen
    /// when an upgrade ran twice, and in that pair it is the older file — from before the first,
    /// possibly half-finished attempt — that is worth having.
    ///
    /// Which is only sound because the copy is written to a `.partial` beside it and renamed into
    /// place. "Is there already a backup?" is a question the *next* run asks by looking for a file,
    /// and a copy that died half way would answer it wrongly — leaving a truncated database where a
    /// safety net is supposed to be, and this function stepping over it. After the rename, a file
    /// at that path can only have come from a copy that finished.
    async fn back_up(&self) -> Result<()> {
        let backup = backup_path(&self.file);

        if backup.exists() {
            tracing::warn!(
                path = %backup.display(),
                "a backup from this version is already there — keeping it and migrating anyway"
            );
            return Ok(());
        }

        // Left behind by a copy that died half way. It is not a backup and never was — and
        // `VACUUM INTO` refuses a destination that exists, so it has to go before the retry rather
        // than after it.
        let partial = partial_path(&backup);
        if partial.exists() {
            tracing::warn!(
                path = %partial.display(),
                "discarding a copy that did not finish"
            );
            std::fs::remove_file(&partial).map_err(|source| Error::Backup {
                path: partial.clone(),
                source: sqlx::Error::Io(source),
            })?;
        }

        tracing::info!(
            path = %backup.display(),
            "copying the database aside before migrating it"
        );

        // Bound rather than interpolated, so a home directory containing a quote cannot end the
        // string early. Lossy conversion matches how the filename reached SQLite in the first
        // place — a path this one is derived from, and one that has already been opened.
        sqlx::query("VACUUM INTO ?")
            .bind(partial.to_string_lossy().into_owned())
            .execute(&self.pool)
            .await
            .map_err(|source| Error::Backup {
                path: backup.clone(),
                source,
            })?;

        // The rename is what makes a file at `backup` mean "a copy that finished". Atomic on all
        // three platforms, and the destination cannot exist — the check at the top returned if it
        // did. Reported as a failed backup like the copy itself: the two are one operation, and
        // half of it leaves no more of a safety net than none of it.
        std::fs::rename(&partial, &backup).map_err(|source| Error::Backup {
            path: backup,
            source: sqlx::Error::Io(source),
        })
    }

    /// Decide which of the three migration failures this is.
    ///
    /// The split is about who can act on it. Failing to read the bookkeeping is a file that cannot
    /// be used, which the user fixes; a migration that fails while running is our SQL being wrong,
    /// which no user can fix; a version that does not line up means the file was written by a
    /// different build of MixEngine, and the way out is the backup sitting next to it.
    fn migration_failure(&self, source: MigrateError) -> Error {
        let path = self.file.clone();

        match source {
            // The bookkeeping around a migration rather than a migration: creating
            // `_sqlx_migrations` and reading it back, which is all `schema_state` does and which
            // therefore happens on every single daemon start. It fails for the reasons any write to
            // a file fails — a read-only volume, a full disk, a `mixengine.db` that is not a
            // database, another process holding the lock past the busy timeout — so it is reported
            // like every other database failure. Calling it ours would greet a home directory on a
            // read-only disk with "report a bug" instead of naming the file.
            MigrateError::Execute(source) => Error::Database {
                action: "migrate",
                path,
                source,
            },

            MigrateError::VersionMismatch(_)
            | MigrateError::VersionMissing(_)
            | MigrateError::VersionNotPresent(_)
            // Unreachable on SQLite, which has transactional DDL: a migration that fails rolls
            // back whole rather than leaving half of itself applied. That is exactly what lets
            // `Error::Migration` promise the database is untouched. Mapped rather than left to the
            // catch-all anyway, because if it ever does arrive the advice it needs is this one —
            // the copy next to the database — and not "this is a bug in MixEngine".
            | MigrateError::Dirty(_) => Error::IncompatibleDatabase { path, source },

            // `VersionTooOld` and `VersionTooNew` are deliberately not in that list: they mean the
            // migrations *shipped in this binary* are numbered out of order, which is a release
            // mistake and not something the user's file did.
            _ => Error::Migration { path, source },
        }
    }
}

/// Where a database stands relative to the migrations the running build carries.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Schema {
    /// Nothing has ever been applied — a new file, or one this build is about to fill. There is no
    /// state to protect, so no backup is taken: a copy of an empty database is noise in the home
    /// directory and one more thing for the uninstaller to explain.
    Empty,
    /// Every migration this build carries is already applied. The overwhelmingly common case, and
    /// the one that has to be free.
    Current,
    /// Something is applied and something is not: a database with the user's sites in it is about
    /// to be changed by an upgrade. This is what the backup exists for.
    Behind,
}

/// `mixengine.db` → `mixengine.db.bak-0.1.0`.
///
/// The version is the one doing the migrating, not the one that wrote the file, because that is the
/// question being answered later: *which upgrade do I need to undo?* Built by appending to the
/// whole file name rather than through `set_extension`, which would eat the `.db`.
fn backup_path(file: &Path) -> PathBuf {
    let mut name = file.as_os_str().to_os_string();
    name.push(format!(".bak-{}", env!("CARGO_PKG_VERSION")));
    PathBuf::from(name)
}

/// `mixengine.db.bak-0.1.0` → `mixengine.db.bak-0.1.0.partial`, where a copy is written before it
/// earns the name next to it.
///
/// Beside the backup rather than in a temporary directory, so that the rename cannot cross a
/// filesystem — that is what makes it atomic, and a `MIXENGINE_HOME` on a separate disk is an
/// ordinary thing for a user to have.
fn partial_path(backup: &Path) -> PathBuf {
    let mut name = backup.as_os_str().to_os_string();
    name.push(".partial");
    PathBuf::from(name)
}

#[cfg(test)]
mod tests {
    use std::path::PathBuf;

    use super::*;

    /// A migrator built from SQL written at run time, which is the only way to have a database that
    /// is *behind*: the embedded set is applied in full by the first `open`.
    async fn migrator(directory: &Path, files: &[(&str, &str)]) -> Migrator {
        // Emptied rather than added to: a test that asks for one migration after asking for two
        // would otherwise be handed both, and would then quietly assert nothing.
        if directory.exists() {
            std::fs::remove_dir_all(directory).expect("the previous migrations");
        }
        std::fs::create_dir_all(directory).expect("a migrations directory");

        for (name, sql) in files {
            std::fs::write(directory.join(name), sql).expect("a migration");
        }

        Migrator::new(directory).await.expect("a migrator")
    }

    const FIRST: (&str, &str) = ("0001_first.sql", "CREATE TABLE first (id INTEGER) STRICT;");
    const SECOND: (&str, &str) = (
        "0002_second.sql",
        "CREATE TABLE second (id INTEGER) STRICT;",
    );

    fn temporary() -> (tempfile::TempDir, PathBuf) {
        let home = tempfile::tempdir().expect("a temporary directory");
        let file = home.path().join(crate::paths::DATABASE_FILE_NAME);
        (home, file)
    }

    #[tokio::test]
    async fn a_new_database_is_not_backed_up() {
        let (home, file) = temporary();
        let migrations = migrator(&home.path().join("migrations"), &[FIRST]).await;

        Store::open_with(&file, &migrations)
            .await
            .expect("a new database");

        assert!(
            !backup_path(&file).exists(),
            "there was nothing in it to lose"
        );
    }

    #[tokio::test]
    async fn an_upgrade_copies_the_database_aside_first() {
        let (home, file) = temporary();
        let directory = home.path().join("migrations");

        let store = Store::open_with(&file, &migrator(&directory, &[FIRST]).await)
            .await
            .expect("a database at the first version");
        sqlx::query("INSERT INTO first (id) VALUES (1)")
            .execute(store.pool())
            .await
            .expect("a row the user would miss");
        store.close().await;

        let store = Store::open_with(&file, &migrator(&directory, &[FIRST, SECOND]).await)
            .await
            .expect("the upgraded database");

        let backup = backup_path(&file);
        assert!(backup.exists(), "the upgrade left a copy behind");

        // The copy is of the state *before* the upgrade: the row is there, the new table is not.
        let restored = Store::open_with(&backup, &migrator(&directory, &[FIRST]).await)
            .await
            .expect("the backup is a usable database");
        let rows: i64 = sqlx::query_scalar("SELECT count(*) FROM first")
            .fetch_one(restored.pool())
            .await
            .expect("the row survived the copy");
        assert_eq!(rows, 1);
        assert!(
            sqlx::query("SELECT 1 FROM second")
                .fetch_optional(restored.pool())
                .await
                .is_err(),
            "the backup predates the second migration"
        );

        // And the database itself did get upgraded.
        sqlx::query("INSERT INTO second (id) VALUES (1)")
            .execute(store.pool())
            .await
            .expect("the second migration ran");
    }

    #[tokio::test]
    async fn a_backup_from_this_version_is_never_overwritten() {
        let (home, file) = temporary();
        let directory = home.path().join("migrations");

        Store::open_with(&file, &migrator(&directory, &[FIRST]).await)
            .await
            .expect("a database at the first version")
            .close()
            .await;

        // What a half-finished upgrade would have left: the copy from before the first attempt.
        let backup = backup_path(&file);
        std::fs::write(&backup, b"the good one").expect("the earlier backup");

        Store::open_with(&file, &migrator(&directory, &[FIRST, SECOND]).await)
            .await
            .expect("the upgrade goes ahead");

        assert_eq!(
            std::fs::read(&backup).expect("the backup is still there"),
            b"the good one",
            "the older copy is the one worth keeping"
        );
    }

    #[tokio::test]
    async fn a_copy_that_did_not_finish_is_never_mistaken_for_a_backup() {
        let (home, file) = temporary();
        let directory = home.path().join("migrations");

        let store = Store::open_with(&file, &migrator(&directory, &[FIRST]).await)
            .await
            .expect("a database at the first version");
        sqlx::query("INSERT INTO first (id) VALUES (1)")
            .execute(store.pool())
            .await
            .expect("a row the user would miss");
        store.close().await;

        // What a copy killed part way through leaves behind: a truncated file at the path the next
        // run writes to. Before the rename it sat at the *backup* path, where `back_up` reads it as
        // "already done" and steps over it — the user keeping a broken safety net without knowing.
        let backup = backup_path(&file);
        let partial = partial_path(&backup);
        std::fs::write(&partial, b"half a database").expect("the remains of the last attempt");

        Store::open_with(&file, &migrator(&directory, &[FIRST, SECOND]).await)
            .await
            .expect("the upgrade goes ahead");

        assert!(
            backup.exists(),
            "the copy was taken again rather than skipped"
        );
        assert!(
            !partial.exists(),
            "the unfinished one is gone, not left for the next run to trip over"
        );

        let restored = Store::open_with(&backup, &migrator(&directory, &[FIRST]).await)
            .await
            .expect("the backup is a usable database, not the bytes written above");
        let rows: i64 = sqlx::query_scalar("SELECT count(*) FROM first")
            .fetch_one(restored.pool())
            .await
            .expect("the row is in it");
        assert_eq!(rows, 1);
    }

    #[test]
    fn the_unfinished_copy_sits_beside_the_backup_it_will_become() {
        let backup = backup_path(Path::new("/home/mixengine/mixengine.db"));

        // Same directory, so the rename stays within one filesystem and stays atomic.
        assert_eq!(partial_path(&backup).parent(), backup.parent());
    }

    #[tokio::test]
    async fn a_database_from_a_newer_build_is_refused_and_named_as_such() {
        let (home, file) = temporary();
        let directory = home.path().join("migrations");

        Store::open_with(&file, &migrator(&directory, &[FIRST, SECOND]).await)
            .await
            .expect("a database written by the newer build")
            .close()
            .await;

        let error = Store::open_with(&file, &migrator(&home.path().join("older"), &[FIRST]).await)
            .await
            .expect_err("this build has never heard of the second migration");

        assert!(
            matches!(error, Error::IncompatibleDatabase { .. }),
            "not our SQL failing — the file belongs to another build: {error:?}"
        );
    }

    #[tokio::test]
    async fn an_edited_migration_is_refused_rather_than_reapplied() {
        let (home, file) = temporary();
        let directory = home.path().join("migrations");

        Store::open_with(&file, &migrator(&directory, &[FIRST]).await)
            .await
            .expect("the database as released")
            .close()
            .await;

        // The thing data-model.md forbids: editing a migration that has already shipped.
        let edited = (
            FIRST.0,
            "CREATE TABLE first (id INTEGER, extra TEXT) STRICT;",
        );
        let error = Store::open_with(&file, &migrator(&directory, &[edited]).await)
            .await
            .expect_err("the checksum no longer matches");

        assert!(
            matches!(error, Error::IncompatibleDatabase { .. }),
            "{error:?}"
        );
    }

    #[tokio::test]
    async fn a_database_that_cannot_be_read_is_not_blamed_on_our_sql() {
        let (home, file) = temporary();
        let directory = home.path().join("migrations");

        let store = Store::open_with(&file, &migrator(&directory, &[FIRST]).await)
            .await
            .expect("a database at the first version");

        // A bookkeeping table that is not sqlx's, standing in for the reasons this actually
        // happens — a read-only volume, a full disk, a file that is not a database. All of them
        // arrive as `MigrateError::Execute` before a migration of ours has run at all, and none of
        // them is something the user should be told to report as a bug.
        for statement in [
            "DROP TABLE _sqlx_migrations",
            "CREATE TABLE _sqlx_migrations (nonsense TEXT) STRICT",
        ] {
            sqlx::query(statement)
                .execute(store.pool())
                .await
                .expect("the bookkeeping table, replaced");
        }
        store.close().await;

        let error = Store::open_with(&file, &migrator(&directory, &[FIRST]).await)
            .await
            .expect_err("the applied versions cannot be read");

        assert!(
            matches!(&error, Error::Database { action: "migrate", path, .. } if path == &file),
            "{error:?}"
        );
    }

    #[test]
    fn the_backup_keeps_the_database_extension_in_its_name() {
        assert_eq!(
            backup_path(Path::new("/home/mixengine/mixengine.db")),
            PathBuf::from(format!(
                "/home/mixengine/mixengine.db.bak-{}",
                env!("CARGO_PKG_VERSION")
            ))
        );
    }
}
