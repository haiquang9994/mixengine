//! The `runtime_installs` table, and the two facts about a language that are not the index's.
//!
//! This module owns every write to that table and nothing else — the same division
//! [`crate::jobs`] draws over the work a job does: what downloads eighty megabytes is
//! [`crate::install`], what decides to is the daemon, and what is written down afterwards is here.
//!
//! # A row exists only for a directory that does
//!
//! The order every caller follows is: install into place, *then* write the row; remove the
//! directory, *then* delete the row. Neither half is a transaction across the two, because there is
//! no such thing — SQLite cannot roll back a rename. What the ordering buys is that the failure
//! modes are the survivable ones: a directory with no row is invisible and costs disk, a row with no
//! directory is a runtime that fails the moment somebody uses it. The first is a repair, the second
//! is a bug report.
//!
//! # Two facts that belong to the language rather than to the index
//!
//! [`directory`] and [`smoke_test`]. Where a version lands is MixEngine's layout and not the
//! publisher's, and which flag prints a version is a fact about PHP and Node — which is exactly why
//! [`install::SmokeTest`](crate::install::SmokeTest) arrives as an argument rather than being
//! decided down there. Everything else about an artifact comes out of the signed index.

use std::path::{Path, PathBuf};

use mixengine_proto::{
    RuntimeChannel, RuntimeKind, RuntimeSummary, RuntimeVersion, Timestamp, VersionError,
};

use crate::install::SmokeTest;
use crate::{Error, Paths, Result, Store};

/// Where one version of one runtime lives.
///
/// `runtimes/<kind>/<version>/`, which is the layout
/// [runtime-versions.md](../../../.claude/features/runtime-versions.md) states and the reason a
/// version is a validated path component ([`RuntimeVersion`]) rather than a string: this is a `join`
/// and not an escaping problem.
#[must_use]
pub fn directory(paths: &Paths, kind: RuntimeKind, version: &RuntimeVersion) -> PathBuf {
    paths.runtimes().join(kind.as_str()).join(version.as_str())
}

/// What to run once a runtime is unpacked, to find out whether it runs *here*.
///
/// The executable is named as a key of the artifact's `provides` map rather than as a path, because
/// the path inside the archive is the publisher's and the name is ours.
///
/// **Every one of these exits zero quickly and touches the runtime's own machinery**, which is the
/// property that makes the check worth its second: `php -v` loads every extension the ini names, so
/// a build whose extension directory is wrong fails here rather than in the middle of somebody's
/// afternoon. `--version` is the same thing for the other three.
#[must_use]
pub fn smoke_test(kind: RuntimeKind) -> SmokeTest {
    let (executable, flag) = match kind {
        // Not `php-win.exe`, which answers `-v` with nothing at all — one of the four bugs T20a
        // found, and the reason the index publishes both under names of ours.
        RuntimeKind::Php => ("php", "-v"),
        RuntimeKind::Node => ("node", "--version"),
        RuntimeKind::Python => ("python", "--version"),
        RuntimeKind::Ruby => ("ruby", "--version"),
    };

    SmokeTest {
        executable: executable.to_owned(),
        args: vec![flag.to_owned()],
    }
}

/// Everything a finished install has to write down.
///
/// Taken as one value rather than as seven arguments, because five of them come straight off the
/// artifact that was installed and a caller assembling them in the wrong order would produce a row
/// that is wrong rather than one that fails to insert.
#[derive(Debug, Clone)]
pub struct Installation {
    /// Which language.
    pub kind: RuntimeKind,

    /// Which version.
    pub version: RuntimeVersion,

    /// Which channel the index published it on.
    pub channel: RuntimeChannel,

    /// Where it landed — [`directory`]'s answer, passed in rather than recomputed so the row names
    /// the directory that was actually renamed into place.
    pub path: PathBuf,

    /// How large the archive was, as the index declared it and the download proved it.
    pub bytes: u64,

    /// Which URL it came from, kept so a support conversation can ask "from where".
    pub url: String,

