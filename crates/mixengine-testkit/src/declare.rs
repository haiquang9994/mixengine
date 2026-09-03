//! The rows a service has to have before anything can start it, and the one a killed daemon leaves.
//!
//! **Scaffolding for a build that cannot create a service yet, and nothing more.** A `services` row
//! is what `mixengine-core` transitions, what the supervisor writes a pid into and — since roadmap
//! task **T30** — what the daemon renders into a configuration and a `ServiceSpec`. What is still
//! missing is Phase 3's `service.create`, so every suite that drives a service through the daemon
//! has to put the row there itself. This is that, written once.
//!
//! [`running`] is the exception that is not scaffolding for a missing feature but for a state no
//! test can ask a daemon to produce: a daemon that is running clears those columns on its way out,
//! whichever way it is asked to stop, so the only way to hand a *new* daemon the row a killed one
//! leaves behind is to write it. Crash recovery (roadmap task **T18**) is what reads it.
//!
//! # Every row here belongs to `fakeservice`
//!
//! A daemon turns a row into something runnable by looking up a **recipe** for `packages.name`, and
//! the recipe a debug build has beyond what MixEngine ships is the one for this crate's own
//! `fakeservice` — see `crates/mixengine-daemon/src/services/fakeservice.rs`. So the row [`package`]
//! writes names that program and points at the directory it was built into, and how a service is to
//! *behave* is said in overrides through [`Service`] rather than in a spec the test writes. That is
//! what replaced `MIXENGINE_DEV_SPECS`, and it is narrower in the way that matters: a test
//! configures a service, where before it described an arbitrary program to run.
//!
//! A suite with a **real** server on disk — the Caddy CI fetches — has no business here at all any
//! more: since T31a it packs that server into an archive, serves it from a `MockRegistry` and
//! installs it through `package.install`, which is the path a user takes.
//!
//! **This is the one place in the crate that knows the schema**, which is the exception to the rule
//! [`crate`] states about restating conventions: there is no way to write a row without knowing the
//! table, and the alternative — a second copy inside every suite — is what this crate exists to
//! prevent. The queries are plain [`sqlx::query()`] rather than the checked macro, because a
//! dev-dependency has no business in `.sqlx/`, which is prepared for the crates that ship.

use std::collections::BTreeMap;
use std::path::Path;

use serde_json::json;
use sqlx::sqlite::{SqliteConnectOptions, SqlitePoolOptions};

use crate::service::FakeService;

/// A service to declare, and how it is to behave once something starts it.
///
/// Every method here sets one override the `fakeservice` recipe declares, so what a test writes and
/// what the daemon reads are the same vocabulary. A bare [`new`](Self::new) is a service that
/// announces itself ready at once and then waits to be stopped — the well-behaved baseline the
/// misbehaving ones are compared against, exactly as [`FakeService`] is.
#[derive(Debug, Clone)]
pub struct Service {
    /// The `ServiceId`, which is also the row's primary key.
    id: String,

    /// What goes into `config_overrides_json`.
    overrides: BTreeMap<String, serde_json::Value>,
}

impl Service {
    /// A service that behaves.
    #[must_use]
    pub fn new(id: impl Into<String>) -> Self {
        Self {
            id: id.into(),
            overrides: BTreeMap::new(),
        }
    }

    /// Its id.
    #[must_use]
    pub fn id(&self) -> &str {
        &self.id
    }

    /// Take this long to announce readiness.
    #[must_use]
    pub fn ready_after(self, millis: u64) -> Self {
        self.set("ready_after", json!(millis))
    }

    /// Never announce readiness at all, however long it is given.
    #[must_use]
    pub fn never_ready(self) -> Self {
        self.set("never_ready", json!(true))
    }

    /// How long the supervisor waits for readiness before giving up.
    ///
    /// The one override a test that means to wait a timeout out has to set: the default is the
    /// twenty seconds a loaded Windows runner needs, and waiting that out on purpose is twenty
    /// seconds per test.
    #[must_use]
    pub fn ready_timeout(self, millis: u64) -> Self {
        self.set("ready_timeout_ms", json!(millis))
    }

    /// Print a line this often, so there is something for a log stream to carry.
    #[must_use]
    pub fn log_every(self, millis: u64) -> Self {
        self.set("log_every", json!(millis))
    }

