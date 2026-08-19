//! The `services` table: what the daemon believes about each supervised service between restarts.
//!
//! This module owns every write to that table. Most of it is the columns a *supervisor* writes:
//! what T14 owns is the state machine and the guarantee that a transition is either persisted *and*
//! announced or neither, and what T19 added beside it is [`started`] and [`ended`], the two writes
//! that turn a running process into a row the adoption after a daemon restart (T18) can meet.
//!
//! **T31a added the row's own beginning and end** — [`create`] and [`delete`] — which until then
//! were a comment here saying Phase 3 would bring them. What is *decided* about a new service still
//! happens above: which recipe, whether the package is installed, whether the id's shape suits the
//! recipe's instancing. What is here is the insert and the delete, so that the table has one writer.
//!
//! **T32 gave the row a second possible parent.** A service used to be an instance of a `packages`
//! row and nothing else; php-fpm is an instance of a `runtime_installs` one, because the process
//! serving a user's sites lives inside the PHP they installed. [`Origin`] is which of the two a
//! caller means, `services` carries both columns with a `CHECK` that exactly one is set, and the
//! foreign key is what lets `runtime.uninstall` refuse to remove a PHP a pool still points at.
//!
//! `last_started_at` is still not written here, but the question T14 left open is now closed: the
//! column holds epoch milliseconds — a [`mixengine_proto::Timestamp`] verbatim — rather than the
//! ISO-8601 text it was first declared as. It is read back by the supervisor on every exit to place
//! a restart inside or outside the crash-loop window, which makes it a moment the daemon does
//! arithmetic on rather than one a person reads; storing it as text would have bought a date library
//! this workspace needs for nothing else, to parse on the hot path of a restart. The other `_at`
//! columns stay text because nothing branches on them. `0001_initial.sql` was edited rather than
//! migrated for the same reason T14 edited it: nothing has shipped, so forward-only has nothing yet
//! to protect.

use std::collections::BTreeMap;

use mixengine_proto::{
    PackageVersion, RuntimeKind, ServiceId, ServiceState, ServiceTransition, StateReason, Timestamp,
};

use crate::{Error, Result, Store};

pub mod graph;
pub mod pools;

pub use graph::{GraphError, Plan, ServiceGraph};

/// What the database says this service is doing.
///
/// # Errors
///
/// [`Error::NotFound`] when there is no such service; [`Error::UnknownServiceState`] when the row
/// holds a word this build does not recognise; [`Error::Database`] when the file cannot be read.
pub async fn state(store: &Store, service: &ServiceId) -> Result<ServiceState> {
    let id = service.as_str();

    let stored = sqlx::query_scalar!("SELECT state FROM services WHERE id = ?", id)
        .fetch_optional(store.pool())
        .await
        .map_err(|source| store.failure("read", source))?
        .ok_or_else(|| Error::NotFound {
            kind: "service",
            id: id.to_owned(),
        })?;

    parse_state(service, stored)
}

/// Everything a `services` row says about the process behind a service.
///
/// The columns a supervisor writes, read back in one value. What is **not** in it is the reason for
/// the state: no column holds one, because a reason explains a *move* and the row keeps only where
/// the machine ended up. `DaemonEvent::ServiceStateChanged` is what carries the why, as it happens.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ServiceRecord {
    /// What the row says the service is doing.
    pub state: ServiceState,

    /// The process it is running as, where there is one. Cleared by [`ended`].
    pub pid: Option<u32>,

    /// When that process began, as the OS counts such moments — a
    /// [`StartTime`](mixengine_platform::process::StartTime) stored verbatim.
    ///
    /// **Half of an identity and useless on its own**, which is why it travels beside the pid and is
    /// cleared with it. Crash recovery (T18) is the only reader: it asks the OS when the process
    /// bearing `pid` began and compares the two, because a pid the machine has handed out again
    /// names somebody else's program. A row with a pid and no start time is one adoption refuses —
    /// see [`started`].
    ///
    /// An `i64` rather than the platform type: this crate stores what it is given and does not
    /// interpret it, and the column is `.claude/architecture/data-model.md`'s "exists to be
    /// compared, never read".
    pub pid_start_time: Option<i64>,

    /// When it was last started, whether or not it is still running.
    pub last_started_at: Option<Timestamp>,

    /// What its process exited with the last time one ended.
    pub last_exit_code: Option<i32>,
}

