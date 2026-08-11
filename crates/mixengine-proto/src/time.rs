//! The one way a moment is written on the wire.

use std::time::{Duration, SystemTime, UNIX_EPOCH};

/// A moment, as milliseconds since the Unix epoch.
///
/// Not [`SystemTime`], whose serde representation is a two-field object nobody would choose, and not
/// an RFC 3339 string, which cannot be produced without a date library — and the crate list in
/// `.claude/standards/rust.md` has none, because until now nothing needed one. A number is
/// unambiguous, sorts, needs no parser, and is `new Date(ms)` in the GUI.
///
/// Milliseconds rather than seconds because log lines (roadmap task T14) arrive faster than one a
/// second and would otherwise all carry the same timestamp; signed rather than unsigned because a
/// machine whose clock is set before 1970 should produce a strange number instead of a panic.
#[derive(
    Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, serde::Serialize, serde::Deserialize,
)]
#[serde(transparent)]
pub struct Timestamp(pub i64);

impl Timestamp {
    /// The moment a [`SystemTime`] names.
    ///
    /// Takes the clock reading rather than taking it itself, so this crate keeps the property that
    /// none of it touches the world — the daemon passes `SystemTime::now()`.
    ///
    /// Saturates instead of failing. The only inputs that overflow are clocks set roughly 292
    /// million years from now, and a status line is not the place to turn one into an error.
    #[must_use]
    pub fn from_system_time(time: SystemTime) -> Self {
        let millis = match time.duration_since(UNIX_EPOCH) {
            Ok(since) => i64::try_from(since.as_millis()).unwrap_or(i64::MAX),
            // Before 1970: the difference comes back in the error, and the sign is put back on.
            Err(before) => {
                i64::try_from(before.duration().as_millis()).map_or(i64::MIN, |millis| -millis)
            }
        };

        Self(millis)
    }
}

/// How long something has been going on, in whole seconds.
///
/// Separate from [`Timestamp`] because it answers a different question and is read differently: a
/// client renders it as "up 3 days", never as a date. Whole seconds because nothing displays an
/// uptime more precisely than that.
#[derive(
    Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, serde::Serialize, serde::Deserialize,
)]
#[serde(transparent)]
pub struct Uptime(pub u64);

impl Uptime {
    /// Truncated to whole seconds, which is the resolution the type promises.
    #[must_use]
    pub fn from_duration(elapsed: Duration) -> Self {
        Self(elapsed.as_secs())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_moment_is_a_plain_number_of_milliseconds() {
        let time = UNIX_EPOCH + Duration::from_millis(1_723_000_000_500);
        let stamp = Timestamp::from_system_time(time);

        assert_eq!(stamp, Timestamp(1_723_000_000_500));
        assert_eq!(serde_json::to_string(&stamp).unwrap(), "1723000000500");
    }

    #[test]
    fn a_clock_set_before_the_epoch_goes_negative_rather_than_panicking() {
        let time = UNIX_EPOCH - Duration::from_millis(1_500);

        assert_eq!(Timestamp::from_system_time(time), Timestamp(-1_500));
    }

    #[test]
    fn an_uptime_is_whole_seconds() {
        assert_eq!(
            Uptime::from_duration(Duration::from_millis(3_999)),
            Uptime(3)
        );
        assert_eq!(serde_json::to_string(&Uptime(3)).unwrap(), "3");
    }
}
