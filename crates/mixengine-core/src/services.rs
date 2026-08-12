//! The `services` table: what the daemon believes about each supervised service between restarts.
//!
//! This module owns exactly one column so far — `state` — and that narrowness is deliberate. The
//! row itself is created by `service.create` in Phase 3, which is what knows about packages, ports
//! and data directories; `pid` and `pid_start_time` are written by whatever spawns a process (T15)
//! and read by the adoption that follows a daemon restart (T18). What T14 owns is the state machine
//! and the guarantee that a transition is either persisted *and* announced or neither.
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

use mixengine_proto::{ServiceId, ServiceState, ServiceTransition, StateReason, Timestamp};

use crate::{Error, Result, Store};

pub mod graph;

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
    /// The foreign key to `packages` is `NOT NULL` and enforced (`foreign_keys=ON`), so a service
    /// cannot exist without a package even in a test — which is the constraint doing its job, not
    /// an obstacle to route around.
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
}
