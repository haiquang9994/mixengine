//! The queue of privileged operations, the document that carries them to `mixengine-elevate`, and
//! the report it leaves behind.
//!
//! **This crate owns the row and the document; the daemon owns the prompt.** The same cut
//! [`crate::jobs`] documents one table across: nothing here has a loop, a clock or a task, and
//! nothing here spawns a process. What that buys is the test the daemon cannot have —
//! `mixengine-daemon` is a binary crate with no library target, so with the document here a test in
//! `tests/elevation.rs` can build a request with the shipped code, run the **real** helper under an
//! ordinary token, and read the report back with the shipped code. See the T40b design, D3.

use std::path::{Path, PathBuf};

use mixengine_proto::privileged::{OpOutcome, PrivilegedOp};
use mixengine_proto::{PendingOp, PendingOpId, Timestamp};

use crate::{Error, Result, Store};

/// The canonical form of an operation: what the `dedupe_key` column holds.
///
/// Today this is exactly its serialisation, and the two columns hold the same bytes. It is a
/// function of its own because canonical and serialised part company the moment an operation carries
/// a set: T41's `HostsApply` is a list of domains whose order must not make two requests for one
/// change into two rows, and sorting it belongs here rather than in a migration.
///
/// # Errors
///
/// [`Error::OpUnwritable`], which cannot happen — a [`PrivilegedOp`] is one of ours and holds
/// nothing serde can refuse. Mapped rather than unwrapped, because nothing in this crate panics.
fn canonical(op: &PrivilegedOp) -> Result<String> {
    serde_json::to_string(op).map_err(|source| Error::OpUnwritable { source })
}

/// Put an operation in the queue, and hand back the whole queue when that changed something.
///
/// [`None`] means the operation was already waiting: the machine's needs did not change, so there is
/// nothing to announce and the daemon publishes no
/// [`ElevationRequired`](mixengine_proto::DaemonEvent::ElevationRequired). See the T40b design, D8.
///
/// The list is read back **inside the transaction that inserted**, on
/// [`services::transition`](crate::services::transition)'s rule: what is announced is what survived
/// the write, so the row and the event cannot disagree.
///
/// `at` is passed in rather than read from the clock, as everywhere else in this crate: the caller
/// already has a reading, and a test needs to be able to say when.
///
/// # Errors
///
/// [`Error::OpUnwritable`] when the operation cannot be encoded, and [`Error::Database`] when the row
/// cannot be written.
pub async fn enqueue(
    store: &Store,
    op: &PrivilegedOp,
    at: Timestamp,
) -> Result<Option<Vec<PendingOp>>> {
    let (encoded, key, requested) = (canonical(op)?, canonical(op)?, at.0);

    // `BEGIN IMMEDIATE` for `jobs::progress`' reason: the insert decides whether the read below
    // happens at all, and a deferred `BEGIN` would leave a write to upgrade a read snapshot, which
    // WAL refuses outright without even running the busy handler.
    let mut tx = store
        .pool()
        .begin_with("BEGIN IMMEDIATE")
        .await
        .map_err(|source| store.failure("write", source))?;

    let written = sqlx::query!(
        "INSERT INTO pending_privileged_ops (op, dedupe_key, requested_at)
         VALUES (?, ?, ?)
         ON CONFLICT (dedupe_key) DO NOTHING",
        encoded,
        key,
        requested
    )
    .execute(&mut *tx)
    .await
    .map_err(|source| store.failure("write", source))?;

    if written.rows_affected() == 0 {
        // Rolled back by being dropped. `ON CONFLICT DO NOTHING` is what the statement says, but it
        // is the `UNIQUE` index that makes a second writer unable to break the rule by forgetting
        // the clause.
        return Ok(None);
    }

    let waiting = read(store, &mut tx).await?;

    tx.commit()
        .await
        .map_err(|source| store.failure("write", source))?;

    tracing::info!(
        op = op.name(),
        waiting = waiting.len(),
        "an operation is waiting for permission"
    );

    Ok(Some(waiting))
}

/// Everything waiting, oldest first.
///
/// **A writer as well as a reader**, for the one case D2 names: a row this build cannot decode is
/// deleted and logged rather than carried. Filtering it instead would leave it to be met, and
/// warned about, on every call for the life of the home — and a row no installed build can act on is
/// a degraded mode nobody can ever clear.
///
/// # Errors
///
/// [`Error::Database`] when the table cannot be read, or when an undecodable row cannot be removed.
pub async fn pending(store: &Store) -> Result<Vec<PendingOp>> {
    let mut tx = store
        .pool()
        .begin_with("BEGIN IMMEDIATE")
        .await
        .map_err(|source| store.failure("write", source))?;

    let waiting = read(store, &mut tx).await?;

    tx.commit()
        .await
        .map_err(|source| store.failure("write", source))?;

    Ok(waiting)
}

