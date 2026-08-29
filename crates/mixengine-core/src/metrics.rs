//! The 24-hour history, and the three things done to it — roadmap task **T71**.
//!
//! Writing a minute, reading a window, deleting what is older than the retention. **The arithmetic
//! that decides what a minute *is* lives in the daemon**, beside the loop that takes the readings;
//! what is here is the store and nothing else.
//!
//! Design: `docs/superpowers/specs/2026-08-30-t71-metrics-history-design.md`.

use mixengine_proto::{
    MetricsHistory, MetricsHistoryQuery, MetricsMinute, MetricsSubject, Timestamp,
};

use crate::{Result, Store};

/// Write one subject's minute, replacing a row already there.
///
/// **`INSERT OR REPLACE` rather than an insert that may fail.** A daemon that stops at forty seconds
/// past writes the minute it had; the daemon that starts next may finish the same minute. Two rows
/// for one minute is a doubled chart, and the later write is the better-supported of the two.
///
/// # Errors
///
/// [`Error::Database`](crate::Error::Database) when the row cannot be written.
pub async fn write_minute(store: &Store, row: &MetricsMinute) -> Result<()> {
    let subject = row.subject.to_string();
    let minute = row.minute.0;
    let cpu_avg = row.cpu_avg.map(f64::from);
    let cpu_peak = row.cpu_peak.map(f64::from);
    let rss_avg = i64::try_from(row.rss_avg).unwrap_or(i64::MAX);
    let rss_peak = i64::try_from(row.rss_peak).unwrap_or(i64::MAX);
    let samples = i64::from(row.samples);

    sqlx::query!(
        "INSERT OR REPLACE INTO metrics_minutes
             (subject, minute, cpu_avg, cpu_peak, rss_avg, rss_peak, samples)
         VALUES (?, ?, ?, ?, ?, ?, ?)",
        subject,
        minute,
        cpu_avg,
        cpu_peak,
        rss_avg,
        rss_peak,
        samples
    )
    .execute(store.pool())
    .await
    .map_err(|source| store.failure("write", source))?;

    Ok(())
}

/// The rows a query asks for, oldest first.
///
/// **A row whose subject this build cannot read is skipped and logged**, on
/// [`services::records`](crate::services::records)' rule: one row somebody wrote by hand may not
/// fail a listing that does not depend on it.
///
/// # Errors
///
/// [`Error::Database`](crate::Error::Database) when the table cannot be read.
pub async fn history(
    store: &Store,
    query: &MetricsHistoryQuery,
    retention_hours: u32,
) -> Result<MetricsHistory> {
    let subject = query.subject.as_ref().map(ToString::to_string);
    let since = query.since.map_or(i64::MIN, |at| at.0);
    let until = query.until.map_or(i64::MAX, |at| at.0);

    let rows = sqlx::query!(
        r#"SELECT subject  AS "subject!: String",
                  minute   AS "minute!: i64",
                  cpu_avg  AS "cpu_avg: f64",
                  cpu_peak AS "cpu_peak: f64",
                  rss_avg  AS "rss_avg!: i64",
                  rss_peak AS "rss_peak!: i64",
                  samples  AS "samples!: i64"
           FROM metrics_minutes
           WHERE minute >= ? AND minute <= ? AND (? IS NULL OR subject = ?)
           ORDER BY minute, subject"#,
        since,
        until,
        subject,
        subject
    )
    .fetch_all(store.pool())
    .await
    .map_err(|source| store.failure("read", source))?;

    let minutes = rows
        .into_iter()
        .filter_map(|row| {
            let Some(subject) = MetricsSubject::parse(&row.subject) else {
                tracing::warn!(
                    subject = row.subject,
                    "a metrics row names a subject this build cannot read"
                );
                return None;
            };

            Some(MetricsMinute {
                subject,
                minute: Timestamp(row.minute),
                cpu_avg: row.cpu_avg.map(|value| value as f32),
                cpu_peak: row.cpu_peak.map(|value| value as f32),
                rss_avg: u64::try_from(row.rss_avg).unwrap_or_default(),
                rss_peak: u64::try_from(row.rss_peak).unwrap_or_default(),
                samples: u32::try_from(row.samples).unwrap_or(u32::MAX),
            })
        })
        .collect();

    Ok(MetricsHistory {
        minutes,
        retention_hours,
    })
}

/// Delete every minute before `oldest`, and say how many went.
///
/// **The boundary minute is kept**: `oldest` is the earliest minute a client may still be shown.
///
/// The caller works `oldest` out from a wall clock and never from an elapsed
/// [`Instant`](std::time::Instant) — a laptop that slept eight hours has to trim eight hours of rows
/// on the tick after it wakes, and tokio's clock counted none of that time.
///
/// # Errors
///
/// [`Error::Database`](crate::Error::Database) when the rows cannot be deleted.
pub async fn trim(store: &Store, oldest: Timestamp) -> Result<u64> {
    let oldest = oldest.0;

    let deleted = sqlx::query!("DELETE FROM metrics_minutes WHERE minute < ?", oldest)
        .execute(store.pool())
        .await
        .map_err(|source| store.failure("write", source))?;

    Ok(deleted.rows_affected())
}

#[cfg(test)]
mod tests {
    use mixengine_proto::ServiceId;

    use super::*;

