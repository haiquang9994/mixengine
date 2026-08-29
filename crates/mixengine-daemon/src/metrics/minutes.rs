//! Readings in, minutes out — roadmap task **T71**.
//!
//! **The minute being assembled lives here and not in SQL.** Sixty readings against a
//! read-modify-write would be sixty transactions for one row, once a minute, for as long as somebody
//! watched; a daemon that dies loses at most the partial minute, which is the same thing a daemon
//! that dies loses about everything else.
//!
//! **A subject with no reading contributes no row.** A minute with no row means *nobody measured* —
//! the service was stopped, the machine was asleep, the daemon was being replaced — which is not the
//! same fact as *nothing was used*. Writing zeroes would turn one into the other, and a chart drawn
//! from them would show a service idling through a night it was not running at all.

use std::collections::BTreeMap;

use mixengine_proto::{MetricsFrame, MetricsMinute, MetricsSample, MetricsSubject, Timestamp};

/// How many milliseconds a minute is.
const MINUTE: i64 = 60_000;

/// The minute `at` falls in.
pub(crate) const fn minute_of(at: Timestamp) -> Timestamp {
    Timestamp(at.0.div_euclid(MINUTE) * MINUTE)
}

/// One subject's minute, still being assembled.
#[derive(Debug, Default)]
struct InHand {
    /// The sum of the CPU figures that were taken, and how many there were.
    ///
    /// Two counters rather than one, because a reading with no figure is not a reading of zero: it
    /// must not pull the average down, and it must still count towards `samples`.
    cpu_total: f64,
    cpu_readings: u32,
    cpu_peak: Option<f32>,

    /// `u128` because sixty readings of a service holding tens of gigabytes is still exact here and
    /// would not be in a `u64` on a machine with enough memory to matter.
    rss_total: u128,
    rss_peak: u64,

    /// Every reading, whether or not it carried a CPU figure.
    samples: u32,
}

impl InHand {
    /// Fold one reading in.
    fn observe(&mut self, sample: &MetricsSample) {
        if let Some(cpu) = sample.cpu_percent {
            self.cpu_total += f64::from(cpu);
            self.cpu_readings += 1;
            self.cpu_peak = Some(self.cpu_peak.map_or(cpu, |peak| peak.max(cpu)));
        }

        self.rss_total += u128::from(sample.rss_bytes);
        self.rss_peak = self.rss_peak.max(sample.rss_bytes);
        self.samples += 1;
    }

    /// The row this minute became.
    fn finish(self, subject: MetricsSubject, minute: Timestamp) -> MetricsMinute {
        // `max(1)` cannot be reached — an entry exists only because a reading created it — and is
        // here so that the division is total rather than trusting that.
        let readings = u128::from(self.samples.max(1));

        MetricsMinute {
            subject,
            minute,
            // Over the readings that carried a figure, not over all of them.
            cpu_avg: (self.cpu_readings > 0).then(|| {
                let mean = self.cpu_total / f64::from(self.cpu_readings);

                // A percentage of one core on a machine with a few hundred of them is nowhere near
                // f32's range; the cast is exact for every value this can hold.
                mean as f32
            }),
            cpu_peak: self.cpu_peak,
            rss_avg: u64::try_from(self.rss_total / readings).unwrap_or(u64::MAX),
            rss_peak: self.rss_peak,
            samples: self.samples,
        }
    }
}

/// The minute every subject is in, and the readings taken in it so far.
#[derive(Debug, Default)]
pub(crate) struct Accumulator {
    /// Which minute is being assembled. [`None`] before the first reading and after a drain.
    minute: Option<Timestamp>,

    /// One entry per subject seen in this minute.
    in_hand: BTreeMap<MetricsSubject, InHand>,
}

impl Accumulator {
    /// Take one frame in, and hand back whatever minute it completed.
    ///
    /// **Any change of minute finishes what was in hand, not only a later one.** A machine whose
    /// clock is corrected backwards would otherwise fold two moments into one row, and a row that
    /// mixes two minutes is worse than two rows that each name theirs.
    pub(crate) fn observe(&mut self, frame: &MetricsFrame) -> Vec<MetricsMinute> {
        let minute = minute_of(frame.at);

        let finished = if self.minute.is_some_and(|held| held != minute) {
            self.drain()
        } else {
            Vec::new()
        };

        self.minute = Some(minute);

        for sample in &frame.samples {
            self.in_hand
                .entry(sample.subject.clone())
                .or_default()
                .observe(sample);
        }

        finished
    }

    /// Finish the minute in hand, whatever the clock says.
    ///
    /// What a shutdown calls: a daemon stopping at forty seconds past would otherwise throw away two
    /// thirds of a minute it had already measured. Draining twice hands back nothing the second
    /// time, because the minute is taken rather than copied.
    pub(crate) fn drain(&mut self) -> Vec<MetricsMinute> {
        let Some(minute) = self.minute.take() else {
            return Vec::new();
        };

        std::mem::take(&mut self.in_hand)
            .into_iter()
            .map(|(subject, in_hand)| in_hand.finish(subject, minute))
            .collect()
    }
}

