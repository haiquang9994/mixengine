//! One loop, two rates — roadmap task **T71**.
//!
//! Sixty seconds by default, one second while a client holds `GET /metrics` open. **One loop and not
//! two**: a slow loop for the history beside a fast one for the stream would measure the same
//! processes at two different moments and hand a client two answers to one question, and while
//! somebody watched, every minute stored would have been measured twice.
//!
//! **What is measured is decided from the rows, not from the registry's own list.** A service is
//! measurable when its row names a pid *and* the moment that pid began — the pair T18 stores for
//! adoption — because a pid alone would let a service that exited between two ticks be drawn as
//! whatever program the system handed its number to next.

use std::sync::Arc;
use std::time::{Duration, Instant, SystemTime};

use mixengine_core::Store;
use mixengine_core::config::Metrics as Config;
use mixengine_platform::process::StartTime;
use mixengine_platform::{GroupRoot, Host};
use mixengine_proto::{
    MetricsFrame, MetricsSample, MetricsSubject, ServiceId, ServiceState, Timestamp,
};
use tokio::sync::{broadcast, mpsc, oneshot};

use super::minutes::Accumulator;
use super::watchers::Watchers;

/// How recent a reading has to be for [`Sampler::snapshot`] to serve it rather than take another.
///
/// One second, which is the fast rate: a script looping on `metrics.snapshot` cannot drive this
/// machine harder than a client that opened the stream and asked for the fast rate properly.
const FRESH_ENOUGH: Duration = Duration::from_secs(1);

/// How many frames a stream may fall behind by.
///
/// **Eight, where the event bus holds 1024, and a lagging reader is given the newest frame rather
/// than a resync.** For a metric the old value is worth nothing — the next one is a second away — so
/// there is nothing to replay and nothing to tell the client to re-fetch.
const STREAM_CAPACITY: usize = 8;

/// How many snapshot requests may be waiting for the loop at once.
const REQUESTS: usize = 8;

/// How many milliseconds an hour is, for the retention arithmetic.
const HOUR: i64 = 3_600_000;

/// One subject and the process at the head of its group.
type Subject = (MetricsSubject, GroupRoot);

/// Measure these subjects, and say what came back.
///
/// **Absence is the only way a subject goes unreported.** A root that has ended, or whose pid the
/// system handed to something else, produces no sample — never a sample of zero, because a minute
/// with no row has to keep meaning *nobody measured*.
fn frame_from(host: &dyn Host, at: Timestamp, subjects: &[Subject]) -> MetricsFrame {
    let roots: Vec<GroupRoot> = subjects.iter().map(|(_, root)| *root).collect();
    let readings = host.process_metrics().measure(&roots);

    let samples = subjects
        .iter()
        .filter_map(|(subject, root)| {
            let reading = readings.iter().find(|reading| reading.pid == root.pid)?;

            Some(MetricsSample {
                subject: subject.clone(),
                cpu_percent: reading.cpu_percent,
                rss_bytes: reading.rss_bytes,
                processes: reading.processes,
            })
        })
        .collect();

    MetricsFrame { at, samples }
}

/// The daemon's own group, or [`None`] where this process cannot be identified.
///
/// **Left out rather than measured without an identity.** Every other subject is checked against the
/// moment its process began, and the daemon giving itself an exemption would be the one row on the
/// chart nothing verified.
fn daemon_subject() -> Option<Subject> {
    let pid = std::process::id();
    let started = mixengine_platform::process::started_at(pid)
        .ok()
        .flatten()?;

    Some((MetricsSubject::Daemon, GroupRoot { pid, started }))
}

/// The loop's state: what to measure, what it has assembled, and what it last said.
#[derive(Debug)]
pub(crate) struct Sampler {
    store: Store,
    host: Arc<dyn Host>,
    watchers: Watchers,
    accumulator: Accumulator,

    /// The periods and the retention, read once at boot.
    fast: Duration,
    idle: Duration,
    retention_hours: u32,