/// Where the binary a service runs comes from.
///
/// **Two tables, one of which is not a package** — T32. Everything up to php-fpm was installed from
/// the index by `package.install` and has a `packages` row; a pool has no such row and must not be
/// given a fake one, because the directory it runs out of belongs to `runtime.install` and is
/// removed by `runtime.uninstall`. Which one a service names is what the `CHECK` on `services`
/// enforces, and this enum is that constraint said in Rust, so a caller cannot even assemble the row
/// the database would refuse.
#[derive(Debug, Clone)]
pub enum Origin {
    /// A `packages` row: Caddy, MariaDB, Redis — anything the signed index publishes as a server.
    Package {
        /// `packages.name`, as the caller resolved it from the id.
        ///
        /// Passed rather than read off the id, because the caller has already held it to the
        /// catalogue and this is that answer rather than a second derivation of it.
        name: String,

        /// Which installed version of that package to run.
        version: PackageVersion,
    },

    /// A `runtime_installs` row: php-fpm, whose process lives inside an installed PHP.
    Runtime {
        /// Which language.
        kind: RuntimeKind,

        /// Which installed version of it, in full — `8.3.33` and not `8.3`, because
        /// `runtime_installs` is `UNIQUE (kind, version)` over the full version and two patch
        /// releases of one minor can both be installed.
        version: PackageVersion,
    },
}

/// Everything a new `services` row is made of.
///
/// Taken as one value rather than as nine arguments, on
/// [`packages::Installation`](crate::packages::Installation)'s reasoning: most of them are optional
/// and of similar types, and a caller assembling them positionally would produce a row that is wrong
/// rather than one that fails to insert.
#[derive(Debug, Clone)]
pub struct Declaration {
    /// Which service, which is also which package it is an instance of.
    pub service: ServiceId,

    /// Which table supplies the binary, and which row in it.
    pub origin: Origin,

    /// What goes in `instance_name`, which `UNIQUE (package_id, instance_name)` is enforced over.
    ///
    /// The half after the `@` for a package that has instances, and the package's own name for one
    /// that exists once — a decision that belongs to the recipe and so arrives made.
    pub instance_name: String,

    /// The port it listens on, or [`None`] for a recipe that renders none.
    pub port: Option<u16>,

    /// The address it binds, or [`None`] for the column's own `127.0.0.1`.
    pub bind_addr: Option<String>,

    /// Where its data lives, or [`None`] for the home's own layout.
    pub data_dir: Option<String>,

    /// Whether it starts with the daemon.
    pub autostart: bool,

    /// The settings this instance overrides, as the document the column holds.
    pub overrides: String,
}