    /// The hash the index published for it, kept for the same reason.
    pub sha256: String,
}

/// Write down a runtime that is now on disk.
///
/// **The first version of a kind becomes its default**, in the same transaction that inserts it.
/// That is not a convenience: a home whose only PHP is not the default is a home where `php`
/// resolves to nothing, and nobody installing their first PHP is asking for that. Every version
/// after it arrives beside the default rather than replacing it, because an install that silently
/// moved what `php` means would be an install that broke a project the user was not thinking about.
///
/// # Errors
///
/// [`Error::AlreadyRecorded`] when a row for this kind and version already exists, and
/// [`Error::Database`] when it cannot be written.
pub async fn remember(
    store: &Store,
    installation: &Installation,
    at: Timestamp,
) -> Result<RuntimeSummary> {
    // `BEGIN IMMEDIATE` for `jobs::progress`' reason: the count below decides what the insert
    // writes, and a deferred `BEGIN` would leave the write to upgrade a read snapshot.
    let mut tx = store
        .pool()
        .begin_with("BEGIN IMMEDIATE")
        .await
        .map_err(|source| store.failure("write", source))?;

    let (kind, version) = (installation.kind.as_str(), installation.version.as_str());

    let has_default = sqlx::query_scalar!(
        "SELECT COUNT(*) FROM runtime_installs WHERE kind = ? AND is_default = 1",
        kind
    )
    .fetch_one(&mut *tx)
    .await
    .map_err(|source| store.failure("read", source))?
        > 0;

    let default = !has_default;
    let (channel, installed_at) = (installation.channel.as_str(), at.to_rfc3339());
    let path = installation.path.display().to_string();
    // The column is INTEGER and the value is a count of bytes: a size that does not fit an `i64` is
    // an artifact of nine million terabytes, so the saturation is a formality rather than a case.
    let bytes = i64::try_from(installation.bytes).unwrap_or(i64::MAX);
    let is_default = i64::from(default);

    let inserted = sqlx::query!(
        "INSERT INTO runtime_installs
             (kind, version, channel, install_path, installed_at, size_bytes, source_url, sha256,
              is_default)
         VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?)
         ON CONFLICT (kind, version) DO NOTHING",
        kind,
        version,
        channel,
        path,
        installed_at,
        bytes,
        installation.url,
        installation.sha256,
        is_default
    )
    .execute(&mut *tx)
    .await
    .map_err(|source| store.failure("write", source))?;

    // `DO NOTHING` rather than letting the unique index raise: the collision is a real case — two
    // clients asking for the same install at the same moment — and it deserves a sentence naming the
    // version rather than SQLite's.
    if inserted.rows_affected() == 0 {
        return Err(Error::AlreadyRecorded {
            kind: installation.kind,
            version: installation.version.clone(),
        });
    }

    tx.commit()
        .await
        .map_err(|source| store.failure("write", source))?;

    tracing::info!(%kind, %version, default, "a runtime was recorded");

    Ok(RuntimeSummary {
        kind: installation.kind,
        version: installation.version.clone(),
        channel: installation.channel,
        path,
        installed_at: at,
        bytes: installation.bytes,
        default,
    })
}

/// Forget a runtime whose directory has already gone, and say whether its kind is left with none.
///
/// **Nothing is promoted in its place**, and that stayed true when T24 arrived with the grammar
/// that could pick a successor. Being *able* to name the newest remaining version was never the
/// argument: an uninstall that silently moved what `php` means would break a project nobody was
/// thinking about, which is the same reason [`remember`] does not move it either. A kind whose
/// default was removed is left with no default, out loud, and `mix runtime default` is one command.
///
/// # Errors
///
/// [`Error::NotFound`] when there is no such row, and [`Error::Database`] when it cannot be written.
pub async fn forget(store: &Store, kind: RuntimeKind, version: &RuntimeVersion) -> Result<bool> {
    let (kind_column, version_column) = (kind.as_str(), version.as_str());

    let removed = sqlx::query_scalar!(
        "DELETE FROM runtime_installs WHERE kind = ? AND version = ? RETURNING is_default",
        kind_column,
        version_column
    )
    .fetch_optional(store.pool())
    .await
    .map_err(|source| store.failure("write", source))?
    .ok_or_else(|| missing(kind, version))?;

    let was_default = removed == 1;
    tracing::info!(%kind_column, %version_column, was_default, "a runtime was forgotten");

    Ok(was_default)
}

