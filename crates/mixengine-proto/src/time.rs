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

    /// This moment as `YYYY-MM-DDTHH:MM:SSZ`, which is how `.claude/architecture/data-model.md`
    /// spells an `_at` column that a person reads rather than one the daemon does arithmetic on.
    ///
    /// **T23 is the first task that had to write one at runtime**, which is the mirror of the
    /// discovery T22 recorded: until then every ISO-8601 moment in the schema was a literal in a
    /// fixture, and every moment produced at runtime was a number. `runtime_installs.installed_at`
    /// is neither — it is TEXT by the schema's own convention, and it is written the moment an
    /// install lands.
    ///
    /// So the civil-calendar arithmetic is here, in about thirty lines, rather than bought as a
    /// dependency. That is the same trade [`crate::Millis`] made against a duration crate and the
    /// index client made against a date parser, and it is affordable for the same reason: the
    /// format is one we both write and read, the algorithm is a published one, and the alternative
    /// is a dependency in the crate every client links.
    ///
    /// **Truncated to the second, towards the past.** The column is a record, so the millisecond is
    /// not worth three more characters in something a person reads in a database viewer — and
    /// flooring rather than truncating towards zero is what keeps that true for a clock set before
    /// 1970, where rounding towards zero would move a moment *forwards*.
    #[must_use]
    pub fn to_rfc3339(self) -> String {
        let seconds = self.0.div_euclid(1_000);
        let (days, second_of_day) = (seconds.div_euclid(86_400), seconds.rem_euclid(86_400));
        let (year, month, day) = civil_from_days(days);

        format!(
            "{year:04}-{month:02}-{day:02}T{:02}:{:02}:{:02}Z",
            second_of_day / 3_600,
            (second_of_day / 60) % 60,
            second_of_day % 60
        )
    }

    /// Read back what [`Timestamp::to_rfc3339`] wrote, and nothing else.
    ///
    /// **Narrowed to the one spelling on purpose**, exactly as the index client narrows the shape it
    /// accepts for `generated_at`: `2026-08-14T06:55:12+00:00`, a fractional second and an unpadded
    /// month are all valid RFC 3339, none of them is what this schema's columns hold, and accepting
    /// them would mean a parser with opinions about time zones in a crate that has no clock.
    ///
    /// Ranges are checked, calendars are not — the 31st of February parses, and produces a moment in
    /// March. The value being read is one we wrote a moment or a year ago, not one a user typed.
    #[must_use]
    pub fn parse_rfc3339(text: &str) -> Option<Self> {
        let bytes = text.as_bytes();
        if bytes.len() != 20 || bytes[10] != b'T' || bytes[19] != b'Z' {
            return None;
        }
        if bytes[4] != b'-' || bytes[7] != b'-' || bytes[13] != b':' || bytes[16] != b':' {
            return None;
        }
        // `parse` accepts a leading `+`, so the digits are checked rather than assumed.
        if !bytes
            .iter()
            .enumerate()
            .all(|(at, byte)| matches!(at, 4 | 7 | 10 | 13 | 16 | 19) || byte.is_ascii_digit())
        {
            return None;
        }

        let number = |from: usize, to: usize| text.get(from..to)?.parse::<i64>().ok();
        let (year, month, day) = (number(0, 4)?, number(5, 7)?, number(8, 10)?);
        let (hour, minute, second) = (number(11, 13)?, number(14, 16)?, number(17, 19)?);

        if !(1..=12).contains(&month) || !(1..=31).contains(&day) {
            return None;
        }
        if hour > 23 || minute > 59 || second > 60 {
            return None;
        }

        let days = days_from_civil(year, month, day);
        let seconds = days * 86_400 + hour * 3_600 + minute * 60 + second;

        Some(Self(seconds.checked_mul(1_000)?))
    }
}

