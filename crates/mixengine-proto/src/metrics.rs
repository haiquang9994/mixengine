//! What a service is costing, now and over the last day — roadmap task **T71**.
//!
//! Three tenses and one type each. [`MetricsFrame`] is *now*: what `metrics.snapshot` answers and
//! what every frame of `GET /metrics` carries. [`MetricsMinute`] is *just now*: one row of the
//! 24-hour history, which `metrics.history` reads. There is no third shape for *from here on*,
//! because the stream carries the first one — a subscription that invented a type of its own would
//! be a second description of the same reading.

use std::fmt;

use serde::{Deserialize, Serialize};

use crate::{ServiceId, Timestamp};

/// Whose reading this is.
///
/// **A closed set rather than a string**, so a client matches on it rather than recognising it. Its
/// [`Display`](fmt::Display) is also its wire spelling *and* its column spelling: one encoding,
/// defined once, so a row read back out of the database is a subject rather than a parse of one.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord)]
// `"daemon"` or `"service:<id>"` in one string — the shape `Display` writes and `parse` reads, and
// what this type's hand-written serde puts on the wire. The grammar stays in
// [`MetricsSubject::parse`] rather than becoming a TypeScript template literal nothing in this
// repository could check against it (roadmap task T56).
#[cfg_attr(feature = "ts", derive(ts_rs::TS), ts(export, as = "String"))]
pub enum MetricsSubject {
    /// `mixengined` itself, and whatever it started that is not a service.
    ///
    /// Measured because the footprint this project defends is *daemon plus web server*, and a client
    /// that could not ask the daemon what it costs would have to go behind its back to the operating
    /// system to draw the larger half of that number.
    ///
    /// **It does not include the services it supervises**, although every one of them is a child of
    /// this process — a group stops where another group begins. The rows are disjoint on purpose:
    /// otherwise the daemon would be the largest consumer on every chart, and a client adding the
    /// rows up would count each service twice. Corrected at **T72**, which is what summing them was
    /// first needed for.
    Daemon,

    /// One supervised service, and everything its process started.
    ///
    /// Except a process that is a subject of its own, which cannot happen today — no service is
    /// started by another — and which the boundary rule above would put under its own row if it did.
    Service(ServiceId),
}

impl MetricsSubject {
    /// Read one back, or [`None`] where this build cannot make sense of it.
    ///
    /// **The `service:` prefix is load-bearing rather than decorative.**
    /// [`ServiceId::parse`](crate::ServiceId::parse) accepts a bare name, so a service may legally
    /// be called `daemon`; one spelling for both would hand that service the daemon's own history.
    /// `:` is not in a service id's alphabet, which makes the two spaces disjoint by the same rule
    /// that validates the ids.
    #[must_use]
    pub fn parse(value: &str) -> Option<Self> {
        if value == "daemon" {
            return Some(Self::Daemon);
        }

        ServiceId::parse(value.strip_prefix("service:")?)
            .ok()
            .map(Self::Service)
    }
}

impl fmt::Display for MetricsSubject {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Daemon => formatter.write_str("daemon"),
            Self::Service(id) => write!(formatter, "service:{id}"),
        }
    }
}

impl Serialize for MetricsSubject {
    fn serialize<S: serde::Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        serializer.collect_str(self)
    }
}

impl<'de> Deserialize<'de> for MetricsSubject {
    fn deserialize<D: serde::Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        let value = String::deserialize(deserializer)?;

        Self::parse(&value).ok_or_else(|| {
            serde::de::Error::custom(format!("`{value}` names neither the daemon nor a service"))
        })
    }
}

/// One subject's reading, taken as part of a [`MetricsFrame`].
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS), ts(export))]
pub struct MetricsSample {
    /// Whose reading this is.
    pub subject: MetricsSubject,

    /// Percentage of **one** core, so 250 is two and a half cores' worth.
    ///
    /// The unit [`ResourceLimits::cpu_percent`](crate::ResourceLimits) is declared in, deliberately:
    /// a client that offers a cap and then draws the usage never converts between the two.
    ///
    /// [`None`] where no figure could be taken, and **never `0.0` for that case** — a CPU reading is
    /// a difference between two moments, and the first reading of a group has nothing to subtract
    /// from. A zero there would draw an idle service during the second it is most expensive.
    pub cpu_percent: Option<f32>,

    /// Resident bytes over the whole group, with shared pages counted once per process.
    ///
    /// **Not the quantity a `memory_mb` limit is judged against** — see
    /// [`MemoryMeasure`](crate::MemoryMeasure), which is commit charge on Windows and charged pages
    /// on Linux. A client rendering this and a limit as one ratio is rendering two different
    /// measurements as though they were one.
    pub rss_bytes: u64,

    /// How many processes the group holds, the root included.
    pub processes: u32,
}

/// Every subject that could be measured, in one pass.
///
/// **The moment belongs to the frame and not to the sample.** One refresh produced all of these, so
/// a timestamp repeated once per service would be a value free to disagree with itself the day
/// somebody assembled a frame out of two readings.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS), ts(export))]
pub struct MetricsFrame {
    /// When the pass was taken.
    pub at: Timestamp,