/// Read the queue inside a transaction somebody else opened, dropping what cannot be decoded.
async fn read(
    store: &Store,
    tx: &mut sqlx::Transaction<'_, sqlx::Sqlite>,
) -> Result<Vec<PendingOp>> {
    let rows = sqlx::query!("SELECT id, op, requested_at FROM pending_privileged_ops ORDER BY id")
        .fetch_all(&mut **tx)
        .await
        .map_err(|source| store.failure("read", source))?;

    let mut waiting = Vec::with_capacity(rows.len());
    let mut undecodable = Vec::new();

    for row in rows {
        match serde_json::from_str::<PrivilegedOp>(&row.op) {
            Ok(op) => waiting.push(PendingOp {
                id: PendingOpId(row.id),
                description: op.describe(),
                op,
                requested_at: Timestamp(row.requested_at),
            }),
            Err(error) => {
                tracing::warn!(
                    id = row.id,
                    op = row.op,
                    %error,
                    "a pending privileged operation this build cannot act on was removed"
                );
                undecodable.push(row.id);
            }
        }
    }

    for id in undecodable {
        sqlx::query!("DELETE FROM pending_privileged_ops WHERE id = ?", id)
            .execute(&mut **tx)
            .await
            .map_err(|source| store.failure("write", source))?;
    }

    Ok(waiting)
}

/// Forget one operation, or all of them, and say how many rows went.
///
/// `discard` and not `drop`: a free function called `drop` in a module every caller imports shadows
/// the one in the prelude, and the confusion is not worth the symmetry with the wire verb.
///
/// Forgetting something that is not there is **not** an error — the caller wanted it gone and it is.
///
/// # Errors
///
/// [`Error::Database`] when the rows cannot be removed.
pub async fn discard(store: &Store, which: Option<PendingOpId>) -> Result<usize> {
    let removed = match which {
        Some(PendingOpId(id)) => {
            sqlx::query!("DELETE FROM pending_privileged_ops WHERE id = ?", id)
                .execute(store.pool())
                .await
        }
        None => {
            sqlx::query!("DELETE FROM pending_privileged_ops")
                .execute(store.pool())
                .await
        }
    }
    .map_err(|source| store.failure("write", source))?;

    Ok(usize::try_from(removed.rows_affected()).unwrap_or(usize::MAX))
}

/// What one grant did to the queue.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Settled {
    /// How many operations came back done — [`OpOutcome::Applied`] or [`OpOutcome::AlreadyDone`].
    pub applied: usize,

    /// The ones that will never succeed as written, and why, so the job's result can say so once.
    ///
    /// [`OpOutcome::Refused`] and [`OpOutcome::Unsupported`] together: their rows go for the same
    /// reason and what a person needs from either is the sentence.
    pub refused: Vec<(PendingOpId, String)>,

    /// How many rows are still there — the [`OpOutcome::Failed`] ones.
    pub kept: usize,
}

/// Apply a helper's report to the queue.
///
/// **Four outcomes delete the row and one keeps it** — the T40b design, D5:
///
/// | [`OpOutcome`] | The row | Why |
/// | --- | --- | --- |
/// | [`Applied`](OpOutcome::Applied) | deleted | done |
/// | [`AlreadyDone`](OpOutcome::AlreadyDone) | deleted | the machine is in the state that was asked for; that is the same outcome |
/// | [`Refused`](OpOutcome::Refused) | deleted | "the caller's fault, and the same request will be refused again". A row that cannot ever succeed and is never removed is a permanent degraded mode nobody can clear |
/// | [`Unsupported`](OpOutcome::Unsupported) | deleted | the installed helper does not know this operation, and it is excluded from auto-update, so it will not learn |
/// | [`Failed`](OpOutcome::Failed) | **kept** | "the OS refused. Trying again may work; nothing about the request is wrong" |
///
/// The distinction is `mixengine-proto`'s already; what this function contributes is not blurring
/// it. One transaction, so a report is applied whole or not at all.
///
/// # Errors
///
/// [`Error::Database`] when the rows cannot be removed.
pub async fn settle(store: &Store, results: &[(PendingOpId, OpOutcome)]) -> Result<Settled> {
    let mut tx = store
        .pool()
        .begin_with("BEGIN IMMEDIATE")
        .await
        .map_err(|source| store.failure("write", source))?;

    let mut settled = Settled::default();

    for (id, outcome) in results {
        match outcome {
            OpOutcome::Applied { .. } | OpOutcome::AlreadyDone => settled.applied += 1,
            OpOutcome::Refused { reason } | OpOutcome::Unsupported { reason } => {
                settled.refused.push((*id, reason.clone()));
            }
            OpOutcome::Failed { .. } => {
                settled.kept += 1;
                continue;
            }
        }

        let row = id.0;
        sqlx::query!("DELETE FROM pending_privileged_ops WHERE id = ?", row)
            .execute(&mut *tx)
            .await
            .map_err(|source| store.failure("write", source))?;
    }

    tx.commit()
        .await
        .map_err(|source| store.failure("write", source))?;

    Ok(settled)
}