    /// Take another `mb` megabytes every 50 ms and never let go of any of it.
    ///
    /// **For walking a service into a memory ceiling** — roadmap task **T68**. The only way to prove
    /// a cap by *outcome*: a service that dies is a discrete event, where a CPU cap is a rate and
    /// asserting a rate means timing work on a shared runner.
    ///
    /// Pick a bite well under the ceiling, so it is reached in steps: a single request larger than
    /// the cap can fail on its own, which would prove the allocator refused a large ask rather than
    /// that the ceiling refused it.
    #[must_use]
    pub fn eating_memory(self, mb: usize) -> Self {
        self.set("eat_memory_mb", json!(mb))
    }

    /// Exit on its own after this long, with [`exit_code`](Self::exit_code).
    #[must_use]
    pub fn exit_after(self, millis: u64) -> Self {
        self.set("exit_after", json!(millis))
    }

    /// What to exit with. Zero is a service that *stopped* rather than crashed.
    #[must_use]
    pub fn exit_code(self, code: i32) -> Self {
        self.set("exit_code", json!(code))
    }

    /// Install the stop handlers and then ignore them, so only a kill ends it.
    #[must_use]
    pub fn ignore_stop(self) -> Self {
        self.set("ignore_stop", json!(true))
    }

    /// How long a stop is given before it becomes a kill.
    #[must_use]
    pub fn stop_grace(self, millis: u64) -> Self {
        self.set("stop_grace_ms", json!(millis))
    }

    /// Start this service only after `id`.
    #[must_use]
    pub fn depends_on(mut self, id: impl Into<String>) -> Self {
        self.overrides
            .entry("depends_on".to_owned())
            .or_insert_with(|| json!([]))
            .as_array_mut()
            .expect("depends_on is a list")
            .push(json!(id.into()));

        self
    }

    /// Add these lines to the generated arguments file verbatim.
    ///
    /// The free-form half of every recipe's overrides, and the only way a test reaches a
    /// `fakeservice` flag the recipe does not model.
    #[must_use]
    pub fn extra(self, lines: impl Into<String>) -> Self {
        self.set("extra", json!(lines.into()))
    }

    fn set(mut self, key: &str, value: serde_json::Value) -> Self {
        self.overrides.insert(key.to_owned(), value);
        self
    }

    /// What goes in the column, and what a `service.create` sends.
    pub(crate) fn overrides(&self) -> serde_json::Map<String, serde_json::Value> {
        self.overrides
            .iter()
            .map(|(key, value)| (key.clone(), value.clone()))
            .collect()
    }
}

impl From<&str> for Service {
    fn from(id: &str) -> Self {
        Self::new(id)
    }
}

/// The version the fixture package is recorded as, which is what a `service.create` names.
pub const VERSION: &str = "1.0.0";

/// Write the `packages` row every `fakeservice` service is an instance of.
///
/// **The one thing that cannot go through the API**, which is why it is still here now that
/// [`create`](mod@crate::create) exists: `fakeservice` is a fixture binary that no package index will
/// ever publish, so `package.install` has nothing to download and the row has to be written by hand.
/// Everything after it — the service itself — goes through `service.create`, exactly as a user's
/// would.
///
/// The database has to exist already: the daemon's migrations are what create the schema, so a test
/// starts a daemon (or opens a `Store`) first and calls this afterwards. Calling it twice is not an
/// error; the second call points the row at the same directory.
///
/// # Panics
///
/// If the database cannot be opened or the row cannot be written — a fixture that half worked would
/// fail later as an assertion about the daemon, which is the wrong thing to go looking at.
pub async fn package(database: &Path) {
    let pool = open(database).await;

    // Where the daemon looks for the program: `<install_path>/fakeservice`, which is how a real
    // recipe finds its server inside a real package too.
    let install_path = FakeService::program()
        .parent()
        .expect("the fixture binary is in a directory")
        .to_string_lossy()
        .into_owned();

    sqlx::query(
        "INSERT INTO packages (name, version, install_path, installed_at, source_url, sha256)
         VALUES ('fakeservice', ?, ?, '2026-08-12T00:00:00Z', 'https://example', 'ab')
         ON CONFLICT (name, version) DO UPDATE SET install_path = excluded.install_path",
    )
    .bind(VERSION)
    .bind(&install_path)
    .execute(&pool)
    .await
    .unwrap_or_else(|error| panic!("a package row for the fixture: {error}"));

    pool.close().await;
}

