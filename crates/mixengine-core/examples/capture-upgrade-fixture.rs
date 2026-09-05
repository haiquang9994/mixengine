//! Capture a frozen `mixengine.db` at a schema version — the fixtures T89's suite migrates.
//!
//! ```text
//! cargo run -p mixengine-core --example capture-upgrade-fixture -- 1
//! ```
//!
//! **It refuses a destination that exists**, and that refusal is the whole design. A fixture is
//! evidence exactly as long as nobody regenerates it: the failure this guards against is not malice
//! but convenience — CI goes red, somebody re-runs this, CI goes green, and the thing the test
//! existed to catch has been erased by the commit that hid it. Overwriting takes a deliberate `rm`.
//!
//! The migrations are applied through sqlx's **own** `Migrate::apply` rather than by running their
//! SQL and writing the bookkeeping by hand, so that a captured database is bookkept exactly as
//! `Migrator::run` would have bookkept it — including the checksum, which is the one row in the
//! file that cannot be reconstructed honestly later.

use std::path::{Path, PathBuf};
use std::process::ExitCode;

use sqlx::migrate::{Migrate as _, Migrator};
use sqlx::sqlite::{SqliteConnectOptions, SqliteJournalMode, SqliteSynchronous};
use sqlx::{ConnectOptions as _, Connection as _};

/// The migrations this build carries, read from `mixengine-core`'s own directory.
static MIGRATIONS: Migrator = sqlx::migrate!("./migrations");

/// Where the fixtures live, relative to this crate.
const FIXTURES: &str = "../mixengine-testkit/fixtures/upgrade";

#[tokio::main]
async fn main() -> ExitCode {
    let Some(schema) = std::env::args()
        .nth(1)
        .and_then(|argument| argument.parse::<i64>().ok())
    else {
        eprintln!(
            "usage: cargo run -p mixengine-core --example capture-upgrade-fixture -- <schema>"
        );
        return ExitCode::FAILURE;
    };

    let directory = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join(FIXTURES);
    let database = directory.join(format!("schema-{schema:04}.db"));
    let seed = directory.join(format!("schema-{schema:04}.sql"));

    if database.exists() {
        eprintln!(
            "{} is already there. A captured fixture is frozen — delete it deliberately if you \
             really mean to replace it.",
            database.display()
        );
        return ExitCode::FAILURE;
    }

    let Ok(statements) = std::fs::read_to_string(&seed) else {
        eprintln!("no seed at {}; write it before capturing", seed.display());
        return ExitCode::FAILURE;
    };

    if let Err(error) = capture(&database, schema, statements).await {
        eprintln!("{error}");
        // A half-written capture is not a fixture, and leaving one where the check above looks
        // would make the next attempt refuse for the wrong reason.
        remove(&database);
        return ExitCode::FAILURE;
    }

    let bytes = std::fs::metadata(&database)
        .map(|file| file.len())
        .unwrap_or_default();
    println!("captured {} ({bytes} bytes)", database.display());
    ExitCode::SUCCESS
}

/// Build the database: the migrations up to `schema`, then the seed, then a vacuum.
///
/// `statements` is taken by value because `sqlx::raw_sql` wants a string it can hold for as long as
/// the statement runs — a borrow of this function's argument would have to outlive the call.
async fn capture(database: &Path, schema: i64, statements: String) -> Result<(), String> {
    // The connection settings `Store::open` uses, so a captured file is one this product would have
    // written rather than one this tool invented.
    let mut connection = SqliteConnectOptions::new()
        .filename(database)
        .create_if_missing(true)
        .journal_mode(SqliteJournalMode::Wal)
        .synchronous(SqliteSynchronous::Normal)
        .foreign_keys(true)
        .connect()
        .await
        .map_err(|error| format!("opening {}: {error}", database.display()))?;

    connection
        .ensure_migrations_table(&MIGRATIONS.table_name)
        .await
        .map_err(|error| format!("the bookkeeping table: {error}"))?;

    let mut applied = 0;
    for migration in MIGRATIONS
        .iter()
        .filter(|migration| !migration.migration_type.is_down_migration())
        .filter(|migration| migration.version <= schema)
    {
        connection
            .apply(&MIGRATIONS.table_name, migration)
            .await
            .map_err(|error| format!("migration {}: {error}", migration.version))?;
        applied += 1;
    }

    if applied == 0 {
        return Err(format!(
            "this build carries no migration at or below {schema}"
        ));
    }

    // `AssertSqlSafe`, audited: the statements come from a file committed to this repository and
    // read by a developer running this tool by hand. Nothing a user types reaches here, and there
    // is no parameter to bind — a seed *is* the literal rows a fixture is supposed to hold.
    sqlx::raw_sql(sqlx::AssertSqlSafe(statements))
        .execute(&mut connection)
        .await
        .map_err(|error| format!("the seed: {error}"))?;

    // Everything the write-ahead log is holding, moved into the file itself — then a vacuum, which
    // is what keeps a fixture a few tens of kilobytes rather than a few hundred.
    for statement in [
        "PRAGMA wal_checkpoint(TRUNCATE)",
        "VACUUM",
        "PRAGMA wal_checkpoint(TRUNCATE)",
    ] {
        sqlx::raw_sql(statement)
            .execute(&mut connection)
            .await
            .map_err(|error| format!("{statement}: {error}"))?;
    }

    connection
        .close()
        .await
        .map_err(|error| format!("closing {}: {error}", database.display()))?;

    // A fixture is one file. A `-wal` beside it is a database missing exactly the most recent
    // commits, which is the trap data-model.md describes about copying one with `fs::copy`.
    for suffix in ["-wal", "-shm"] {
        remove(&PathBuf::from(format!("{}{suffix}", database.display())));
    }

    Ok(())
}

/// Delete a file if it is there, and say nothing if it is not.
fn remove(file: &Path) {
    if let Err(error) = std::fs::remove_file(file)
        && error.kind() != std::io::ErrorKind::NotFound
    {
        eprintln!("could not remove {}: {error}", file.display());
    }
}
