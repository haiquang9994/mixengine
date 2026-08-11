//! The one way a moment, and a length of time, are written on the wire.

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

/// A length of time, in milliseconds.
///
/// The third time type, and the last: [`Timestamp`] is when something happened, [`Uptime`] is how
/// long ago that was in the resolution a person reads, and this is how long something is *allowed*
/// to take. Not [`Duration`], whose serde form is a `{secs, nanos}` object; not [`Uptime`], because a
/// restart backoff starts at 500 ms and whole seconds would round it to nothing.
///
/// **Written as a number, read as a number or as a human string.** The canonical form — what a
/// daemon sends and what a client should expect — is milliseconds as an integer. Deserialisation
/// additionally accepts `"500ms"`, `"10s"`, `"5m"`, `"2h"`, because the other direction this type
/// travels is a hand-written `extension.toml`, where `timeout = "10s"` is what an author will write
/// and `timeout = 10000` is what they will get wrong. Whole numbers only: `"1.5s"` is rejected
/// rather than rounded, since the unit to say it in already exists.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Default, serde::Serialize)]
#[serde(transparent)]
pub struct Millis(pub u64);

impl Millis {
    /// A whole number of seconds.
    #[must_use]
    pub const fn from_secs(secs: u64) -> Self {
        Self(secs.saturating_mul(1_000))
    }

    /// The [`Duration`] a caller actually sleeps on or times out with.
    #[must_use]
    pub const fn as_duration(self) -> Duration {
        Duration::from_millis(self.0)
    }

    /// Whether this is a real length of time.
    ///
    /// A zero timeout, interval or grace period is almost always a field somebody forgot rather
    /// than an instruction, so the builders in [`crate::ServiceSpec`] reject one. Kept here so
    /// every check asks the question the same way.
    #[must_use]
    pub const fn is_zero(self) -> bool {
        self.0 == 0
    }

    /// The multiplier each unit suffix stands for.
    const UNITS: [(&'static str, u64); 4] =
        [("ms", 1), ("s", 1_000), ("m", 60_000), ("h", 3_600_000)];

    /// Parse `"500ms"`, `"10s"`, `"5m"` or `"2h"`.
    ///
    /// `ms` is tried before `s` because `s` is a suffix of it and the shorter match would read
    /// `"500ms"` as 500 000 milliseconds worth of `"500m"`.
    fn parse(text: &str) -> Option<Self> {
        let text = text.trim();

        let (digits, multiplier) = Self::UNITS
            .iter()
            .find_map(|(suffix, multiplier)| Some((text.strip_suffix(suffix)?, *multiplier)))?;

        // `u64::from_str` accepts a leading `+`, which nothing should be writing here.
        if digits.is_empty() || !digits.bytes().all(|byte| byte.is_ascii_digit()) {
            return None;
        }

        digits
            .parse::<u64>()
            .ok()?
            .checked_mul(multiplier)
            .map(Self)
    }
}

impl std::fmt::Display for Millis {
    /// The largest whole unit that loses nothing — `"10s"` rather than `"10000ms"`.
    ///
    /// For messages and logs, never for the wire, which is always the number.
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let Some((suffix, multiplier)) = Self::UNITS
            .iter()
            .rev()
            .find(|(_, multiplier)| self.0 != 0 && self.0.is_multiple_of(*multiplier))
        else {
            return write!(f, "{}ms", self.0);
        };

        write!(f, "{}{suffix}", self.0 / multiplier)
    }
}

impl<'de> serde::Deserialize<'de> for Millis {
    fn deserialize<D: serde::Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        /// Signed, so a negative number produces "a length of time cannot be negative" instead of
        /// serde's "data did not match any variant" from failing to be a `u64` and then failing to
        /// be a string.
        #[derive(serde::Deserialize)]
        #[serde(untagged)]
        enum Repr {
            Number(i64),
            Text(String),
        }

        match Repr::deserialize(deserializer)? {
            Repr::Number(millis) => u64::try_from(millis).map(Millis).map_err(|_| {
                serde::de::Error::custom(format!("a length of time cannot be negative: {millis}"))
            }),
            Repr::Text(text) => Millis::parse(&text).ok_or_else(|| {
                serde::de::Error::custom(format!(
                    "expected a whole number of milliseconds or a duration like \"10s\", got {text:?}"
                ))
            }),
        }
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

    #[test]
    fn a_length_of_time_is_always_written_as_a_number() {
        assert_eq!(
            serde_json::to_string(&Millis::from_secs(10)).unwrap(),
            "10000"
        );
        assert_eq!(
            serde_json::from_str::<Millis>("10000").unwrap(),
            Millis(10_000)
        );
    }

    #[test]
    fn a_length_of_time_is_also_readable_as_a_human_wrote_it() {
        for (text, expected) in [
            ("500ms", 500),
            ("10s", 10_000),
            ("5m", 300_000),
            ("2h", 7_200_000),
            (" 30s ", 30_000),
        ] {
            assert_eq!(
                serde_json::from_str::<Millis>(&format!("\"{text}\"")).unwrap(),
                Millis(expected),
                "{text}"
            );
        }
    }

    /// `ms` has to win over `s`, or every millisecond value is read as minutes.
    #[test]
    fn milliseconds_are_not_read_as_minutes() {
        assert_eq!(Millis::parse("500ms"), Some(Millis(500)));
        assert_ne!(Millis::parse("500ms"), Millis::parse("500m"));
    }

    #[test]
    fn a_length_of_time_refuses_what_it_cannot_mean_exactly() {
        for text in [
            "1.5s",
            "10",
            "s",
            "-5s",
            "+5s",
            "ten seconds",
            "9999999999999999999h",
        ] {
            assert!(Millis::parse(text).is_none(), "{text} should not parse");
        }
    }

    #[test]
    fn a_negative_number_says_so_rather_than_failing_to_be_a_string() {
        let error = serde_json::from_str::<Millis>("-1")
            .unwrap_err()
            .to_string();

        assert!(error.contains("cannot be negative"), "{error}");
    }

    #[test]
    fn a_length_of_time_prints_in_the_largest_unit_that_loses_nothing() {
        assert_eq!(Millis(7_200_000).to_string(), "2h");
        assert_eq!(Millis(30_000).to_string(), "30s");
        assert_eq!(Millis(500).to_string(), "500ms");
        assert_eq!(Millis(0).to_string(), "0ms");
        assert_eq!(Millis(90_000).to_string(), "90s");
    }
}