/// A PHP recorded as installed, carrying the extension facts an artifact publishes.
///
/// A row rather than an install: what the daemon tests are about is the state model and the wire
/// shape, and eighty megabytes of real PHP is `crates/mixengine-cli/tests/php_extensions.rs`'
/// business. `opcache` is static here and `xdebug` is shipped and off, which is exactly the pair
/// the two refusals and the `source` field are about.
///
/// # Panics
///
/// If the database cannot be opened, or the row cannot be written.
pub async fn runtime_with_extensions(database: &Path, version: &str) {
    let pool = open(database).await;

    sqlx::query(
        "INSERT INTO runtime_installs
             (kind, version, channel, install_path, installed_at, size_bytes, source_url, sha256,
              is_default, provides_json, extension_dir, extensions_json)
         VALUES ('php', ?, 'stable', ?, '2026-08-20T09:00:00Z', 1,
                 'https://example.invalid/php.tar.zst', 'abc', 1, '{\"php\":\"bin/php\"}',
                 'lib/php/extensions',
                 '{\"static\":[\"opcache\"],\"shared\":[\"redis\",\"xdebug\"],\"enabled\":[\"redis\"]}')",
    )
    .bind(version)
    .bind(format!("/runtimes/php/{version}"))
    .execute(&pool)
    .await
    .unwrap_or_else(|error| panic!("a runtime row carrying extensions: {error}"));

    pool.close().await;
}

/// A PHP recorded as installed **with the pool `pools::ensure` would have made for it** — roadmap
/// task **T81b**.
///
/// A row and not an install, for [`runtime_with_extensions`]' reason. The pool row is written here
/// rather than left to the daemon's boot-time repair because the tests that need it seed *after*
/// the daemon is up, which is the only moment the database exists.
///
/// # Panics
///
/// If the database cannot be opened, or a row cannot be written.
pub async fn php_pool(database: &Path, version: &str) {
    let pool = open(database).await;

    // **`fakeservice` stands in for `php`, `php-fpm` and `php-cgi` alike.** The front end's
    // regeneration validates a staged pool file with `php-fpm --test --fpm-config` wherever a PHP
    // publishes `php-fpm`, and the fixture binary answers that the way php-fpm does — so the walk
    // an install ends with succeeds on a system with php-fpm as on one without.
    let program = FakeService::program();
    let install_path = program
        .parent()
        .expect("the fixture binary is in a directory")
        .to_string_lossy()
        .into_owned();
    let name = program
        .file_name()
        .expect("the fixture binary has a name")
        .to_string_lossy()
        .into_owned();
    let provides = format!(r#"{{"php":"{name}","php-fpm":"{name}","php-cgi":"{name}"}}"#);

    sqlx::query(
        "INSERT INTO runtime_installs
             (kind, version, channel, install_path, installed_at, size_bytes, source_url, sha256,
              is_default, provides_json)
         VALUES ('php', ?1, 'stable', ?2, '2026-09-03T00:00:00Z', 1,
                 'https://example.invalid/php.tar.zst', 'abc', 0, ?3)",
    )
    .bind(version)
    .bind(&install_path)
    .bind(&provides)
    .execute(&pool)
    .await
    .unwrap_or_else(|error| panic!("a runtime row: {error}"));

    sqlx::query(
        "INSERT INTO services (id, runtime_install_id, instance_name, state, port)
         VALUES ('php-fpm@' || ?1,
                 (SELECT id FROM runtime_installs WHERE kind = 'php' AND version = ?1),
                 ?1, 'stopped', 9000 + (SELECT count(*) FROM services))",
    )
    .bind(version)
    .execute(&pool)
    .await
    .unwrap_or_else(|error| panic!("a pool row: {error}"));

    pool.close().await;
}

/// Every executable the three database recipes look for, in one list — roadmap task **T82**.
///
/// A recipe refuses to render a service whose install publishes none of the binaries it names, and
/// `Generator::declared` renders **every** row before it can serve any site — so a fixture database
/// with an empty `provides_json` fails an extension install that has nothing to do with it. One list
/// rather than a per-engine map, because `fakeservice` answers every one of these the same way and a
/// superset costs a fixture nothing.
const DATABASE_EXECUTABLES: [&str; 12] = [
    "mariadbd",
    "mariadb",
    "mariadb-admin",
    "mariadb-install-db",
    "mysqld",
    "mysql",
    "mysqladmin",
    "postgres",
    "initdb",
    "pg_ctl",
    "psql",
    "pg_isready",
];

/// A database recorded as installed, with the `services` row that runs out of it — roadmap task
/// **T82**.
///
/// A row and not an install, for [`php_pool`]'s reason: what a `web-app` extension needs from a
/// database is an id, a package name and a port, and starting a real MariaDB to supply three columns
/// would put a server in every test that installs phpMyAdmin.
///
/// `package` is what the recipe is called — `mariadb`, `mysql`, `postgres` — because that is what
/// `extensions::database` matches `[web-app.database].engines` against, and it is also what decides
/// the account name `{db_user}` renders to.
///
/// # Panics
///
/// If the database cannot be opened, or a row cannot be written.
pub async fn database(file: &Path, service: &str, package: &str, port: u16) {
    let pool = open(file).await;
    let instance = service.split('@').nth(1).unwrap_or("main");

    // **`fakeservice` stands in for every binary the recipe names** — [`php_pool`]'s device, and
    // needed for the same reason it is needed there: the generator renders *every* declared service
    // on the way to serving a site, so a row whose `provides_json` is empty fails the whole pass
    // with "publishes no executable called mariadbd". Found by an extension install that had nothing
    // to do with MariaDB.
    let program = FakeService::program();
    let install_path = program
        .parent()
        .expect("the fixture binary is in a directory")
        .to_string_lossy()
        .into_owned();
    let name = program
        .file_name()
        .expect("the fixture binary has a name")
        .to_string_lossy()
        .into_owned();
    let provides = DATABASE_EXECUTABLES
        .iter()
        .map(|executable| format!("\"{executable}\":\"{name}\""))
        .collect::<Vec<_>>()
        .join(",");
    let provides = format!("{{{provides}}}");

    // One `packages` row per package: two instances of one server run out of one installed copy,
    // which is what `UNIQUE (name, version)` says.
    sqlx::query(
        "INSERT INTO packages (name, version, install_path, installed_at, source_url, sha256,
                               provides_json)
         VALUES (?1, '1.0.0', ?2, '2026-09-03T00:00:00Z',
                 'https://example.invalid/x.tar.zst', 'abc', ?3)
         ON CONFLICT (name, version) DO UPDATE SET name = excluded.name",
    )
    .bind(package)
    .bind(&install_path)
    .bind(&provides)
    .execute(&pool)
    .await
    .unwrap_or_else(|error| panic!("a package row: {error}"));

    sqlx::query(
        "INSERT INTO services (id, package_id, instance_name, state, port, bind_addr)
         VALUES (?1,
                 (SELECT id FROM packages WHERE name = ?2 AND version = '1.0.0'),
                 ?3, 'stopped', ?4, '127.0.0.1')",
    )
    .bind(service)
    .bind(package)
    .bind(instance)
    .bind(i64::from(port))
    .execute(&pool)
    .await
    .unwrap_or_else(|error| panic!("a database service row: {error}"));

    pool.close().await;
}