/// Write down a service somebody asked for.
///
/// **The row and nothing else.** Whether the package has a recipe, whether that version is
/// installed and whether the id's shape suits the recipe are decided by the caller, and rendering
/// the configuration happens after this returns — which is what makes [`delete`] the rollback for a
/// rendering that failed.
///
/// # Errors
///
/// [`Error::ServiceAlreadyDeclared`] when a row with this id exists or the parent already has an
/// instance of this name, [`Error::NotFound`] when the [`Origin`] names a package or a runtime that
/// is not installed, and [`Error::Database`] when the row cannot be written.
pub async fn create(store: &Store, declaration: &Declaration) -> Result<()> {
    let Declaration {
        service,
        origin,
        instance_name,
        port,
        bind_addr,
        data_dir,
        autostart,
        overrides,
    } = declaration;

    let id = service.as_str();
    let port_column = port.map(i64::from);
    let autostart_column = i64::from(*autostart);

    // Checked here as well as by the caller, because the alternative is a constraint violation
    // whose message names a column: the row and the lookup are one statement otherwise, and a
    // subquery that found nothing is not a failure SQLite explains.
    let (package_id, runtime_install_id) = match origin {
        Origin::Package { name, version } => {
            let version_column = version.as_str();

            let found: Option<i64> = sqlx::query_scalar!(
                "SELECT id FROM packages WHERE name = ? AND version = ?",
                name,
                version_column
            )
            .fetch_optional(store.pool())
            .await
            .map_err(|source| store.failure("read", source))?;

            let found = found.ok_or_else(|| Error::NotFound {
                kind: "package",
                id: format!("{name} {version}"),
            })?;

            (Some(found), None)
        }

        Origin::Runtime { kind, version } => {
            let kind_column = kind.as_str();
            let version_column = version.as_str();

            let found: Option<i64> = sqlx::query_scalar!(
                "SELECT id FROM runtime_installs WHERE kind = ? AND version = ?",
                kind_column,
                version_column
            )
            .fetch_optional(store.pool())
            .await
            .map_err(|source| store.failure("read", source))?;

            let found = found.ok_or_else(|| Error::NotFound {
                kind: "runtime",
                id: format!("{kind_column} {version_column}"),
            })?;

            (None, Some(found))
        }
    };

    let written = sqlx::query!(
        "INSERT INTO services
             (id, package_id, runtime_install_id, instance_name, state, autostart, port, bind_addr,
              data_dir, config_overrides_json)
         VALUES (?, ?, ?, ?, 'stopped', ?, ?, COALESCE(?, '127.0.0.1'), ?, ?)
         ON CONFLICT DO NOTHING",
        id,
        package_id,
        runtime_install_id,
        instance_name,
        autostart_column,
        port_column,
        bind_addr,
        data_dir,
        overrides
    )
    .execute(store.pool())
    .await
    .map_err(|source| store.failure("write", source))?;

    // `DO NOTHING` over every unique constraint rather than letting one raise: the id, and one
    // `(parent, instance_name)` per kind of parent — and what a person did wrong is the same in all
    // of those cases, they asked for a service that is already here.
    if written.rows_affected() == 0 {
        return Err(Error::ServiceAlreadyDeclared {
            service: service.clone(),
        });
    }

    tracing::info!(%id, origin = ?origin, "a service was created");

    Ok(())
}

/// Remove a service's row, and say what its `data_dir` column held.
///
/// **The column verbatim, not the directory it resolves to.** [`None`] is a row that left the
/// placement to the home's layout, and only the generator knows what that layout made of it — so the
/// caller reconstructs it, and this stays a function about a table.
///
/// # Errors
///
/// [`Error::NotFound`] when there is no such row, and [`Error::Database`] when it cannot be written.
pub async fn delete(store: &Store, service: &ServiceId) -> Result<Option<String>> {
    let id = service.as_str();

    let removed = sqlx::query_scalar!("DELETE FROM services WHERE id = ? RETURNING data_dir", id)
        .fetch_optional(store.pool())
        .await
        .map_err(|source| store.failure("write", source))?
        .ok_or_else(|| Error::NotFound {
            kind: "service",
            id: id.to_owned(),
        })?;

    tracing::info!(%id, "a service was deleted");

    Ok(removed)
}

/// One service's row.
///
/// # Errors
///
/// As [`state`], whose narrower question this answers as well: [`Error::NotFound`] when there is no
/// such service, [`Error::UnknownServiceState`] when the row holds a word this build does not
/// recognise, [`Error::Database`] when the file cannot be read.
pub async fn record(store: &Store, service: &ServiceId) -> Result<ServiceRecord> {
    let id = service.as_str();

    let row = sqlx::query!(
        "SELECT state, pid, pid_start_time, last_started_at, last_exit_code
         FROM services WHERE id = ?",
        id
    )
    .fetch_optional(store.pool())
    .await
    .map_err(|source| store.failure("read", source))?
    .ok_or_else(|| Error::NotFound {
        kind: "service",
        id: id.to_owned(),
    })?;

    Ok(ServiceRecord {
        state: parse_state(service, row.state)?,
        pid: process_id(row.pid),
        pid_start_time: row.pid_start_time,
        last_started_at: row.last_started_at.map(Timestamp),
        last_exit_code: exit_code(row.last_exit_code),
    })
}