/// Where `mixengine-elevate` is, given the program that is asking.
///
/// **Beside whatever is running, and there is no override** — the T40b design, D9. A setting that
/// chooses which file is run as root is a setting that chooses which file is run as root; the
/// directory beside `mixengined` is already exactly as trustworthy as `mixengined` itself, which is
/// the trust boundary `.claude/architecture/security-model.md` and ADR 0005 both already accept.
/// D3's split is what removes the reason anyone would want one: the round trip is testable in this
/// crate without a prompt, so no test needs to redirect what the daemon spawns.
///
/// The same shape as [`shims::source`](crate::shims::source), and for the same reason: a release
/// ships the binaries in one directory, and a `PATH` search would find something else.
///
/// # Errors
///
/// [`Error::ElevateMissing`] when there is no such file, which the daemon answers as
/// `dependency_missing`: nothing can be granted, and the fix is a reinstall rather than a retry.
pub fn helper(program: &Path) -> Result<PathBuf> {
    let beside = program.parent().unwrap_or_else(|| Path::new("."));
    let helper = beside.join(format!("mixengine-elevate{}", std::env::consts::EXE_SUFFIX));

    match helper.is_file() {
        true => Ok(helper),
        false => Err(Error::ElevateMissing { path: helper }),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A store on a temporary file, migrated, with nothing in this table.
    async fn store() -> (tempfile::TempDir, Store) {
        let home = tempfile::tempdir().expect("a temporary home");
        let store = Store::open(&home.path().join("mixengine.db"))
            .await
            .expect("a fresh database migrates");

        (home, store)
    }

    const WHEN: Timestamp = Timestamp(1_760_000_000_000);
    const LATER: Timestamp = Timestamp(1_760_000_900_000);

    #[tokio::test]
    async fn the_first_enqueue_hands_back_the_whole_queue() {
        let (_home, store) = store().await;

        let waiting = enqueue(&store, &PrivilegedOp::Probe {}, WHEN)
            .await
            .expect("the row is written")
            .expect("a first enqueue changed something");

        assert_eq!(waiting.len(), 1);
        assert_eq!(waiting[0].op, PrivilegedOp::Probe {});
        assert_eq!(waiting[0].description, PrivilegedOp::Probe {}.describe());
        assert_eq!(waiting[0].requested_at, WHEN);
    }

    /// D2, and the whole of "no code path elevates in a loop" at rest: the schema is what refuses,
    /// so a producer that enqueues on every start writes one row without having to know it.
    #[tokio::test]
    async fn the_same_operation_asked_for_twice_is_one_row_with_the_first_moment() {
        let (_home, store) = store().await;

        enqueue(&store, &PrivilegedOp::Probe {}, WHEN)
            .await
            .unwrap();
        let again = enqueue(&store, &PrivilegedOp::Probe {}, LATER)
            .await
            .expect("a conflicting insert is not an error");

        assert!(
            again.is_none(),
            "nothing changed, so there is nothing to announce"
        );

        let waiting = pending(&store).await.unwrap();
        assert_eq!(waiting.len(), 1);
        assert_eq!(
            waiting[0].requested_at, WHEN,
            "the surviving row keeps the moment the machine first needed this"
        );
    }

    #[tokio::test]
    async fn dropping_one_leaves_the_others_and_dropping_all_empties_it() {
        let (_home, store) = store().await;

        enqueue(&store, &PrivilegedOp::Probe {}, WHEN)
            .await
            .unwrap();
        let only = pending(&store).await.unwrap()[0].id;

        assert_eq!(discard(&store, Some(only)).await.unwrap(), 1);
        assert!(pending(&store).await.unwrap().is_empty());

        // Dropping something that is not there is not an error: the caller wanted it gone and it is.
        assert_eq!(discard(&store, Some(only)).await.unwrap(), 0);

        enqueue(&store, &PrivilegedOp::Probe {}, LATER)
            .await
            .unwrap();
        assert_eq!(discard(&store, None).await.unwrap(), 1);
        assert!(pending(&store).await.unwrap().is_empty());
    }

    /// D2's last paragraph. The only way to make one of these is to downgrade the daemon underneath
    /// its own database, and a row no installed build can act on is a degraded mode nobody can ever
    /// clear — so it is deleted and logged rather than carried.
    #[tokio::test]
    async fn a_row_this_build_cannot_decode_is_removed_rather_than_carried() {
        let (_home, store) = store().await;

        enqueue(&store, &PrivilegedOp::Probe {}, WHEN)
            .await
            .unwrap();
        sqlx::query(
            "INSERT INTO pending_privileged_ops (op, dedupe_key, requested_at)
             VALUES ('{\"op\":\"trust-ca-install\",\"der\":[1,2,3]}', 'future', 1)",
        )
        .execute(store.pool())
        .await
        .expect("a row from a build that knew more than this one");

        let waiting = pending(&store).await.unwrap();

        assert_eq!(waiting.len(), 1, "only the one this build can act on");
        assert_eq!(waiting[0].op, PrivilegedOp::Probe {});

        // And it is gone rather than merely hidden — a reader that filtered would find it again on
        // every call and log the same warning for the life of the home.
        let left: i64 = sqlx::query_scalar("SELECT count(*) FROM pending_privileged_ops")
            .fetch_one(store.pool())
            .await
            .unwrap();
        assert_eq!(left, 1);
    }

    /// D9: beside the program that went looking, and nowhere else. The refusal is what
    /// `elevation.grant` turns into `dependency_missing`.
    #[test]
    fn a_helper_that_is_not_beside_the_daemon_is_named_rather_than_searched_for() {
        let directory = tempfile::tempdir().expect("a temporary directory");
        let program = directory.path().join("mixengined");

        let error = helper(&program).expect_err("nothing was put there");

        assert!(matches!(error, Error::ElevateMissing { .. }), "{error}");
        assert!(
            error
                .to_string()
                .contains(&directory.path().display().to_string()),
            "the message has to say where it looked: {error}"
        );
    }

    /// D5's table, one row at a time. Four outcomes delete and exactly one is kept — the only one
    /// whose own type in `mixengine-proto` says retrying is meaningful.
    #[tokio::test]
    async fn only_the_outcome_that_says_try_again_keeps_its_row() {
        use mixengine_proto::privileged::OpOutcome;

        for (outcome, survives) in [
            (
                OpOutcome::Applied {
                    detail: "wrote two lines".to_owned(),
                },
                false,
            ),
            (OpOutcome::AlreadyDone, false),
            (
                OpOutcome::Refused {
                    reason: "outside the home".to_owned(),
                },
                false,
            ),
            (
                OpOutcome::Unsupported {
                    reason: "this helper is older".to_owned(),
                },
                false,
            ),
            (
                OpOutcome::Failed {
                    message: "the file was locked".to_owned(),
                },
                true,
            ),
        ] {
            let (_home, store) = store().await;
            enqueue(&store, &PrivilegedOp::Probe {}, WHEN)
                .await
                .unwrap();
            let only = pending(&store).await.unwrap()[0].id;

            settle(&store, &[(only, outcome.clone())]).await.unwrap();

            assert_eq!(
                !pending(&store).await.unwrap().is_empty(),
                survives,
                "{outcome:?}"
            );
        }
    }

    /// What the counts are for: the job's result says how many operations came back done, which of
    /// them will never succeed and why, and what is left in the queue afterwards. A client cannot
    /// compute the third from the first two, because refused and failed part company.
    #[tokio::test]
    async fn a_settlement_counts_what_a_person_is_told_afterwards() {
        use mixengine_proto::privileged::OpOutcome;

        let (_home, store) = store().await;

        // Three distinct rows: `Probe` is the only operation this build has, so the other two are
        // written directly — which is also the shape T41 will produce.
        enqueue(&store, &PrivilegedOp::Probe {}, WHEN)
            .await
            .unwrap();
        for (key, at) in [("second", 2), ("third", 3)] {
            sqlx::query(
                "INSERT INTO pending_privileged_ops (op, dedupe_key, requested_at) \
                 VALUES ('{\"op\":\"probe\"}', ?, ?)",
            )
            .bind(key)
            .bind(at)
            .execute(store.pool())
            .await
            .expect("a second and third row");
        }

        let waiting = pending(&store).await.unwrap();
        assert_eq!(waiting.len(), 3);

        let settled = settle(
            &store,
            &[
                (waiting[0].id, OpOutcome::AlreadyDone),
                (
                    waiting[1].id,
                    OpOutcome::Refused {
                        reason: "outside the home".to_owned(),
                    },
                ),
                (
                    waiting[2].id,
                    OpOutcome::Failed {
                        message: "the file was locked".to_owned(),
                    },
                ),
            ],
        )
        .await
        .unwrap();

        assert_eq!(settled.applied, 1);
        assert_eq!(settled.kept, 1);
        assert_eq!(settled.refused.len(), 1);
        assert_eq!(settled.refused[0].0, waiting[1].id);
        assert!(settled.refused[0].1.contains("outside the home"));

        assert_eq!(pending(&store).await.unwrap().len(), 1);
    }
}