    async fn store() -> (tempfile::TempDir, Store) {
        let home = tempfile::tempdir().expect("a temporary directory");
        let store = Store::open(&home.path().join(crate::paths::DATABASE_FILE_NAME))
            .await
            .expect("a database");
        (home, store)
    }

    fn service(id: &str) -> MetricsSubject {
        MetricsSubject::Service(ServiceId::parse(id).expect("an id"))
    }

    fn minute(subject: MetricsSubject, minute: i64, samples: u32) -> MetricsMinute {
        MetricsMinute {
            subject,
            minute: Timestamp(minute),
            cpu_avg: Some(10.0),
            cpu_peak: Some(40.0),
            rss_avg: 1_000,
            rss_peak: 4_000,
            samples,
        }
    }

    #[tokio::test]
    async fn a_minute_is_written_and_read_back_as_itself() {
        let (_home, store) = store().await;

        write_minute(&store, &minute(service("mariadb@main"), 60_000, 60))
            .await
            .expect("written");

        let read = history(&store, &MetricsHistoryQuery::default(), 24)
            .await
            .expect("read");

        assert_eq!(
            read.minutes,
            vec![minute(service("mariadb@main"), 60_000, 60)]
        );
        assert_eq!(read.retention_hours, 24);
    }

    #[tokio::test]
    async fn a_minute_with_no_cpu_figure_reads_back_with_none_rather_than_zero() {
        let (_home, store) = store().await;

        let row = MetricsMinute {
            cpu_avg: None,
            cpu_peak: None,
            ..minute(MetricsSubject::Daemon, 60_000, 1)
        };

        write_minute(&store, &row).await.expect("written");

        let read = history(&store, &MetricsHistoryQuery::default(), 24)
            .await
            .expect("read");

        assert_eq!(read.minutes[0].cpu_avg, None);
        assert_eq!(
            read.minutes[0].rss_avg, 1_000,
            "memory was measured all the same"
        );
    }

    #[tokio::test]
    async fn a_subject_that_no_longer_exists_keeps_its_history() {
        // No foreign key: nothing about `services` may take a row here with it.
        let (_home, store) = store().await;

        write_minute(&store, &minute(service("redis@main"), 60_000, 1))
            .await
            .expect("written");

        sqlx::query("DELETE FROM services")
            .execute(store.pool())
            .await
            .expect("a delete over no rows is still a delete");

        assert_eq!(
            history(&store, &MetricsHistoryQuery::default(), 24)
                .await
                .expect("read")
                .minutes
                .len(),
            1
        );
    }

    #[tokio::test]
    async fn a_query_narrows_by_subject_and_by_window() {
        let (_home, store) = store().await;

        for (subject, at) in [
            (service("mariadb@main"), 60_000),
            (service("mariadb@main"), 120_000),
            (MetricsSubject::Daemon, 120_000),
        ] {
            write_minute(&store, &minute(subject, at, 1))
                .await
                .expect("written");
        }

        let narrowed = history(
            &store,
            &MetricsHistoryQuery {
                subject: Some(service("mariadb@main")),
                since: Some(Timestamp(120_000)),
                until: None,
            },
            24,
        )
        .await
        .expect("read");

        assert_eq!(narrowed.minutes.len(), 1);
        assert_eq!(narrowed.minutes[0].minute, Timestamp(120_000));
    }

    #[tokio::test]
    async fn the_trim_deletes_what_is_older_and_keeps_the_boundary() {
        let (_home, store) = store().await;

        write_minute(&store, &minute(MetricsSubject::Daemon, 60_000, 1))
            .await
            .expect("written");
        write_minute(&store, &minute(MetricsSubject::Daemon, 120_000, 1))
            .await
            .expect("written");

        assert_eq!(trim(&store, Timestamp(120_000)).await.expect("trimmed"), 1);

        let left = history(&store, &MetricsHistoryQuery::default(), 24)
            .await
            .expect("read");

        assert_eq!(left.minutes.len(), 1);
        assert_eq!(left.minutes[0].minute, Timestamp(120_000));
    }

    #[tokio::test]
    async fn writing_the_same_minute_twice_replaces_it() {
        let (_home, store) = store().await;

        write_minute(&store, &minute(MetricsSubject::Daemon, 60_000, 1))
            .await
            .expect("written");
        write_minute(&store, &minute(MetricsSubject::Daemon, 60_000, 42))
            .await
            .expect("written again");

        let read = history(&store, &MetricsHistoryQuery::default(), 24)
            .await
            .expect("read");

        assert_eq!(read.minutes.len(), 1);
        assert_eq!(read.minutes[0].samples, 42);
    }

    #[tokio::test]
    async fn a_subject_row_this_build_cannot_read_is_skipped_rather_than_fatal() {
        let (_home, store) = store().await;

        sqlx::query(
            "INSERT INTO metrics_minutes (subject, minute, rss_avg, rss_peak, samples)
             VALUES ('service:NOT AN ID', 60000, 1, 1, 1)",
        )
        .execute(store.pool())
        .await
        .expect("a row somebody wrote by hand");

        assert!(
            history(&store, &MetricsHistoryQuery::default(), 24)
                .await
                .expect("read")
                .minutes
                .is_empty(),
            "one unreadable row must not fail a listing that does not depend on it"
        );
    }
}