    /// One per subject that could be measured. **A subject that could not is absent** — never a
    /// sample of zero, because *not measured* and *measured nothing* are different facts.
    pub samples: Vec<MetricsSample>,
}

/// One subject's minute, out of the 24-hour history.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS), ts(export))]
pub struct MetricsMinute {
    /// Whose minute this is.
    pub subject: MetricsSubject,

    /// The minute itself, truncated to it.
    pub minute: Timestamp,

    /// The mean of the readings that carried a CPU figure, or [`None`] where none did.
    pub cpu_avg: Option<f32>,

    /// The largest CPU figure among them.
    ///
    /// **The largest of what was looked at, not of the minute.** Where [`samples`](Self::samples) is
    /// 1 it equals the average, and that is what the pair means. It exists because "what was eating
    /// my battery" is a question about spikes: a service holding 900 MB for two seconds and 200 MB
    /// for the rest of the minute leaves nothing behind in an average.
    pub cpu_peak: Option<f32>,

    /// The mean resident size over the readings.
    pub rss_avg: u64,

    /// The largest resident size among them.
    pub rss_peak: u64,

    /// **How many readings this row is made of** — 1 while nobody was watching, up to 60 while
    /// somebody was.
    ///
    /// An average of one reading and an average of sixty may both be drawn, but not as though they
    /// were equally supported, and the row has to carry the difference for that to be possible.
    pub samples: u32,
}

/// What a client wants out of the history.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields, default)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS), ts(export))]
pub struct MetricsHistoryQuery {
    /// One subject, or every subject when absent.
    pub subject: Option<MetricsSubject>,

    /// The oldest minute wanted. Absent is as far back as the history goes.
    pub since: Option<Timestamp>,

    /// The newest minute wanted. Absent is now.
    pub until: Option<Timestamp>,
}

/// The rows a [`MetricsHistoryQuery`] found, oldest first.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS), ts(export))]
pub struct MetricsHistory {
    /// The rows, ordered by minute and then by subject.
    ///
    /// **A minute with no row for a subject is a minute nobody measured** — the service was stopped,
    /// or the machine was asleep, or the daemon was being replaced. It is never a minute in which
    /// nothing was used, so a client draws a gap rather than joining the points across it.
    pub minutes: Vec<MetricsMinute>,

    /// How long this home keeps a row, so a client can say why its chart starts where it does.
    pub retention_hours: u32,
}

#[cfg(test)]
mod tests {
    use super::*;

    fn service(id: &str) -> MetricsSubject {
        MetricsSubject::Service(ServiceId::parse(id).expect("an id"))
    }

    #[test]
    fn a_subject_is_one_string_on_the_wire_and_in_the_column() {
        assert_eq!(MetricsSubject::Daemon.to_string(), "daemon");
        assert_eq!(service("mariadb@main").to_string(), "service:mariadb@main");
        assert_eq!(
            serde_json::to_value(service("mariadb@main")).expect("serialises"),
            serde_json::json!("service:mariadb@main")
        );
    }

    #[test]
    fn a_service_named_daemon_is_not_the_daemon() {
        // `ServiceId::parse` accepts a bare name, so `daemon` is a legal service id. The prefix is
        // the whole of what keeps its history out of the daemon's own.
        assert_ne!(
            service("daemon").to_string(),
            MetricsSubject::Daemon.to_string()
        );
        assert_eq!(
            MetricsSubject::parse("service:daemon"),
            Some(service("daemon"))
        );
        assert_eq!(
            MetricsSubject::parse("daemon"),
            Some(MetricsSubject::Daemon)
        );
    }

    #[test]
    fn a_subject_this_build_cannot_read_is_none_rather_than_a_panic() {
        assert_eq!(MetricsSubject::parse("service:NOT AN ID"), None);
        assert_eq!(MetricsSubject::parse("nonsense"), None);
        assert_eq!(MetricsSubject::parse("service:"), None);
    }

    #[test]
    fn a_frame_round_trips_and_an_absent_figure_travels_as_null() {
        let frame = MetricsFrame {
            at: Timestamp(1_700_000_000_000),
            samples: vec![MetricsSample {
                subject: MetricsSubject::Daemon,
                cpu_percent: None,
                rss_bytes: 42_000,
                processes: 1,
            }],
        };

        let json = serde_json::to_string(&frame).expect("serialises");
        let read: MetricsFrame = serde_json::from_str(&json).expect("deserialises");

        assert_eq!(read, frame);
        assert!(
            json.contains(r#""cpu_percent":null"#),
            "a figure that could not be taken travels as null, never as a zero"
        );
    }

    #[test]
    fn a_history_query_defaults_to_everything() {
        let query: MetricsHistoryQuery = serde_json::from_str("{}").expect("deserialises");

        assert_eq!(query, MetricsHistoryQuery::default());
        assert!(query.subject.is_none() && query.since.is_none() && query.until.is_none());
    }
}