    /// The last frame taken and when.
    ///
    /// Owned by the loop rather than shared, because the loop is the only thing that measures: a
    /// second reader taking its own readings would be a second consumer of the CPU state a
    /// difference is taken against, and each would see the interval since the *other's* refresh.
    latest: Option<(Instant, MetricsFrame)>,

    /// Where an open stream reads its frames from.
    frames: broadcast::Sender<MetricsFrame>,

    /// Snapshots somebody is waiting for. See [`Handle::snapshot`].
    requests: mpsc::Receiver<oneshot::Sender<MetricsFrame>>,

    /// Kept so that [`Sampler::handle`] can hand out more senders after construction.
    asking: mpsc::Sender<oneshot::Sender<MetricsFrame>>,
}

impl Sampler {
    /// A sampler over this home.
    pub(crate) fn new(
        store: Store,
        host: Arc<dyn Host>,
        watchers: Watchers,
        config: &Config,
    ) -> Self {
        // One in flight per waiting client is enough: a snapshot is answered by the next turn of the
        // loop, and a queue deeper than that would only let requests pile up behind a reading that
        // is already being taken.
        let (asking, requests) = mpsc::channel(REQUESTS);

        Self {
            store,
            host,
            watchers,
            accumulator: Accumulator::default(),
            fast: Duration::from_secs(config.sample_seconds),
            idle: Duration::from_secs(config.idle_sample_seconds),
            retention_hours: config.retention_hours,
            latest: None,
            frames: broadcast::Sender::new(STREAM_CAPACITY),
            requests,
            asking,
        }
    }

    /// The handle the API holds: what a stream reads and what a snapshot reuses.
    pub(crate) fn handle(&self) -> Handle {
        Handle {
            watchers: self.watchers.clone(),
            frames: self.frames.clone(),
            asking: self.asking.clone(),
            retention_hours: self.retention_hours,
        }
    }

    /// How long until the next reading, given who is watching.
    fn period(&self) -> Duration {
        if self.watchers.fast() {
            self.fast
        } else {
            self.idle
        }
    }

    /// Every subject this daemon can measure right now.
    ///
    /// A row whose state is not `running`, or which holds a pid without the moment it began, is not
    /// measurable and is left out — the second case is a row mid-write rather than a service to
    /// draw.
    async fn subjects(&self) -> Vec<Subject> {
        let mut subjects: Vec<Subject> = daemon_subject().into_iter().collect();

        let records = match mixengine_core::services::records(&self.store).await {
            Ok(records) => records,

            // The daemon's own reading still goes out: a database that cannot be read says nothing
            // about what this process costs, and dropping the frame entirely would blank a live
            // client's screen over a failure that has its own log line.
            Err(error) => {
                tracing::warn!(%error, "this home's services could not be read, so only the daemon is measured");
                return subjects;
            }
        };

        subjects.extend(records.into_iter().filter_map(|(id, record)| {
            if record.state != ServiceState::Running {
                return None;
            }

            let root = GroupRoot {
                pid: record.pid?,
                started: StartTime::from_stored(record.pid_start_time?),
            };

            Some((MetricsSubject::Service(ServiceId::parse(id).ok()?), root))
        }));

        subjects
    }

    /// Take one reading, fold it into the minute, and publish it.
    pub(crate) async fn take(&mut self) -> MetricsFrame {
        let subjects = self.subjects().await;
        let at = Timestamp::from_system_time(SystemTime::now());
        let frame = frame_from(self.host.as_ref(), at, &subjects);

        self.latest = Some((Instant::now(), frame.clone()));

        // Nobody listening is the ordinary state of a daemon with no client attached.
        let _ = self.frames.send(frame.clone());

        let rolled = self.accumulator.observe(&frame);
        self.write(rolled, at).await;

        frame
    }