/// The civil date `days` days after 1970-01-01, from Howard Hinnant's `chrono`-compatible algorithm.
///
/// Written out rather than depended on, for the reason [`Timestamp::to_rfc3339`] gives. The
/// divisions are deliberately truncating — Rust's `/` on integers is, and the `z - 146096` and
/// `y - 399` adjustments are what the published algorithm uses to make that correct for dates before
/// the epoch. Changing either to `div_euclid` would be changing the algorithm.
fn civil_from_days(days: i64) -> (i64, i64, i64) {
    let z = days + 719_468;
    let era = if z >= 0 { z } else { z - 146_096 } / 146_097;
    let day_of_era = z - era * 146_097;
    let year_of_era =
        (day_of_era - day_of_era / 1_460 + day_of_era / 36_524 - day_of_era / 146_096) / 365;
    let year = year_of_era + era * 400;
    let day_of_year = day_of_era - (365 * year_of_era + year_of_era / 4 - year_of_era / 100);
    let shifted_month = (5 * day_of_year + 2) / 153;
    let day = day_of_year - (153 * shifted_month + 2) / 5 + 1;
    let month = shifted_month + if shifted_month < 10 { 3 } else { -9 };

    (year + i64::from(month <= 2), month, day)
}

/// The inverse of [`civil_from_days`], from the same source.
fn days_from_civil(year: i64, month: i64, day: i64) -> i64 {
    let year = year - i64::from(month <= 2);
    let era = if year >= 0 { year } else { year - 399 } / 400;
    let year_of_era = year - era * 400;
    let day_of_year = (153 * (month + if month > 2 { -3 } else { 9 }) + 2) / 5 + day - 1;
    let day_of_era = year_of_era * 365 + year_of_era / 4 - year_of_era / 100 + day_of_year;

    era * 146_097 + day_of_era - 719_468
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

    /// The shape `.claude/architecture/data-model.md` writes in its own example, produced from the
    /// number this type actually holds.
    #[test]
    fn a_moment_is_also_the_iso_8601_a_text_column_holds() {
        assert_eq!(
            Timestamp(1_723_000_000_500).to_rfc3339(),
            "2024-08-07T03:06:40Z"
        );
        assert_eq!(Timestamp(0).to_rfc3339(), "1970-01-01T00:00:00Z");
    }

    /// Every moment the column can hold survives the trip, which is what makes reading a row back
    /// the same as never having written it.
    #[test]
    fn what_is_written_as_text_is_read_back_as_the_same_moment() {
        for millis in [
            0,
            1_000,
            1_723_000_000_000,
            // A leap day, and the century rule the civil-calendar arithmetic exists for: 2000 is a
            // leap year and 1900 is not, which is exactly what an off-by-one implementation gets
            // wrong.
            951_782_400_000,
            // Before the epoch, where truncating towards zero would move the moment forwards.
            -1_000,
            -2_208_988_800_000,
        ] {
            let stamp = Timestamp(millis);
            assert_eq!(
                Timestamp::parse_rfc3339(&stamp.to_rfc3339()),
                Some(stamp),
                "{millis} as {}",
                stamp.to_rfc3339()
            );
        }
    }

    /// The fraction is dropped rather than rounded, and towards the past on both sides of the
    /// epoch — a moment that moved forwards would be a record of something that had not happened.
    #[test]
    fn a_moment_written_as_text_is_truncated_towards_the_past() {
        assert_eq!(Timestamp(1_500).to_rfc3339(), "1970-01-01T00:00:01Z");
        assert_eq!(Timestamp(-1_500).to_rfc3339(), "1969-12-31T23:59:58Z");
    }

    #[test]
    fn every_other_rfc_3339_spelling_is_refused_rather_than_guessed_at() {
        for text in [
            "2026-08-14T06:55:12+00:00",
            "2026-08-14T06:55:12.5Z",
            "2026-08-14 06:55:12Z",
            "2026-8-14T06:55:12Z",
            "+026-08-14T06:55:12Z",
            "2026-13-14T06:55:12Z",
            "2026-08-14T24:55:12Z",
            "",
        ] {
            assert!(
                Timestamp::parse_rfc3339(text).is_none(),
                "{text:?} should be refused"
            );
        }
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