/// Every service's row, keyed by the id the row itself holds.
///
/// **One query rather than one per service**, because the caller is answering `service.list` and a
/// question per declared service would be a round trip per declared service to say the same thing.
///
/// The key is the stored string and not a [`ServiceId`]: the caller already holds the ids it is
/// asking about — they came from the declarations, not from here — and parsing a column back into
/// one would give a hand-edited row the power to fail a listing that does not even mention it.
///
/// # Errors
///
/// [`Error::UnknownServiceState`] when a row holds a state word this build does not recognise, and
/// [`Error::Database`] when the file cannot be read. Not [`Error::NotFound`]: a home with no
/// services has no rows, which is an answer and not a failure.
pub async fn records(store: &Store) -> Result<BTreeMap<String, ServiceRecord>> {
    let rows = sqlx::query!(
        "SELECT id, state, pid, pid_start_time, last_started_at, last_exit_code FROM services"
    )
    .fetch_all(store.pool())
    .await
    .map_err(|source| store.failure("read", source))?;

    rows.into_iter()
        .map(|row| {
            let state =
                ServiceState::parse(&row.state).ok_or_else(|| Error::UnknownServiceState {
                    service: row.id.clone(),
                    value: row.state,
                })?;

            Ok((
                row.id,
                ServiceRecord {
                    state,
                    pid: process_id(row.pid),
                    pid_start_time: row.pid_start_time,
                    last_started_at: row.last_started_at.map(Timestamp),
                    last_exit_code: exit_code(row.last_exit_code),
                },
            ))
        })
        .collect()
}

/// A stored pid, or [`None`] where the column holds something no pid could be.
///
/// [`started`] writes an `i64` widened from a `u32`, so the narrowing here cannot lose one this
/// build wrote. A value that does not fit is a hand-edited row, and "no process" is the safer of the
/// two readings available: the alternative is handing a number to something that will signal it.
fn process_id(stored: Option<i64>) -> Option<u32> {
    stored.and_then(|pid| u32::try_from(pid).ok())
}

/// The same, for an exit code — [`ended`] writes an `i32`.
fn exit_code(stored: Option<i64>) -> Option<i32> {
    stored.and_then(|code| i32::try_from(code).ok())
}