/// Put `overrides` in `id`'s `config_overrides_json`, whatever they say.
///
/// **The way a test produces a home the daemon cannot answer for.** Overrides are the one part of a
/// `services` row a person edits, and the daemon refuses a document that names a setting no recipe
/// has rather than ignoring it — so this is also how a suite reaches the "the source cannot say what
/// is declared" path that a shutdown, a listing and a walk each have to survive. Nothing validates
/// what is passed here, deliberately: a fixture that refused what the daemon refuses could not stage
/// the case at all.
///
/// # Panics
///
/// If the database cannot be opened, or if there is no such service — a fixture that half worked.
pub async fn reconfigure(database: &Path, id: &str, overrides: &str) {
    let pool = open(database).await;

    let updated = sqlx::query("UPDATE services SET config_overrides_json = ? WHERE id = ?")
        .bind(overrides)
        .bind(id)
        .execute(&pool)
        .await
        .unwrap_or_else(|error| panic!("`{id}` can be reconfigured: {error}"));

    assert_eq!(
        updated.rows_affected(),
        1,
        "there is no services row for `{id}` to reconfigure"
    );

    pool.close().await;
}

/// Move a service to another port, the way nothing in the shipped product yet can.
///
/// `service.configure` does not exist — changing a row is still a direct edit, which is what this
/// module is for. It is here rather than in a suite because the *reason* is general: a port a test
/// did not choose is a port that may already be taken on the machine running it, and a fixture that
/// cannot rebind can only hope. A php-fpm pool on Windows is the first such row: its port is
/// allocated from 9000 by the install, so the suite that drives it could not pick one up front the
/// way a suite that calls `service.create` picks Caddy's.
///
/// # Panics
///
/// If the database cannot be opened, or if there is no such service — a fixture that half worked.
pub async fn rebind(database: &Path, id: &str, port: u16) {
    let pool = open(database).await;

    let updated = sqlx::query("UPDATE services SET port = ? WHERE id = ?")
        .bind(i64::from(port))
        .bind(id)
        .execute(&pool)
        .await
        .unwrap_or_else(|error| panic!("`{id}` can be rebound: {error}"));

    assert_eq!(
        updated.rows_affected(),
        1,
        "there is no services row for `{id}` to rebind"
    );

    pool.close().await;
}

