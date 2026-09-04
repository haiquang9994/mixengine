//! What this home remembers about updates — roadmap task **T88**, the design's D10.
//!
//! Four rows in `settings`, which has been `(key TEXT PRIMARY KEY, value_json TEXT NOT NULL) STRICT`
//! since `0001_initial.sql` and has never had a row in it. This is its first user, and it needs no
//! migration.
//!
//! | Key | Value | Written by | Read by |
//! | --- | --- | --- | --- |
//! | [`SKIPPED_VERSION`] | `"0.2.0"` | `update.decide`, and the post-restart check | the offer decision |
//! | [`REMIND_AFTER`] | a moment | `update.decide` | the offer decision |
//! | [`APPLIED`] | [`Applied`] | the apply, before the swap | the new daemon at start |
//! | [`RESTORE`] | the services that were running | the apply, before the swap | the new daemon at start |
//!
//! **Every record is deleted before it is acted on**, and that is not a tidiness rule. A
//! [`RESTORE`] that survived being read would be replayed by every later start — so a person who
//! updates, stops MariaDB because they are done with it, and reboots gets MariaDB back, for ever,
//! with nothing in the product able to tell them why. Deleted *before* rather than after, so that a
//! start which dies half way through the restore does not make the record immortal either: the cost
//! of this order is one lost restore on a daemon that crashed mid-start, and the cost of the other
//! is a home that can never stop a service again.
//!
//! **A row this build cannot read is [`None`] and not an error.** A record whose JSON no longer
//! parses means the same thing to every caller here as one that was never written — go and ask
//! again — and failing a daemon start over a skipped version would be absurd.

use crate::{Result, store::Store};

/// The version somebody answered *skip this one* to.
pub const SKIPPED_VERSION: &str = "updates.skipped_version";

/// When somebody who answered *remind me later* wants to be asked again.
pub const REMIND_AFTER: &str = "updates.remind_after";

/// The update that is in progress, written before the binaries are swapped.
pub const APPLIED: &str = "updates.applied";

/// What was running when an update stopped it, in the order it was stopped.
pub const RESTORE: &str = "updates.restore";

/// How far ahead *remind me later* puts the next offer.
///
/// Three days, against a check that runs daily: one day would be tomorrow, which is not what
/// anybody means by later, and a week is long enough that a security release waits behind a shrug.
pub const LATER_SECONDS: i64 = 3 * 24 * 60 * 60;

/// The furthest ahead a stored [`REMIND_AFTER`] is believed at all.
///
/// [`REMIND_AFTER`] is a wall-clock moment, so a machine whose clock was a year fast when somebody
/// answered *later* holds a moment a year away once the clock is corrected — and would then never
/// be offered anything again.
///
/// **A moment further ahead than this is ignored rather than clamped**, and the difference matters:
/// clamping it to `now + seven days` on every read would move the deadline forward every time it
/// was read, so it would never come due at all. What is stored is either a reminder somebody asked
/// for — which is [`LATER_SECONDS`] away and well inside this — or a reading from a clock that has
/// since been corrected, which is not a reminder and is not treated as one.
pub const REMIND_CLAMP_SECONDS: i64 = 7 * 24 * 60 * 60;

/// An update this daemon started, as the daemon that comes up afterwards reads it.
///
/// `to` is what the *feed* said the payload was, which is why the check that uses it belongs after
/// the restart: the only honest answer to "what version is running" is the running binary's own
/// `CARGO_PKG_VERSION`, and comparing it against this is what catches a mislabelled release.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct Applied {
    /// The version that was running before.
    pub from: String,

    /// The version the feed said the payload is.
    pub to: String,

    /// When the swap was made.
    pub at: mixengine_proto::Timestamp,
}

/// Read one record, or [`None`] when it is absent or unreadable.
///
/// # Errors
///
/// [`crate::Error`] only for a database that could not be read at all. A row that is there and
/// cannot be decoded is [`None`] — see the module note.
pub async fn get<T: serde::de::DeserializeOwned>(store: &Store, key: &str) -> Result<Option<T>> {
    let row = sqlx::query!("SELECT value_json FROM settings WHERE key = ?", key)
        .fetch_optional(store.pool())
        .await
        .map_err(|source| store.failure("read", source))?;

    let Some(row) = row else {
        return Ok(None);
    };

    match serde_json::from_str(&row.value_json) {
        Ok(value) => Ok(Some(value)),
        Err(error) => {
            tracing::warn!(
                key,
                %error,
                "a settings row this build cannot read; treating it as absent"
            );
            Ok(None)
        }
    }
}