/// Move a service to `to`, and hand back the transition that was written.
///
/// The return value is the whole point: it is a [`ServiceTransition`], which is also what
/// [`mixengine_proto::DaemonEvent::ServiceStateChanged`] carries. The caller publishes the value
/// this function persisted rather than describing the same event a second time, so the row and the
/// event cannot drift — and because it only comes back on success, an event is impossible to
/// publish for a transition that did not happen.
///
/// `at` is passed in rather than read from the clock here for the same reason
/// [`Timestamp::from_system_time`] takes a [`std::time::SystemTime`]: the caller already has a
/// reading, and a test needs to be able to say when.
///
/// **The read and the write are one `BEGIN IMMEDIATE` transaction.** Two supervisors racing — a
/// health check going `Degraded` while a user's `service.stop` arrives — must not both see `Running`
/// and both write, and the plain `BEGIN` sqlx issues by default would not stop them: it takes no
/// lock, so the `SELECT` only pins a read snapshot and the `UPDATE` that follows has to *upgrade* to
/// a writer. In WAL mode that upgrade fails with `SQLITE_BUSY_SNAPSHOT` the moment anybody else has
/// committed since the snapshot — and SQLite deliberately does **not** run the busy handler for it,
/// because no amount of waiting can resolve it while this transaction still holds its old read. The
/// [`crate::Store`]'s `busy_timeout` would be bypassed and the loser would get a bare database
/// error instead of an answer.
///
/// `BEGIN IMMEDIATE` takes the write lock up front, so the two supervisors serialise at the `BEGIN`
/// — where `busy_timeout` does apply — and the second one reads the state the first one committed
/// and re-judges its move against it. The `WHERE state = ?` on the `UPDATE` stays as a cheap
/// assertion that this is really so; [`Error::StateRaced`] is what it reports if it ever is not.
///
/// # Errors
///
/// [`Error::NotFound`] when there is no such service; [`Error::IllegalTransition`] when the machine
/// has no such edge, which is a bug in the caller rather than a condition to handle;
/// [`Error::StateRaced`] when something else changed the state in between;
/// [`Error::UnknownServiceState`] when the row holds a word this build does not recognise; and
/// [`Error::Database`] when the file cannot be written — including when the write lock could not be
/// taken within the store's `busy_timeout`.
pub async fn transition(
    store: &Store,
    service: &ServiceId,
    to: ServiceState,
    reason: StateReason,
    at: Timestamp,
) -> Result<ServiceTransition> {
    let id = service.as_str();

    // Not `begin()`: that is a deferred `BEGIN`, which would leave the `UPDATE` below to upgrade a
    // read snapshot into a write and fail unrecoverably against any concurrent writer. See above.
    let mut tx = store
        .pool()
        .begin_with("BEGIN IMMEDIATE")
        .await
        .map_err(|source| store.failure("write", source))?;

    let stored = sqlx::query_scalar!("SELECT state FROM services WHERE id = ?", id)
        .fetch_optional(&mut *tx)
        .await
        .map_err(|source| store.failure("read", source))?
        .ok_or_else(|| Error::NotFound {
            kind: "service",
            id: id.to_owned(),
        })?;

    let from = parse_state(service, stored)?;

    if !from.can_become(to) {
        return Err(Error::IllegalTransition {
            service: id.to_owned(),
            from,
            to,
        });
    }

    let (next, current) = (to.as_str(), from.as_str());
    let updated = sqlx::query!(
        "UPDATE services SET state = ? WHERE id = ? AND state = ?",
        next,
        id,
        current
    )
    .execute(&mut *tx)
    .await
    .map_err(|source| store.failure("write", source))?;

    if updated.rows_affected() == 0 {
        return Err(Error::StateRaced {
            service: id.to_owned(),
            expected: from,
        });
    }

    tx.commit()
        .await
        .map_err(|source| store.failure("write", source))?;

    tracing::info!(service = id, %from, to = %to, ?reason, "service state changed");

    Ok(ServiceTransition {
        service: service.clone(),
        from,
        to,
        reason,
        at,
    })
}

/// Record the process this service is now running as.
///
/// **The pair is the point.** A pid on its own is not an identity — the OS reuses the number within
/// minutes — so what makes a row adoptable after a daemon restart (T18) is `pid` *and* the moment
/// that process began, read by
/// [`Supervised::started_at`](mixengine_platform::process::Supervised::started_at) while the child
/// is still held. `pid_start_time` is [`None`] where that reading could not be made — a process that
/// ended in its first milliseconds is the ordinary case — and a null column is the honest answer for
/// it: adoption refuses a row it cannot identify, where a zero would look like a reading.
///
/// Separate from [`transition`] rather than folded into it, because these are facts about a process
/// and not an edge in the machine: the state was already written when the service reached
/// `Starting`, and a spawn is what happens after that.
///
/// # Errors
///
/// [`Error::NotFound`] when there is no such service, and [`Error::Database`] when the file cannot
/// be written.
pub async fn started(
    store: &Store,
    service: &ServiceId,
    pid: u32,
    pid_start_time: Option<i64>,
    at: Timestamp,
) -> Result<()> {
    let id = service.as_str();
    let (pid, at) = (i64::from(pid), at.0);

    let updated = sqlx::query!(
        "UPDATE services SET last_started_at = ?, pid = ?, pid_start_time = ? WHERE id = ?",
        at,
        pid,
        pid_start_time,
        id
    )
    .execute(store.pool())
    .await
    .map_err(|source| store.failure("write", source))?;

    if updated.rows_affected() == 0 {
        return Err(Error::NotFound {
            kind: "service",
            id: id.to_owned(),
        });
    }

    Ok(())
}