/// [`reconfigure`], for a test that has no runtime of its own.
///
/// # Panics
///
/// As [`reconfigure`], and if a runtime cannot be started.
pub fn reconfigure_blocking(database: &Path, id: &str, overrides: &str) {
    tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .expect("a current-thread runtime")
        .block_on(reconfigure(database, id, overrides));
}

/// The database a fixture writes into, opened the one way every function in this crate opens it.
///
/// Read-write and never `create_if_missing`: an empty file where the schema should be is a test that
/// pointed at the wrong path, and creating one would turn that into "no such table".
///
/// One connection, and a busy timeout because the daemon under test is holding the same file. WAL
/// lets a writer and readers coexist; two writers still take turns.
pub(crate) async fn open(database: &Path) -> sqlx::SqlitePool {
    let options = SqliteConnectOptions::new()
        .filename(database)
        .create_if_missing(false);

    SqlitePoolOptions::new()
        .max_connections(1)
        .connect_with(options)
        .await
        .unwrap_or_else(|error| panic!("{} is a MixEngine database: {error}", database.display()))
}

/// Record that `id` is running as `pid`, a process that began at `started` — the row a **killed**
/// daemon leaves behind.
///
/// The subject of roadmap task **T18**, and the one state no test can reach by asking a daemon for
/// it: a daemon that is running writes this row and then clears it on its way out, whichever way it
/// is asked to stop. What crash recovery meets is what is left when it is given no way out at all,
/// and the only way to hand a *new* daemon one of those is to write it here.
///
/// `started` is a `mixengine_platform::process::StartTime` as it is stored, and the pair is what
/// makes the row identifiable: a pid on its own is reused within minutes. A test that wants the
/// other case — a row whose process is gone — writes the pair of a process it has since killed, or
/// one whose start time does not match.
///
/// `declare` has to have been called for `id` first, since this only moves a row that exists.
///
/// # Panics
///
/// If the database cannot be opened, or if there is no such service to update — both of which are a
/// fixture that half worked, and would fail later as an assertion about the daemon.
pub async fn running(database: &Path, id: &str, pid: u32, started: i64) {
    let pool = open(database).await;

    let updated = sqlx::query(
        "UPDATE services
         SET state = 'running', pid = ?, pid_start_time = ?, last_started_at = ?
         WHERE id = ?",
    )
    .bind(i64::from(pid))
    .bind(started)
    .bind(now())
    .bind(id)
    .execute(&pool)
    .await
    .unwrap_or_else(|error| panic!("`{id}` can be recorded as running: {error}"));

    assert_eq!(
        updated.rows_affected(),
        1,
        "there is no services row for `{id}` to record a process against"
    );

    pool.close().await;
}

/// [`running`], for a test that has no runtime of its own.
///
/// # Panics
///
/// As [`running`], and if a runtime cannot be started.
pub fn running_blocking(database: &Path, id: &str, pid: u32, started: i64) {
    tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .expect("a current-thread runtime")
        .block_on(running(database, id, pid, started));
}

/// Epoch milliseconds, which is what `services.last_started_at` holds.
///
/// Restated here rather than taken from `mixengine_proto::Timestamp`, on the rule this crate follows
/// everywhere else: a fixture that computed a value the way the daemon computes it would make a
/// suite agree with itself by construction.
fn now() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|since| i64::try_from(since.as_millis()).unwrap_or(i64::MAX))
        .unwrap_or_default()
}

/// [`package`], for a test that has no runtime of its own.
///
/// The end-to-end suites drive `mix` through [`std::process::Command`] and are plain `#[test]`
/// functions; building a runtime for one insert is cheaper than making every one of them `async`.
///
/// # Panics
///
/// As [`package`], and if a runtime cannot be started.
pub fn package_blocking(database: &Path) {
    tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .expect("a current-thread runtime")
        .block_on(package(database));
}
