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

use mixengine_proto::privileged::{
    OpOutcome, PrivilegedOp, PrivilegedRequest, PrivilegedResponse, RESPONSE_FILE_NAME,
};
use mixengine_proto::{PendingOp, PendingOpId, Timestamp};

use crate::{Error, Result, Store};

/// The operation as the `op` column holds it: its serialisation, and nothing else.
///
/// **Not the `dedupe_key` any more.** T40b wrote one value into both columns, which is right for an
/// operation carrying no state and wrong for a whole-state one: two `hosts-apply` rows disagreeing
/// about what the file should hold would both be valid. The key is now the operation's *identity*
/// and is [`PrivilegedOp::dedupe_key`]'s to answer — see the T41 design, D2.
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
/// **A whole-state operation supersedes the one that was waiting** rather than queueing beside it —
/// the T41 design, D2. `requested_at` is deliberately not refreshed: the need started when it
/// started, and a queue that reset its own clock on every site creation would report a wait that
/// never got older. The `WHERE` clause is what preserves [`None`]'s meaning: re-enqueueing the same
/// state touches no row, so nothing is announced.
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
    let (encoded, key, requested) = (canonical(op)?, op.dedupe_key(), at.0);

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
         ON CONFLICT (dedupe_key) DO UPDATE SET op = excluded.op WHERE op <> excluded.op",
        encoded,
        key,
        requested
    )
    .execute(&mut *tx)
    .await
    .map_err(|source| store.failure("write", source))?;

    if written.rows_affected() == 0 {
        // Rolled back by being dropped. The `WHERE` clause is what the statement says, but it is the
        // `UNIQUE` index that makes a second writer unable to break the rule by forgetting it.
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
            // `Unmanaged` settles beside `Applied`, and that is a decision rather than a
            // shorthand — T74. The operation is finished: this machine has no mechanism for it, so
            // the row must not be kept for a retry that would answer the same thing forever. What
            // the user has to know instead travels back with the *share*, which renders the manual
            // command; the queue's job here is only to stop asking.
            OpOutcome::Applied { .. } | OpOutcome::AlreadyDone | OpOutcome::Unmanaged { .. } => {
                settled.applied += 1;
            }
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

/// The name the request takes inside its own directory.
///
/// Not in `mixengine-proto` beside [`RESPONSE_FILE_NAME`]: the helper is *given* this path as its one
/// argument and never composes it, so it is the writer's name for a file rather than part of the
/// protocol. The response's name is the protocol, because that one is agreed rather than passed.
const REQUEST_FILE_NAME: &str = "request.json";

/// A request lying on disk, and what it is an answer to.
///
/// Holds the nonce and the rows so that [`read_report`] checks the report against **this** request
/// rather than against something a caller remembered — and so that the daemon can zip outcomes back
/// onto rows without keeping a second list in step.
#[derive(Debug)]
pub struct Request {
    /// The single-use directory. Removed by the caller when the grant ends, on every branch.
    directory: PathBuf,

    /// The document, inside it.
    path: PathBuf,

    /// Echoed by the helper, and checked on the way back.
    nonce: String,

    /// The rows this batch was built from, in the order their outcomes will arrive.
    ids: Vec<PendingOpId>,
}

impl Request {
    /// The document's path — the helper's one argument.
    #[must_use]
    pub fn path(&self) -> &Path {
        &self.path
    }

    /// The single-use directory holding it.
    #[must_use]
    pub fn directory(&self) -> &Path {
        &self.directory
    }

    /// What the helper will echo back.
    #[must_use]
    pub fn nonce(&self) -> &str {
        &self.nonce
    }

    /// The rows, in the order their outcomes arrive.
    #[must_use]
    pub fn ids(&self) -> &[PendingOpId] {
        &self.ids
    }
}

/// Write one batch into a fresh single-use directory.
///
/// **The directory is single-use by construction**, which is what makes `response.json`'s existence
/// a sufficient anti-replay check (T40/D10): nothing else is ever written beside a request, so a
/// request with an answer next to it has been processed and the helper refuses it.
///
/// The nonce comes from the OS's random source through
/// [`generate_secret`](mixengine_platform::generate_secret) rather than from a counter or a clock: a
/// daemon restarted twice in a second must not be able to produce two requests the helper cannot
/// tell apart.
///
/// # Errors
///
/// [`Error::ElevateRequestEmpty`] when `ops` is empty — the helper refuses an empty batch outright,
/// with no response file and exit 65, so it is refused here where the message can say why;
/// [`Error::Platform`] when the OS will not produce random bytes; [`Error::OpUnwritable`] when an
/// operation cannot be encoded; and [`Error::Io`] naming the file that could not be written.
pub fn write_request(directory: &Path, home: &Path, ops: &[PendingOp]) -> Result<Request> {
    if ops.is_empty() {
        return Err(Error::ElevateRequestEmpty);
    }

    crate::paths::create_dir(directory)?;

    let nonce = mixengine_platform::generate_secret(32)?;

    let encoded = ops
        .iter()
        .map(|waiting| {
            serde_json::to_value(&waiting.op).map_err(|source| Error::OpUnwritable { source })
        })
        .collect::<Result<Vec<_>>>()?;

    let body = PrivilegedRequest {
        version: mixengine_proto::PROTOCOL_VERSION,
        home: home.to_path_buf(),
        nonce: nonce.clone(),
        ops: encoded,
    };

    let path = directory.join(REQUEST_FILE_NAME);
    let text = serde_json::to_vec(&body).map_err(|source| Error::OpUnwritable { source })?;

    std::fs::write(&path, text).map_err(|source| Error::Io {
        action: "write",
        path: path.clone(),
        source,
    })?;

    Ok(Request {
        directory: directory.to_path_buf(),
        path,
        nonce,
        ids: ops.iter().map(|waiting| waiting.id).collect(),
    })
}

/// Read the report the helper left beside `request`, and check that it is one.
///
/// Three checks, and each of them is the reason a later step can be simple: the nonce, so an answer
/// to an earlier request cannot be read as the answer to this one; the protocol version; and one
/// outcome per operation, which is what lets the caller zip [`Request::ids`] against
/// [`PrivilegedResponse::results`] without wondering.
///
/// # Errors
///
/// [`Error::ElevateReportMissing`] when there is nothing beside the request — **a real state and not
/// an impossibility**: `Completed` means the helper ran, not that it left a report, because a crash
/// is not a per-OS event. [`Error::ElevateReportUnreadable`] when it is not JSON this build can read,
/// [`Error::ElevateReportMismatched`] when it answers something else, and [`Error::Io`] when the file
/// is there and cannot be read.
pub fn read_report(request: &Request) -> Result<PrivilegedResponse> {
    let path = request.directory.join(RESPONSE_FILE_NAME);

    let text = match std::fs::read_to_string(&path) {
        Ok(text) => text,
        Err(source) if source.kind() == std::io::ErrorKind::NotFound => {
            return Err(Error::ElevateReportMissing { path });
        }
        Err(source) => {
            return Err(Error::Io {
                action: "read",
                path,
                source,
            });
        }
    };

    let response: PrivilegedResponse =
        serde_json::from_str(&text).map_err(|source| Error::ElevateReportUnreadable {
            path: path.clone(),
            source,
        })?;

    if response.nonce != request.nonce {
        return Err(Error::ElevateReportMismatched {
            path,
            why: "it answers a different request".to_owned(),
        });
    }

    // Normally unreachable: the helper refuses a request whose version is not its own, and a refused
    // request leaves no response at all. Asserted anyway, because the one way to reach it is a
    // response file that is not the helper's.
    if response.version != mixengine_proto::PROTOCOL_VERSION {
        return Err(Error::ElevateReportMismatched {
            path,
            why: format!(
                "it speaks protocol {} and this daemon speaks {}",
                response.version.0,
                mixengine_proto::PROTOCOL_VERSION.0
            ),
        });
    }

    if response.results.len() != request.ids.len() {
        return Err(Error::ElevateReportMismatched {
            path,
            why: format!(
                "{} operations were sent and {} outcomes came back",
                request.ids.len(),
                response.results.len()
            ),
        });
    }

    Ok(response)
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

    /// A pending operation, without a database — everything below is about the document.
    fn one_waiting(id: i64) -> PendingOp {
        let op = PrivilegedOp::Probe {};

        PendingOp {
            id: PendingOpId(id),
            description: op.describe(),
            op,
            requested_at: WHEN,
        }
    }

    #[test]
    fn a_request_is_written_where_the_helper_will_look_for_it() {
        let home = tempfile::tempdir().expect("a temporary home");
        let directory = home.path().join("run").join("elevate").join("one");

        let request = write_request(&directory, home.path(), &[one_waiting(1), one_waiting(2)])
            .expect("the document is written");

        assert_eq!(request.path(), directory.join("request.json"));
        assert_eq!(request.ids(), [PendingOpId(1), PendingOpId(2)]);
        assert!(!request.nonce().is_empty());

        let written: mixengine_proto::privileged::PrivilegedRequest =
            serde_json::from_slice(&std::fs::read(request.path()).unwrap()).unwrap();

        assert_eq!(written.version, mixengine_proto::PROTOCOL_VERSION);
        assert_eq!(written.home, home.path());
        assert_eq!(written.ops.len(), 2);
        assert_eq!(written.ops[0]["op"], "probe");
    }

    /// Two grants must never write into one directory: `response.json`'s existence is the whole of
    /// the anti-replay check, so a nonce that repeated would make the second request unanswerable.
    #[test]
    fn two_requests_never_share_a_nonce() {
        let home = tempfile::tempdir().expect("a temporary home");

        let first = write_request(&home.path().join("a"), home.path(), &[one_waiting(1)]).unwrap();
        let second = write_request(&home.path().join("b"), home.path(), &[one_waiting(1)]).unwrap();

        assert_ne!(first.nonce(), second.nonce());
    }

    /// The helper refuses an empty batch outright — no response file, exit 65 — so this is refused
    /// here, where the message can say what actually happened.
    #[test]
    fn a_request_with_nothing_in_it_is_refused_before_it_is_written() {
        let home = tempfile::tempdir().expect("a temporary home");

        let error = write_request(&home.path().join("empty"), home.path(), &[])
            .expect_err("an empty batch asks for nothing");

        assert!(matches!(error, Error::ElevateRequestEmpty), "{error}");
    }

    /// T40a is explicit that `Completed` means the helper *ran*, not that it left a report — a crash
    /// is not a per-OS event. So this is a state, not an impossibility, and it has its own error.
    #[test]
    fn a_request_with_no_report_beside_it_says_exactly_that() {
        let home = tempfile::tempdir().expect("a temporary home");
        let request =
            write_request(&home.path().join("one"), home.path(), &[one_waiting(1)]).unwrap();

        let error = read_report(&request).expect_err("nothing was written beside it");

        assert!(
            matches!(error, Error::ElevateReportMissing { .. }),
            "{error}"
        );
    }

    #[test]
    fn a_report_answering_another_request_is_refused() {
        let home = tempfile::tempdir().expect("a temporary home");
        let directory = home.path().join("one");
        let request = write_request(&directory, home.path(), &[one_waiting(1)]).unwrap();

        std::fs::write(
            directory.join(mixengine_proto::privileged::RESPONSE_FILE_NAME),
            serde_json::to_vec(&mixengine_proto::privileged::PrivilegedResponse {
                version: mixengine_proto::PROTOCOL_VERSION,
                elevate_version: "0.1.0".to_owned(),
                nonce: "somebody else's".to_owned(),
                elevated: true,
                supported_ops: vec!["probe".to_owned()],
                audit_log: std::path::PathBuf::from("/var/log/mixengine/elevate.log"),
                results: vec![OpOutcome::AlreadyDone],
            })
            .unwrap(),
        )
        .unwrap();

        let error = read_report(&request).expect_err("the nonce does not match");

        assert!(
            matches!(error, Error::ElevateReportMismatched { .. }),
            "{error}"
        );
    }

    /// One outcome per operation, at the same index, is what `settle` rests on — a short report would
    /// otherwise silently leave the last row of the batch untouched.
    #[test]
    fn a_report_with_the_wrong_number_of_outcomes_is_refused() {
        let home = tempfile::tempdir().expect("a temporary home");
        let directory = home.path().join("one");
        let request =
            write_request(&directory, home.path(), &[one_waiting(1), one_waiting(2)]).unwrap();

        std::fs::write(
            directory.join(mixengine_proto::privileged::RESPONSE_FILE_NAME),
            serde_json::to_vec(&mixengine_proto::privileged::PrivilegedResponse {
                version: mixengine_proto::PROTOCOL_VERSION,
                elevate_version: "0.1.0".to_owned(),
                nonce: request.nonce().to_owned(),
                elevated: true,
                supported_ops: vec!["probe".to_owned()],
                audit_log: std::path::PathBuf::from("/var/log/mixengine/elevate.log"),
                results: vec![OpOutcome::AlreadyDone],
            })
            .unwrap(),
        )
        .unwrap();

        let error = read_report(&request).expect_err("two were sent and one came back");

        assert!(
            matches!(error, Error::ElevateReportMismatched { .. }),
            "{error}"
        );
    }

    /// D2: two sites created before anybody clicks Allow are **one** row, holding the second state.
    ///
    /// Two rows would both be valid, would disagree, and would both be rendered on the one screen
    /// whose whole job is to say what is about to happen.
    #[tokio::test]
    async fn a_newer_hosts_state_supersedes_the_one_that_was_waiting() {
        let (_directory, store) = store().await;

        let first = PrivilegedOp::hosts_apply([entry("blog.test")]);
        let second = PrivilegedOp::hosts_apply([entry("blog.test"), entry("shop.test")]);

        assert!(
            enqueue(&store, &first, Timestamp(1_000))
                .await
                .unwrap()
                .is_some()
        );

        let announced = enqueue(&store, &second, Timestamp(2_000))
            .await
            .unwrap()
            .expect("a different state is a change and is announced");

        assert_eq!(announced.len(), 1, "one row, not two: {announced:?}");
        assert_eq!(announced[0].op, second);
        assert_eq!(
            announced[0].requested_at,
            Timestamp(1_000),
            "the need started when it started; a queue that reset its own clock would report a \
             wait that never got older"
        );
    }

    /// The `WHERE` clause is what keeps `rows_affected` meaning what T40b's caller reads it as:
    /// re-enqueueing the same desired state touches no row and announces nothing.
    #[tokio::test]
    async fn re_enqueueing_the_same_hosts_state_announces_nothing() {
        let (_directory, store) = store().await;
        let op = PrivilegedOp::hosts_apply([entry("blog.test")]);

        assert!(
            enqueue(&store, &op, Timestamp(1_000))
                .await
                .unwrap()
                .is_some()
        );
        assert!(
            enqueue(&store, &op, Timestamp(2_000))
                .await
                .unwrap()
                .is_none()
        );
    }

    /// `Probe` keeps the key it had, which is what makes this need no migration.
    #[tokio::test]
    async fn probes_key_is_unchanged_so_no_existing_row_moves() {
        let (_directory, store) = store().await;

        assert!(
            enqueue(&store, &PrivilegedOp::Probe {}, Timestamp(1))
                .await
                .unwrap()
                .is_some()
        );
        assert!(
            enqueue(&store, &PrivilegedOp::Probe {}, Timestamp(2))
                .await
                .unwrap()
                .is_none()
        );
    }

    fn entry(domain: &str) -> mixengine_proto::privileged::HostEntry {
        mixengine_proto::privileged::HostEntry {
            address: "127.0.0.1".parse().expect("a literal address"),
            domain: domain.to_owned(),
        }
    }
}