/// Make one installed version the one its kind resolves to.
///
/// Two statements in one transaction, and the partial unique index on the table is what makes the
/// pair safe rather than the ordering: `runtime_installs_one_default_per_kind` refuses a second
/// default for a kind outright, so a bug that cleared nothing would fail loudly instead of leaving
/// two rows both claiming to be it.
///
/// Idempotent: making the current default the default again writes the same two rows and answers the
/// same summary, which is what `.claude/architecture/daemon-and-ipc.md` asks of every verb it makes
/// sense for.
///
/// # Errors
///
/// [`Error::NotFound`] when that version is not installed, [`Error::UnreadableRuntimeRow`] when the
/// row cannot be read back, and [`Error::Database`] when it cannot be written.
pub async fn set_default(
    store: &Store,
    kind: RuntimeKind,
    version: &RuntimeVersion,
) -> Result<RuntimeSummary> {
    let mut tx = store
        .pool()
        .begin_with("BEGIN IMMEDIATE")
        .await
        .map_err(|source| store.failure("write", source))?;

    let (kind_column, version_column) = (kind.as_str(), version.as_str());

    sqlx::query!(
        "UPDATE runtime_installs SET is_default = 0 WHERE kind = ? AND is_default = 1",
        kind_column
    )
    .execute(&mut *tx)
    .await
    .map_err(|source| store.failure("write", source))?;

    let promoted = sqlx::query!(
        "UPDATE runtime_installs SET is_default = 1
         WHERE kind = ? AND version = ?
         RETURNING kind, version, channel, install_path, installed_at, size_bytes, is_default",
        kind_column,
        version_column
    )
    .fetch_optional(&mut *tx)
    .await
    .map_err(|source| store.failure("write", source))?
    .ok_or_else(|| missing(kind, version))?;

    let summary = summary(
        promoted.kind,
        promoted.version,
        promoted.channel,
        promoted.install_path,
        &promoted.installed_at,
        promoted.size_bytes,
        promoted.is_default,
    )?;

    tx.commit()
        .await
        .map_err(|source| store.failure("write", source))?;

    tracing::info!(%kind_column, %version_column, "a runtime became the default for its kind");

    Ok(summary)
}

/// Every installed runtime, optionally of one kind.
///
/// Ordered by kind and then by the version string, which is **not** newest-first — and still is not,
/// now that [`RuntimeVersion::cmp_precedence`] could make it so. A listing is a table somebody scans
/// for a row they already have in mind, and the order that makes a row findable is the one the eye
/// can predict. Choosing *between* versions is [`crate::resolve`]'s, and that is where precedence
/// belongs; a client that wants another order here has the whole list.
///
/// # Errors
///
/// [`Error::UnreadableRuntimeRow`] when a row holds something this build cannot read back, and
/// [`Error::Database`] when the table cannot be read.
pub async fn records(store: &Store, kind: Option<RuntimeKind>) -> Result<Vec<RuntimeSummary>> {
    let wanted = kind.map(RuntimeKind::as_str);

    // The same value bound twice rather than a numbered parameter, on `jobs::records`' precedent:
    // `?1` is valid SQLite and it is the sqlx macro that has an opinion about it.
    let rows = sqlx::query!(
        "SELECT kind, version, channel, install_path, installed_at, size_bytes, is_default
         FROM runtime_installs
         WHERE (? IS NULL OR kind = ?)
         ORDER BY kind, version",
        wanted,
        wanted
    )
    .fetch_all(store.pool())
    .await
    .map_err(|source| store.failure("read", source))?;

    rows.into_iter()
        .map(|row| {
            summary(
                row.kind,
                row.version,
                row.channel,
                row.install_path,
                &row.installed_at,
                row.size_bytes,
                row.is_default,
            )
        })
        .collect()
}