    /// Write the minutes a tick completed, and trim what has aged out.
    ///
    /// **A write that fails is logged and the tick continues.** A database that will not take a
    /// metrics row is not a reason to stop measuring, and the live stream does not depend on it.
    async fn write(&self, rows: Vec<mixengine_proto::MetricsMinute>, now: Timestamp) {
        if rows.is_empty() {
            return;
        }

        for row in rows {
            if let Err(error) = mixengine_core::metrics::write_minute(&self.store, &row).await {
                tracing::warn!(%error, subject = %row.subject, "a metrics row could not be written");
            }
        }

        // **From the wall clock, never from an elapsed `Instant`.** A laptop that slept eight hours
        // has to trim eight hours of rows on the tick after it wakes, and tokio's clock counted none
        // of that time.
        let oldest = Timestamp(now.0.saturating_sub(i64::from(self.retention_hours) * HOUR));

        if let Err(error) = mixengine_core::metrics::trim(&self.store, oldest).await {
            tracing::warn!(%error, "the metrics history could not be trimmed");
        }
    }

    /// Answer a snapshot: the last reading if it is recent enough, or a new one.
    ///
    /// **A reading rather than the last one taken.** With nobody watching this daemon samples once a
    /// minute, so serving the cached tick would answer a person with a number up to a minute old and
    /// would not mention a service that started ten seconds ago. Reusing one younger than
    /// [`FRESH_ENOUGH`] is what stops a script looping on the method from driving this machine at a
    /// rate it never opened a stream to ask for.
    async fn answer(&mut self, waiting: oneshot::Sender<MetricsFrame>) {
        let fresh = self
            .latest
            .as_ref()
            .filter(|(taken, _)| taken.elapsed() < FRESH_ENOUGH)
            .map(|(_, frame)| frame.clone());

        let frame = match fresh {
            Some(frame) => frame,
            None => self.take().await,
        };

        // The caller gave up between asking and now, which is a client that closed its connection.
        let _ = waiting.send(frame);
    }

    /// Finish the minute in hand. What a shutdown calls.
    async fn flush(&mut self) {
        let rows = self.accumulator.drain();
        let now = Timestamp::from_system_time(SystemTime::now());

        self.write(rows, now).await;
    }
}

/// What the API holds: enough to serve a stream and a snapshot, and nothing that could take a
/// reading of its own.
#[derive(Debug, Clone)]
pub(crate) struct Handle {
    watchers: Watchers,
    frames: broadcast::Sender<MetricsFrame>,
    asking: mpsc::Sender<oneshot::Sender<MetricsFrame>>,

    /// How long this home keeps a minute row, so a history answer can say why its chart begins
    /// where it does. Read from the same config the loop trims against, rather than a second copy
    /// somewhere else that could disagree with it.
    retention_hours: u32,
}

impl Handle {
    /// Register one open stream, and hand back the frames from now on.
    ///
    /// The [`Watch`](super::watchers::Watch) travels with the receiver so that the two cannot be
    /// separated: the subscription *is* the reason this daemon is sampling every second, and a
    /// stream that dropped one and kept the other would leave the machine on the fast rate with
    /// nobody reading it.
    pub(crate) fn stream(&self) -> (super::watchers::Watch, broadcast::Receiver<MetricsFrame>) {
        (self.watchers.watch(), self.frames.subscribe())
    }

    /// How long a minute row is kept here.
    pub(crate) const fn retention_hours(&self) -> u32 {
        self.retention_hours
    }

    /// One reading, taken by the loop.
    ///
    /// **Asked of the loop rather than taken here**, because the loop is the only thing that
    /// measures: a CPU figure is a difference against the previous refresh, and a second caller
    /// refreshing on its own would leave each of them measuring the interval since the other.
    ///
    /// [`None`] means the loop is gone, which happens only while the daemon is shutting down.
    pub(crate) async fn snapshot(&self) -> Option<MetricsFrame> {
        let (answer, waiting) = oneshot::channel();

        self.asking.send(answer).await.ok()?;

        waiting.await.ok()
    }
}