#[cfg(test)]
mod tests {
    use mixengine_proto::ServiceId;

    use super::*;

    fn sample(cpu: Option<f32>, rss: u64) -> MetricsSample {
        MetricsSample {
            subject: MetricsSubject::Service(ServiceId::parse("mariadb@main").expect("an id")),
            cpu_percent: cpu,
            rss_bytes: rss,
            processes: 1,
        }
    }

    fn frame(at: i64, cpu: Option<f32>, rss: u64) -> MetricsFrame {
        MetricsFrame {
            at: Timestamp(at),
            samples: vec![sample(cpu, rss)],
        }
    }

    fn nothing_measured(at: i64) -> MetricsFrame {
        MetricsFrame {
            at: Timestamp(at),
            samples: Vec::new(),
        }
    }

    #[test]
    fn a_minute_is_the_moment_truncated_to_it() {
        assert_eq!(minute_of(Timestamp(60_999)), Timestamp(60_000));
        assert_eq!(minute_of(Timestamp(60_000)), Timestamp(60_000));
    }

    #[test]
    fn nothing_is_written_until_the_minute_rolls() {
        let mut accumulator = Accumulator::default();

        assert!(
            accumulator
                .observe(&frame(60_000, Some(10.0), 100))
                .is_empty()
        );
        assert!(
            accumulator
                .observe(&frame(60_500, Some(30.0), 300))
                .is_empty()
        );
    }

    #[test]
    fn a_rolled_minute_carries_its_averages_its_peaks_and_its_count() {
        let mut accumulator = Accumulator::default();

        accumulator.observe(&frame(60_000, Some(10.0), 100));
        accumulator.observe(&frame(60_500, Some(30.0), 300));

        let rolled = accumulator.observe(&frame(120_000, Some(1.0), 1));

        assert_eq!(rolled.len(), 1);
        assert_eq!(rolled[0].minute, Timestamp(60_000));
        assert_eq!(rolled[0].samples, 2);
        assert_eq!(rolled[0].cpu_avg, Some(20.0));
        assert_eq!(rolled[0].cpu_peak, Some(30.0));
        assert_eq!(rolled[0].rss_avg, 200);
        assert_eq!(rolled[0].rss_peak, 300);
    }

    #[test]
    fn a_minute_of_one_reading_says_so() {
        let mut accumulator = Accumulator::default();

        accumulator.observe(&frame(60_000, Some(10.0), 100));
        let rolled = accumulator.observe(&frame(120_000, Some(10.0), 100));

        assert_eq!(rolled[0].samples, 1);
        assert_eq!(
            rolled[0].cpu_peak, rolled[0].cpu_avg,
            "the peak is the largest of what was looked at, and once is once"
        );
    }

    #[test]
    fn a_minute_no_reading_carried_a_cpu_figure_has_none() {
        let mut accumulator = Accumulator::default();

        accumulator.observe(&frame(60_000, None, 100));
        let rolled = accumulator.observe(&frame(120_000, None, 100));

        assert_eq!(
            rolled[0].cpu_avg, None,
            "never a zero standing in for a refusal"
        );
        assert_eq!(rolled[0].cpu_peak, None);
        assert_eq!(rolled[0].rss_avg, 100, "memory was measured all the same");
    }

    #[test]
    fn a_cpu_average_is_over_the_readings_that_had_one() {
        let mut accumulator = Accumulator::default();

        accumulator.observe(&frame(60_000, None, 100));
        accumulator.observe(&frame(60_500, Some(30.0), 100));
        let rolled = accumulator.observe(&frame(120_000, None, 100));

        assert_eq!(
            rolled[0].cpu_avg,
            Some(30.0),
            "a reading with no figure must not pull the average down"
        );
        assert_eq!(rolled[0].samples, 2, "and it still counts as a reading");
    }

    #[test]
    fn a_subject_that_stops_being_measured_rolls_out_and_produces_nothing_after() {
        let mut accumulator = Accumulator::default();

        accumulator.observe(&frame(60_000, Some(10.0), 100));

        let rolled = accumulator.observe(&nothing_measured(120_000));
        assert_eq!(rolled.len(), 1, "the minute it did have is written");

        assert!(
            accumulator.observe(&nothing_measured(180_000)).is_empty(),
            "a stopped service produces no rows, never rows of zero"
        );
    }

    #[test]
    fn a_clock_corrected_backwards_finishes_the_minute_rather_than_folding_two_into_one() {
        let mut accumulator = Accumulator::default();

        accumulator.observe(&frame(120_000, Some(10.0), 100));
        let rolled = accumulator.observe(&frame(60_000, Some(10.0), 100));

        assert_eq!(rolled.len(), 1);
        assert_eq!(rolled[0].minute, Timestamp(120_000));
    }

    #[test]
    fn a_shutdown_writes_the_minute_in_hand() {
        let mut accumulator = Accumulator::default();

        accumulator.observe(&frame(60_000, Some(10.0), 100));

        assert_eq!(accumulator.drain().len(), 1);
        assert!(accumulator.drain().is_empty(), "and nothing twice");
    }
}