/// One installed runtime.
///
/// # Errors
///
/// [`Error::NotFound`] when it is not installed, [`Error::UnreadableRuntimeRow`] when the row cannot
/// be read back, and [`Error::Database`] when the table cannot be read.
pub async fn record(
    store: &Store,
    kind: RuntimeKind,
    version: &RuntimeVersion,
) -> Result<RuntimeSummary> {
    let (kind_column, version_column) = (kind.as_str(), version.as_str());

    let row = sqlx::query!(
        "SELECT kind, version, channel, install_path, installed_at, size_bytes, is_default
         FROM runtime_installs WHERE kind = ? AND version = ?",
        kind_column,
        version_column
    )
    .fetch_optional(store.pool())
    .await
    .map_err(|source| store.failure("read", source))?
    .ok_or_else(|| missing(kind, version))?;

    summary(
        row.kind,
        row.version,
        row.channel,
        row.install_path,
        &row.installed_at,
        row.size_bytes,
        row.is_default,
    )
}

/// Whether anything of this kind is installed at all.
///
/// Its own read rather than `records(…).is_empty()`, because the caller asking it — the uninstall
/// that has just removed a default — does not want the rows and would be paying for a listing to
/// learn a boolean.
///
/// # Errors
///
/// [`Error::Database`] when the table cannot be read.
pub async fn any_installed(store: &Store, kind: RuntimeKind) -> Result<bool> {
    let kind_column = kind.as_str();

    let count = sqlx::query_scalar!(
        "SELECT COUNT(*) FROM runtime_installs WHERE kind = ?",
        kind_column
    )
    .fetch_one(store.pool())
    .await
    .map_err(|source| store.failure("read", source))?;

    Ok(count > 0)
}

/// The failure of looking one up, named the way the wire mapping expects.
///
/// `kind: "runtime"` is the RPC namespace, which is what the daemon turns into the hint
/// `mix runtime list` — so the identifier has to read as one thing a person could have typed.
fn missing(kind: RuntimeKind, version: &RuntimeVersion) -> Error {
    Error::NotFound {
        kind: "runtime",
        id: format!("{kind} {version}"),
    }
}

/// One row, as the type a client renders.
///
/// Every column is checked rather than assumed, because this is where a hand-edited database or a
/// row written by a version that knew more than this one arrives. `kind` has a `CHECK` behind it and
/// is still parsed here: the constraint and the enum have to agree, and the reader is what says so
/// when they do not.
// Seven columns, listed once, rather than a struct that exists only to be destructured immediately
// — `jobs::summary`'s reasoning, one table across. It stays one under clippy's threshold, which is
// why there is no expectation here and one next door.
fn summary(
    kind: String,
    version: String,
    channel: String,
    path: String,
    installed_at: &str,
    bytes: i64,
    is_default: i64,
) -> Result<RuntimeSummary> {
    let unreadable = |column: &'static str, value: &str| Error::UnreadableRuntimeRow {
        column,
        value: value.to_owned(),
    };

    Ok(RuntimeSummary {
        kind: RuntimeKind::parse(&kind).ok_or_else(|| unreadable("kind", &kind))?,
        version: RuntimeVersion::parse(version.clone())
            .map_err(|VersionError { value, .. }| unreadable("version", &value))?,
        // The one column with no `CHECK` and no closed set behind it in SQL, which makes it the one
        // a row written by a newer build reaches this function through.
        channel: RuntimeChannel::parse(&channel).ok_or_else(|| unreadable("channel", &channel))?,
        path,
        installed_at: Timestamp::parse_rfc3339(installed_at)
            .ok_or_else(|| unreadable("installed_at", installed_at))?,
        // A negative size is not expressible through any write of ours and would be a hand-edited
        // row; read as zero rather than refused, because the number is a display value and refusing
        // the whole listing over it would hide the runtime it belongs to.
        bytes: u64::try_from(bytes).unwrap_or(0),
        default: is_default == 1,
    })
}