/// Run the loop until the daemon shuts down.
pub(crate) fn start(mut sampler: Sampler, shutdown: tokio_util::sync::CancellationToken) {
    let mut signal = sampler.watchers.signal();

    tokio::spawn(async move {
        loop {
            let period = sampler.period();

            tokio::select! {
                () = shutdown.cancelled() => {
                    // A daemon stopping at forty seconds past would otherwise throw away two thirds
                    // of a minute it had already measured.
                    sampler.flush().await;
                    return;
                }

                // A client opening the stream must not wait out a sixty-second sleep for its first
                // frame, and one closing the last stream must not leave the machine on the fast
                // rate. Either way the period is recomputed and the wait begins again.
                () = signal.changed() => continue,

                // Somebody called `metrics.snapshot` and is holding a connection open for the
                // answer. Served here rather than by a reader of its own so that this loop stays
                // the only thing that measures.
                Some(waiting) = sampler.requests.recv() => {
                    sampler.answer(waiting).await;
                    continue;
                }

                () = tokio::time::sleep(period) => {}
            }

            sampler.take().await;
        }
    });
}

#[cfg(test)]
mod tests {
    use mixengine_platform::mock;

    use super::*;

    fn reading(pid: u32, cpu: Option<f32>, rss: u64) -> mixengine_platform::GroupReading {
        mixengine_platform::GroupReading {
            pid,
            cpu_percent: cpu,
            rss_bytes: rss,
            processes: 2,
        }
    }

    fn service(id: &str, pid: u32, stored: i64) -> Subject {
        (
            MetricsSubject::Service(ServiceId::parse(id).expect("an id")),
            GroupRoot {
                pid,
                started: StartTime::from_stored(stored),
            },
        )
    }

    #[test]
    fn a_measurable_subject_becomes_a_sample() {
        let host = mock::Host::with_home("/mixengine");
        host.set_group_reading(
            41,
            StartTime::from_stored(1),
            reading(41, Some(3.0), 40_000),
        );

        let frame = frame_from(&host, Timestamp(60_000), &[service("mariadb@main", 41, 1)]);

        assert_eq!(frame.at, Timestamp(60_000));
        assert_eq!(frame.samples.len(), 1);
        assert_eq!(frame.samples[0].rss_bytes, 40_000);
        assert_eq!(frame.samples[0].cpu_percent, Some(3.0));
    }

    #[test]
    fn a_subject_whose_process_is_gone_is_absent_from_the_frame() {
        let host = mock::Host::with_home("/mixengine");

        let frame = frame_from(
            &host,
            Timestamp(60_000),
            &[service("mariadb@main", 4_242, 1)],
        );

        assert!(
            frame.samples.is_empty(),
            "absent, never a sample of zero: a subject that cannot be measured has no row"
        );
    }

    #[test]
    fn a_subject_whose_pid_was_handed_round_is_absent() {
        let host = mock::Host::with_home("/mixengine");
        host.set_group_reading(
            41,
            StartTime::from_stored(999),
            reading(41, Some(3.0), 40_000),
        );

        let frame = frame_from(&host, Timestamp(60_000), &[service("mariadb@main", 41, 1)]);

        assert!(frame.samples.is_empty());
    }

    #[test]
    fn one_unmeasurable_subject_does_not_take_the_others_with_it() {
        let host = mock::Host::with_home("/mixengine");
        host.set_group_reading(41, StartTime::from_stored(1), reading(41, None, 40_000));

        let frame = frame_from(
            &host,
            Timestamp(60_000),
            &[service("mariadb@main", 41, 1), service("redis@main", 42, 1)],
        );

        assert_eq!(frame.samples.len(), 1);
        assert_eq!(
            frame.samples[0].cpu_percent, None,
            "and a group with no CPU figure yet is still a group that was measured"
        );
    }
}
