//! Which service already holds a data directory — roadmap task **T36**.
//!
//! A port two services share is a start that fails, diagnosed by naming whoever holds it (T38). A
//! *data directory* two database servers share is two servers over one set of files: MariaDB and
//! MySQL both refuse it through a lock file when they notice, and what happens when they do not is
//! the user's databases. The cost lands on the data rather than on the start, so it is refused
//! where the row is written rather than discovered where the files are opened.
//!
//! **Only an explicit `data_dir` can reach this.** The layout the generator derives is
//! `data/<package>/<instance>` and two instances of one package cannot collide in it — so this is
//! about `mix service create --data-dir`, where the path is a person's to choose and two of them
//! can be the same path spelled two ways.

use std::path::{Path, PathBuf};

use crate::{Result, Store};

/// The id of the service already pointing at `path`, or [`None`] when it is free.
///
/// The id as the column spells it rather than a parsed
/// [`ServiceId`](mixengine_proto::ServiceId): this is read straight out of a row to be put in a
/// message, nothing acts on it, and a hand-edited database holding an id this build cannot parse
/// still has a directory somebody is using.
///
/// # Errors
///
/// [`Error::Database`](crate::Error::Database) when the table cannot be read.
pub(super) async fn held_by(store: &Store, path: &str) -> Result<Option<String>> {
    let wanted = compare_as(path);

    let rows = sqlx::query!("SELECT id, data_dir FROM services WHERE data_dir IS NOT NULL")
        .fetch_all(store.pool())
        .await
        .map_err(|source| store.failure("read", source))?;

    for row in rows {
        let Some(held) = row.data_dir else { continue };

        if compare_as(&held) == wanted {
            return Ok(Some(row.id));
        }
    }

    Ok(None)
}

/// The form two spellings of one path have in common.
///
/// **A guard against an accident, not a proof that two paths are two directories.** It resolves a
/// relative path against the working directory, which is what catches `--data-dir db` beside the
/// absolute path the GUI wrote for the same place; `.` segments need nothing, because
/// [`PathBuf`] compares components and drops those itself.
///
/// What it does not catch is a symlink, a bind mount, or one directory reached through two cases on
/// a filesystem that ignores case. Answering *those* means asking the OS whether two paths are one
/// file, which is a `mixengine-platform` capability nothing has needed yet — and being too lenient
/// here fails by this refusal not firing, which leaves the server's own lock file where it already
/// was rather than removing a guarantee.
///
/// A path the OS will not absolutise — an empty string, or one with a null in it — is compared as
/// it was written. It cannot become a directory either, so the create behind it fails at rendering,
/// with a message about the path rather than about this.
fn compare_as(path: &str) -> PathBuf {
    let path = Path::new(path);

    std::path::absolute(path).unwrap_or_else(|_| path.to_path_buf())
}