/// Remove an installed runtime's directory, if it is there at all.
///
/// **Before the row, never after**, which is the ordering the note at the top of this module
/// explains: a directory that could not be removed leaves a row that still describes it, and asking
/// again repeats exactly this.
///
/// # Errors
///
/// [`Error::Io`] naming the directory, which on Windows most often means a process is still running
/// out of it.
pub async fn discard(path: &Path) -> Result<()> {
    let owned = path.to_path_buf();

    let removed = tokio::task::spawn_blocking(move || match std::fs::remove_dir_all(&owned) {
        // Already gone is the outcome that was wanted. A row whose directory somebody deleted by
        // hand is exactly the repair this ordering leaves possible, and refusing it would make the
        // row unremovable through the API.
        Err(source) if source.kind() == std::io::ErrorKind::NotFound => Ok(()),
        other => other,
    })
    .await;

    match removed {
        Ok(Ok(())) => Ok(()),
        Ok(Err(source)) => Err(Error::Io {
            action: "remove",
            path: path.to_path_buf(),
            source,
        }),
        // The blocking pool panicked, which nothing here does. Reported rather than unwrapped
        // because nothing in this crate panics.
        Err(source) => Err(Error::Io {
            action: "remove",
            path: path.to_path_buf(),
            source: std::io::Error::other(source),
        }),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const NOW: Timestamp = Timestamp(1_760_000_000_000);

    async fn store() -> (tempfile::TempDir, Store) {
        let home = tempfile::tempdir().expect("a temporary directory");
        let store = Store::open(&home.path().join(crate::paths::DATABASE_FILE_NAME))
            .await
            .expect("a database");
        (home, store)
    }

    fn version(text: &str) -> RuntimeVersion {
        RuntimeVersion::parse(text).expect("a valid version")
    }

    fn installation(kind: RuntimeKind, text: &str) -> Installation {
        Installation {
            kind,
            version: version(text),
            channel: RuntimeChannel::Stable,
            path: PathBuf::from("/home/runtimes")
                .join(kind.as_str())
                .join(text),
            bytes: 41_000_000,
            url: format!("https://example.invalid/{kind}-{text}.tar.zst"),
            sha256: "00".to_owned(),
        }
    }

    /// A home whose only PHP is not the default is a home where `php` resolves to nothing.
    #[tokio::test]
    async fn the_first_version_of_a_kind_becomes_its_default_and_the_next_does_not() {
        let (_home, store) = store().await;

        let first = remember(&store, &installation(RuntimeKind::Php, "8.3.33"), NOW)
            .await
            .expect("a row");
        let second = remember(&store, &installation(RuntimeKind::Php, "8.2.20"), NOW)
            .await
            .expect("a row");

        assert!(first.default, "the first of its kind");
        assert!(
            !second.default,
            "an install that moved what `php` means would break a project nobody was thinking about"
        );

        // And the kind's default is untouched by the second install.
        assert!(
            record(&store, RuntimeKind::Php, &version("8.3.33"))
                .await
                .expect("the row")
                .default
        );
    }

    /// Every kind counts its own, so installing Node does not leave PHP without a default.
    #[tokio::test]
    async fn a_default_is_per_kind() {
        let (_home, store) = store().await;

        remember(&store, &installation(RuntimeKind::Php, "8.3.33"), NOW)
            .await
            .expect("a row");
        let node = remember(&store, &installation(RuntimeKind::Node, "20.11.0"), NOW)
            .await
            .expect("a row");

        assert!(node.default, "the first node, whatever php has");
    }

    #[tokio::test]
    async fn the_same_version_is_not_recorded_twice() {
        let (_home, store) = store().await;

        remember(&store, &installation(RuntimeKind::Php, "8.3.33"), NOW)
            .await
            .expect("a row");
        let error = remember(&store, &installation(RuntimeKind::Php, "8.3.33"), NOW)
            .await
            .expect_err("it is already there");

        assert!(
            matches!(&error, Error::AlreadyRecorded { version, .. } if version.as_str() == "8.3.33"),
            "{error:?}"
        );
    }

    #[tokio::test]
    async fn the_default_moves_and_only_one_version_holds_it() {
        let (_home, store) = store().await;

        remember(&store, &installation(RuntimeKind::Php, "8.3.33"), NOW)
            .await
            .expect("a row");
        remember(&store, &installation(RuntimeKind::Php, "8.2.20"), NOW)
            .await
            .expect("a row");

        let moved = set_default(&store, RuntimeKind::Php, &version("8.2.20"))
            .await
            .expect("it is installed");
        assert!(moved.default);

        let defaults: Vec<String> = records(&store, Some(RuntimeKind::Php))
            .await
            .expect("a listing")
            .into_iter()
            .filter(|runtime| runtime.default)
            .map(|runtime| runtime.version.as_str().to_owned())
            .collect();

        assert_eq!(defaults, ["8.2.20"], "one default per kind, and it moved");
    }

    /// The verb has to be idempotent, per the API's own rule about verbs it makes sense for.
    #[tokio::test]
    async fn making_the_default_the_default_again_changes_nothing() {
        let (_home, store) = store().await;
        remember(&store, &installation(RuntimeKind::Php, "8.3.33"), NOW)
            .await
            .expect("a row");

        let again = set_default(&store, RuntimeKind::Php, &version("8.3.33"))
            .await
            .expect("it is installed");

        assert!(again.default);
    }

    #[tokio::test]
    async fn a_version_that_is_not_installed_cannot_become_the_default() {
        let (_home, store) = store().await;

        let error = set_default(&store, RuntimeKind::Php, &version("8.3.33"))
            .await
            .expect_err("nothing is installed");

        assert!(
            matches!(&error, Error::NotFound { kind: "runtime", id } if id == "php 8.3.33"),
            "{error:?}"
        );
    }

    /// Nothing is promoted, and the caller is told — which is the whole of what `default_cleared`
    /// on the wire says.
    #[tokio::test]
    async fn forgetting_the_default_leaves_the_kind_without_one() {
        let (_home, store) = store().await;
        remember(&store, &installation(RuntimeKind::Php, "8.3.33"), NOW)
            .await
            .expect("a row");
        remember(&store, &installation(RuntimeKind::Php, "8.2.20"), NOW)
            .await
            .expect("a row");

        assert!(
            forget(&store, RuntimeKind::Php, &version("8.3.33"))
                .await
                .expect("it is installed"),
            "the one that was the default"
        );
        assert!(
            !records(&store, Some(RuntimeKind::Php))
                .await
                .expect("a listing")
                .iter()
                .any(|runtime| runtime.default),
            "8.2.20 is not promoted: an uninstall does not get to move what `php` means"
        );
        assert!(
            !forget(&store, RuntimeKind::Php, &version("8.2.20"))
                .await
                .expect("it is installed"),
            "and the other one never was"
        );
        assert!(
            !any_installed(&store, RuntimeKind::Php)
                .await
                .expect("a count")
        );
    }

    #[tokio::test]
    async fn forgetting_something_that_is_not_there_says_so() {
        let (_home, store) = store().await;

        let error = forget(&store, RuntimeKind::Php, &version("8.3.33"))
            .await
            .expect_err("no such row");

        assert!(
            matches!(
                &error,
                Error::NotFound {
                    kind: "runtime",
                    ..
                }
            ),
            "{error:?}"
        );
    }

    #[tokio::test]
    async fn a_listing_can_ask_for_one_kind_and_a_home_with_none_is_an_answer() {
        let (_home, store) = store().await;
        remember(&store, &installation(RuntimeKind::Php, "8.3.33"), NOW)
            .await
            .expect("a row");
        remember(&store, &installation(RuntimeKind::Node, "20.11.0"), NOW)
            .await
            .expect("a row");

        let php = records(&store, Some(RuntimeKind::Php))
            .await
            .expect("a listing");
        assert_eq!(php.len(), 1);
        assert_eq!(php[0].kind, RuntimeKind::Php);

        assert_eq!(
            records(&store, None).await.expect("a listing").len(),
            2,
            "no filter is every kind"
        );
        assert!(
            records(&store, Some(RuntimeKind::Ruby))
                .await
                .expect("a listing")
                .is_empty(),
            "a kind nobody has installed is an empty list rather than a failure"
        );
    }

    /// The moment survives the text column, which is what makes `installed_at` a `Timestamp` on the
    /// wire rather than whatever string happened to be stored.
    #[tokio::test]
    async fn the_moment_a_runtime_was_installed_survives_the_column_it_is_written_to() {
        let (_home, store) = store().await;

        remember(&store, &installation(RuntimeKind::Php, "8.3.33"), NOW)
            .await
            .expect("a row");

        let row = record(&store, RuntimeKind::Php, &version("8.3.33"))
            .await
            .expect("the row");

        assert_eq!(row.installed_at, NOW);
        assert_eq!(row.bytes, 41_000_000);
        assert_eq!(row.channel, RuntimeChannel::Stable);
    }

    /// What a hand-edited database looks like from in here, asked of the function rather than
    /// through a doctored row — `jobs::tests`' reasoning, and the same three columns' worth of
    /// vocabulary that no `CHECK` can hold.
    #[test]
    fn a_row_this_build_cannot_read_names_the_column_rather_than_the_reader() {
        for (kind, version, channel, moment, column) in [
            ("perl", "8.3.33", "stable", "2026-08-14T06:55:12Z", "kind"),
            (
                "php",
                "../escape",
                "stable",
                "2026-08-14T06:55:12Z",
                "version",
            ),
            (
                "php",
                "8.3.33",
                "nightly",
                "2026-08-14T06:55:12Z",
                "channel",
            ),
            ("php", "8.3.33", "stable", "yesterday", "installed_at"),
        ] {
            let error = summary(
                kind.to_owned(),
                version.to_owned(),
                channel.to_owned(),
                "/home/runtimes/php/8.3.33".to_owned(),
                moment,
                1,
                0,
            )
            .expect_err("the row holds something this build cannot read");

            assert!(
                matches!(&error, Error::UnreadableRuntimeRow { column: named, .. } if *named == column),
                "{error:?} should have named {column}"
            );
        }
    }

    /// The `CHECK` on the column and [`RuntimeKind`] have to agree, or one of them is decoration.
    #[tokio::test]
    async fn the_kind_column_accepts_every_kind_and_nothing_else() {
        let (_home, store) = store().await;

        for kind in RuntimeKind::ALL {
            remember(&store, &installation(kind, "1.0.0"), NOW)
                .await
                .unwrap_or_else(|error| panic!("the column refused {kind}: {error}"));
        }

        let refused = sqlx::query(
            "INSERT INTO runtime_installs
                 (kind, version, channel, install_path, installed_at, size_bytes, source_url,
                  sha256)
             VALUES ('perl', '5.38', 'stable', '/x', '2026-08-14T06:55:12Z', 1, 'x', '00')",
        )
        .execute(store.pool())
        .await;

        assert!(
            refused.is_err(),
            "the CHECK let a word through that RuntimeKind cannot read back"
        );
    }

    /// Removing a directory that is not there is the outcome that was wanted, which is what keeps a
    /// row whose directory somebody deleted by hand removable through the API.
    #[tokio::test]
    async fn discarding_a_directory_that_has_already_gone_is_not_a_failure() {
        let home = tempfile::tempdir().expect("a temporary directory");
        let runtime = home.path().join("php").join("8.3.33");
        std::fs::create_dir_all(&runtime).expect("a directory");
        std::fs::write(runtime.join("php"), b"binary").expect("a file");

        discard(&runtime).await.expect("it is there");
        assert!(!runtime.exists());

        discard(&runtime).await.expect("and now it is not");
    }
}
