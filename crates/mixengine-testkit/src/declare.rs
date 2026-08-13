//! The rows a service has to have before anything can start it.
//!
//! **Scaffolding for a build that cannot create a service yet, and nothing more.** A `services` row
//! is what `mixengine-core` transitions and what the supervisor writes a pid into, and until roadmap
//! task **T30** there is no `service.create` to make one — so every suite that drives a service
//! through the daemon has to put the row there itself. This is that, written once.
//!
//! It fabricates a `packages` row too, because `services.package_id` is `NOT NULL REFERENCES
//! packages (id)` and Phase 2 is what installs a real package. That is the foreign key doing its
//! job rather than an obstacle: a fixture that skipped it would be testing a schema this workspace
//! does not have.
//!
//! **This is the one place in the crate that knows the schema**, which is the exception to the rule
//! [`crate`] states about restating conventions: there is no way to write a row without knowing the
//! table, and the alternative — a second copy inside every suite — is what this crate exists to
//! prevent. The queries are plain [`sqlx::query()`] rather than the checked macro, because a
//! dev-dependency has no business in `.sqlx/`, which is prepared for the crates that ship.

use std::path::Path;

use sqlx::sqlite::{SqliteConnectOptions, SqlitePoolOptions};

/// Declare `ids` in the database at `database`, so each can be started.
///
/// The database has to exist already: the daemon's migrations are what create the schema, so a test
/// starts a daemon (or opens a `Store`) first and calls this afterwards. Ids already present are
/// left as they are, so calling this twice is not an error.
///
/// # Panics
///
/// If the database cannot be opened or the rows cannot be written — a fixture that half worked would
/// fail later as an assertion about the daemon, which is the wrong thing to go looking at.
pub async fn declare(database: &Path, ids: &[&str]) {
    // Read-write and never `create_if_missing`: an empty file where the schema should be is a test
    // that pointed at the wrong path, and creating one would turn that into "no such table".
    let options = SqliteConnectOptions::new()
        .filename(database)
        .create_if_missing(false);

    // One connection, and a busy timeout because the daemon under test is holding the same file.
    // WAL lets a writer and readers coexist; two writers still take turns.
    let pool = SqlitePoolOptions::new()
        .max_connections(1)
        .connect_with(options)
        .await
        .unwrap_or_else(|error| panic!("{} is a MixEngine database: {error}", database.display()));

    for id in ids {
        sqlx::query(
            "INSERT INTO packages (name, version, install_path, installed_at, source_url, sha256)
             VALUES (?, '1.0.0', '/packages/x', '2026-08-12T00:00:00Z', 'https://example', 'ab')
             ON CONFLICT (name, version) DO NOTHING",
        )
        .bind(id)
        .execute(&pool)
        .await
        .unwrap_or_else(|error| panic!("a package row for `{id}`: {error}"));

        sqlx::query(
            "INSERT INTO services (id, package_id, instance_name, state)
             VALUES (?, (SELECT id FROM packages WHERE name = ?), 'main', 'stopped')
             ON CONFLICT (id) DO NOTHING",
        )
        .bind(id)
        .bind(id)
        .execute(&pool)
        .await
        .unwrap_or_else(|error| panic!("a services row for `{id}`: {error}"));
    }

    pool.close().await;
}

/// [`declare`], for a test that has no runtime of its own.
///
/// The end-to-end suites drive `mix` through [`std::process::Command`] and are plain `#[test]`
/// functions; building a runtime for two inserts is cheaper than making every one of them `async`.
///
/// # Panics
///
/// As [`declare`], and if a runtime cannot be started.
pub fn declare_blocking(database: &Path, ids: &[&str]) {
    tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .expect("a current-thread runtime")
        .block_on(declare(database, ids));
}