/// Write one record, replacing whatever was there.
///
/// # Errors
///
/// [`crate::Error`] when the row cannot be written, or when `value` cannot be encoded — which is a
/// bug in the caller rather than anything about this machine.
pub async fn set<T: serde::Serialize>(store: &Store, key: &str, value: &T) -> Result<()> {
    let encoded = serde_json::to_string(value).map_err(|source| crate::Error::SettingUnwritable {
        key: key.to_owned(),
        source,
    })?;

    sqlx::query!(
        "INSERT INTO settings (key, value_json) VALUES (?, ?)
         ON CONFLICT(key) DO UPDATE SET value_json = excluded.value_json",
        key,
        encoded
    )
    .execute(store.pool())
    .await
    .map_err(|source| store.failure("write", source))?;

    Ok(())
}

/// Remove one record. Removing one that is not there is not an error.
///
/// # Errors
///
/// [`crate::Error`] when the row cannot be deleted.
pub async fn clear(store: &Store, key: &str) -> Result<()> {
    sqlx::query!("DELETE FROM settings WHERE key = ?", key)
        .execute(store.pool())
        .await
        .map_err(|source| store.failure("write", source))?;

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    async fn home() -> (tempfile::TempDir, Store) {
        let temp = tempfile::tempdir().expect("a temporary directory");
        let store = Store::open(&temp.path().join("mixengine.db"))
            .await
            .expect("a database");

        (temp, store)
    }

    #[tokio::test]
    async fn a_record_that_was_written_reads_back() {
        let (_temp, store) = home().await;

        set(&store, SKIPPED_VERSION, &"0.2.0".to_owned())
            .await
            .expect("a write");

        assert_eq!(
            get::<String>(&store, SKIPPED_VERSION)
                .await
                .expect("a read"),
            Some("0.2.0".to_owned())
        );
    }

    #[tokio::test]
    async fn a_record_written_twice_keeps_the_second_value() {
        let (_temp, store) = home().await;

        set(&store, SKIPPED_VERSION, &"0.2.0".to_owned())
            .await
            .expect("a write");
        set(&store, SKIPPED_VERSION, &"0.3.0".to_owned())
            .await
            .expect("a second write");

        assert_eq!(
            get::<String>(&store, SKIPPED_VERSION)
                .await
                .expect("a read"),
            Some("0.3.0".to_owned())
        );
    }

    /// The property the whole restore path rests on: a record that was read is gone, so a start
    /// that replays it once never replays it twice.
    #[tokio::test]
    async fn a_record_that_was_cleared_is_gone() {
        let (_temp, store) = home().await;
        set(&store, RESTORE, &vec!["mariadb".to_owned()])
            .await
            .expect("a write");

        clear(&store, RESTORE).await.expect("a delete");

        assert_eq!(
            get::<Vec<String>>(&store, RESTORE).await.expect("a read"),
            None
        );
    }

    #[tokio::test]
    async fn clearing_a_record_that_was_never_written_is_not_an_error() {
        let (_temp, store) = home().await;

        clear(&store, RESTORE).await.expect("a delete of nothing");
    }

    #[tokio::test]
    async fn a_record_that_was_never_written_is_none_rather_than_an_error() {
        let (_temp, store) = home().await;

        assert_eq!(
            get::<String>(&store, SKIPPED_VERSION)
                .await
                .expect("a read"),
            None
        );
    }

    /// A row this build cannot decode means what an absent one means, and must not fail a start.
    #[tokio::test]
    async fn a_record_this_build_cannot_read_is_absent_rather_than_an_error() {
        let (_temp, store) = home().await;
        set(&store, APPLIED, &"not an Applied at all".to_owned())
            .await
            .expect("a write");

        assert_eq!(get::<Applied>(&store, APPLIED).await.expect("a read"), None);
    }

    #[tokio::test]
    async fn the_applied_record_round_trips() {
        let (_temp, store) = home().await;
        let applied = Applied {
            from: "0.1.0".to_owned(),
            to: "0.2.0".to_owned(),
            at: mixengine_proto::Timestamp(1_757_000_000_000),
        };

        set(&store, APPLIED, &applied).await.expect("a write");

        assert_eq!(
            get::<Applied>(&store, APPLIED).await.expect("a read"),
            Some(applied)
        );
    }
}