/// Record that the process is gone, and what it exited with.
///
/// Clearing `pid` and `pid_start_time` is the half that matters and is why this is not optional
/// bookkeeping: a row that keeps a dead pid is a row the next daemon adopts, and the number will by
/// then belong to something else. `code` is [`None`] where the OS reports none — a Unix process
/// killed by a signal — for the same reason [`mixengine_proto::StateReason::Exited`] carries an
/// option there: writing `0` for it would say "clean exit" about a crash.
///
/// # Errors
///
/// As [`started`].
pub async fn ended(store: &Store, service: &ServiceId, code: Option<i32>) -> Result<()> {
    let id = service.as_str();

    let updated = sqlx::query!(
        "UPDATE services SET pid = NULL, pid_start_time = NULL, last_exit_code = ? WHERE id = ?",
        code,
        id
    )
    .execute(store.pool())
    .await
    .map_err(|source| store.failure("write", source))?;

    if updated.rows_affected() == 0 {
        return Err(Error::NotFound {
            kind: "service",
            id: id.to_owned(),
        });
    }

    Ok(())
}

/// Turn the stored word into a state, blaming the row rather than the reader.
///
/// The `CHECK` constraint on the column means nothing this build wrote can land here, so a failure
/// is a database edited by hand or written by a version that knew a state this one does not — and
/// naming the service is what makes either one findable.
fn parse_state(service: &ServiceId, stored: String) -> Result<ServiceState> {
    ServiceState::parse(&stored).ok_or_else(|| Error::UnknownServiceState {
        service: service.as_str().to_owned(),
        value: stored,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A `services` row, without the Phase 3 machinery that will eventually create one.
    ///
    /// A service cannot exist without a parent even in a test — the `CHECK` demands one of the two
    /// columns and `foreign_keys=ON` demands the row it names — which is the constraint doing its
    /// job, not an obstacle to route around.
    async fn service_row(store: &Store, id: &str, state: ServiceState) -> ServiceId {
        sqlx::query(
            "INSERT INTO packages (name, version, install_path, installed_at, source_url, sha256)
             VALUES (?, '1.0.0', '/packages/x', '2026-08-12T00:00:00Z', 'https://example', 'ab')
             ON CONFLICT (name, version) DO NOTHING",
        )
        .bind(id)
        .execute(store.pool())
        .await
        .expect("a package for the service to belong to");

        sqlx::query(
            "INSERT INTO services (id, package_id, instance_name, state)
             VALUES (?, (SELECT id FROM packages WHERE name = ?), 'main', ?)",
        )
        .bind(id)
        .bind(id)
        .bind(state.as_str())
        .execute(store.pool())
        .await
        .expect("the service row");

        ServiceId::parse(id).expect("a valid id")
    }

    async fn store() -> (tempfile::TempDir, Store) {
        let home = tempfile::tempdir().expect("a temporary directory");
        let store = Store::open(&home.path().join(crate::paths::DATABASE_FILE_NAME))
            .await
            .expect("a database");
        (home, store)
    }

    const NOW: Timestamp = Timestamp(1_760_000_000_000);

    #[tokio::test]
    async fn a_transition_is_written_and_handed_back() {
        let (_home, store) = store().await;
        let id = service_row(&store, "caddy", ServiceState::Stopped).await;

        let change = transition(
            &store,
            &id,
            ServiceState::Starting,
            StateReason::Requested,
            NOW,
        )
        .await
        .expect("stopped can start");

        assert_eq!(change.from, ServiceState::Stopped);
        assert_eq!(change.to, ServiceState::Starting);
        assert_eq!(change.service, id);
        assert_eq!(change.at, NOW);

        assert_eq!(
            state(&store, &id).await.expect("the row"),
            ServiceState::Starting,
            "the value handed back is the value that survived the commit"
        );
    }

    #[tokio::test]
    async fn an_edge_the_machine_does_not_have_leaves_the_row_alone() {
        let (_home, store) = store().await;
        let id = service_row(&store, "caddy", ServiceState::Stopped).await;

        let error = transition(&store, &id, ServiceState::Running, StateReason::Ready, NOW)
            .await
            .expect_err("a stopped service cannot be running without starting");

        assert!(
            matches!(
                error,
                Error::IllegalTransition {
                    from: ServiceState::Stopped,
                    to: ServiceState::Running,
                    ..
                }
            ),
            "{error:?}"
        );
        assert_eq!(
            state(&store, &id).await.expect("the row"),
            ServiceState::Stopped,
            "the refused transition rolled back rather than half-applying"
        );
    }

    #[tokio::test]
    async fn a_service_that_is_not_there_is_named_as_such() {
        let (_home, store) = store().await;
        let id = ServiceId::parse("mariadb@main").expect("a valid id");

        let error = state(&store, &id).await.expect_err("no such row");

        assert!(
            matches!(&error, Error::NotFound { kind: "service", id } if id == "mariadb@main"),
            "{error:?}"
        );
    }

    /// The constraint added in `0001_initial.sql` and the enum have to agree, or one of them is
    /// decoration: every state the machine can be in must be storable, and nothing else may be.
    #[tokio::test]
    async fn the_column_accepts_every_state_and_nothing_else() {
        let (_home, store) = store().await;

        for state in ServiceState::ALL {
            service_row(&store, &format!("svc-{state}"), state).await;
        }

        let refused = sqlx::query(
            "INSERT INTO services (id, package_id, instance_name, state)
             VALUES ('bogus', (SELECT id FROM packages LIMIT 1), 'main', 'crashed')",
        )
        .execute(store.pool())
        .await;

        assert!(
            refused.is_err(),
            "the CHECK let a word through that ServiceState cannot read back"
        );
    }

    /// The pair, and the clearing of it, are one round trip: what T18 adopts is what T19 wrote.
    #[tokio::test]
    async fn the_process_a_service_is_running_as_is_written_and_then_cleared() {
        let (_home, store) = store().await;
        let id = service_row(&store, "caddy", ServiceState::Starting).await;

        started(&store, &id, 4321, Some(1_234_567), NOW)
            .await
            .expect("the row takes a pid");

        let (pid, start_time, at): (Option<i64>, Option<i64>, Option<i64>) = sqlx::query_as(
            "SELECT pid, pid_start_time, last_started_at FROM services WHERE id = ?",
        )
        .bind(id.as_str())
        .fetch_one(store.pool())
        .await
        .expect("the row");

        assert_eq!(pid, Some(4321));
        assert_eq!(at, Some(NOW.0), "epoch milliseconds, not text");
        assert_eq!(
            start_time,
            Some(1_234_567),
            "the reading is stored as it was given, since only the OS that made it can read it"
        );

        ended(&store, &id, Some(3)).await.expect("the row lets go");

        let (pid, start_time, code): (Option<i64>, Option<i64>, Option<i64>) =
            sqlx::query_as("SELECT pid, pid_start_time, last_exit_code FROM services WHERE id = ?")
                .bind(id.as_str())
                .fetch_one(store.pool())
                .await
                .expect("the row");

        assert_eq!(
            pid, None,
            "a row that kept a dead pid is a row the next daemon would adopt"
        );
        assert_eq!(
            start_time, None,
            "and half an identity is one the next daemon would have to guess at"
        );
        assert_eq!(code, Some(3));
    }

    #[tokio::test]
    async fn recording_a_process_against_a_service_that_is_not_there_names_it() {
        let (_home, store) = store().await;
        let id = ServiceId::parse("caddy").expect("a valid id");

        let error = started(&store, &id, 1, None, NOW)
            .await
            .expect_err("no such row");

        assert!(
            matches!(&error, Error::NotFound { kind: "service", id } if id == "caddy"),
            "{error:?}"
        );
    }

    /// What a hand-edited database looks like from in here.
    ///
    /// Asked of the function rather than through a doctored database on purpose. The `CHECK` above
    /// makes the row unreachable through any write of ours, so producing one would mean disabling
    /// the constraint with `PRAGMA writable_schema` — which is per *connection*, and this store
    /// hands out four of them. That test would be about SQLite's schema cache; this one is about
    /// what the reader does when the word does not parse, which is the part we wrote.
    #[test]
    fn a_state_this_build_does_not_know_blames_the_row() {
        let id = ServiceId::parse("caddy").expect("a valid id");

        let error = parse_state(&id, "crashed".to_owned()).expect_err("not a service state");

        assert!(
            matches!(&error, Error::UnknownServiceState { service, value }
                if service == "caddy" && value == "crashed"),
            "{error:?}"
        );
    }
    /// A row whose binary comes from an installed runtime rather than from a package.
    ///
    /// The whole of T32's schema change seen from the only place that writes it: `create` resolves
    /// `runtime_installs` instead of `packages`, and the row that lands names one parent.
    #[tokio::test]
    async fn a_service_can_come_from_a_runtime_install() {
        let (_home, store) = store().await;

        sqlx::query(
            "INSERT INTO runtime_installs
                 (kind, version, channel, install_path, installed_at, size_bytes, source_url, sha256)
             VALUES ('php', '8.3.33', 'stable', '/runtimes/php/8.3.33', '2026-08-19T00:00:00Z',
                     1, 'https://example.invalid/php', 'abc')",
        )
        .execute(store.pool())
        .await
        .expect("a runtime install");

        let service = ServiceId::parse("php-fpm@8.3.33").expect("a valid id");

        create(
            &store,
            &Declaration {
                service,
                origin: Origin::Runtime {
                    kind: RuntimeKind::Php,
                    version: PackageVersion::parse("8.3.33").expect("a version"),
                },
                instance_name: "8.3.33".to_owned(),
                port: None,
                bind_addr: None,
                data_dir: None,
                autostart: false,
                overrides: "{}".to_owned(),
            },
        )
        .await
        .expect("a pool for an installed PHP");

        let (package_id, runtime_install_id): (Option<i64>, Option<i64>) = sqlx::query_as(
            "SELECT package_id, runtime_install_id FROM services WHERE id = 'php-fpm@8.3.33'",
        )
        .fetch_one(store.pool())
        .await
        .expect("the row that was written");

        assert_eq!(package_id, None, "a pool has no package to point at");
        assert!(
            runtime_install_id.is_some(),
            "the row points at the PHP it runs out of"
        );
    }

    /// The `CHECK` is the whole guarantee that [`Origin`] is not a suggestion.
    ///
    /// Written through raw SQL rather than through [`create`], because `create` cannot express
    /// either of these — which is the point: what is being asserted is that a hand-edited database,
    /// or a future writer nobody has written yet, cannot express them either.
    #[tokio::test]
    async fn a_row_names_one_parent_and_not_two_and_not_none() {
        let (_home, store) = store().await;

        sqlx::query(
            "INSERT INTO packages (name, version, install_path, installed_at, source_url, sha256)
             VALUES ('caddy', '2.11.4', '/packages/caddy', '2026-08-19T00:00:00Z',
                     'https://example.invalid/caddy', 'ab')",
        )
        .execute(store.pool())
        .await
        .expect("a package");

        sqlx::query(
            "INSERT INTO runtime_installs
                 (kind, version, channel, install_path, installed_at, size_bytes, source_url, sha256)
             VALUES ('php', '8.3.33', 'stable', '/runtimes/php/8.3.33', '2026-08-19T00:00:00Z',
                     1, 'https://example.invalid/php', 'abc')",
        )
        .execute(store.pool())
        .await
        .expect("a runtime install");

        let both = sqlx::query(
            "INSERT INTO services (id, package_id, runtime_install_id, instance_name, state)
             VALUES ('both', (SELECT id FROM packages LIMIT 1),
                     (SELECT id FROM runtime_installs LIMIT 1), 'both', 'stopped')",
        )
        .execute(store.pool())
        .await;
        assert!(both.is_err(), "a service with two parents was accepted");

        let neither = sqlx::query(
            "INSERT INTO services (id, instance_name, state) VALUES ('orphan', 'orphan', 'stopped')",
        )
        .execute(store.pool())
        .await;
        assert!(neither.is_err(), "a service with no parent was accepted");
    }
}
