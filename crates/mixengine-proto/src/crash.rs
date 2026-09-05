//! What `mixengined` leaves behind when it hits a bug in itself. Roadmap task **T91**.
//!
//! **Every field here is one of three things**: a compile-time constant of the build that wrote it
//! (the location, the version, the target), a literal from `std` or `tokio` (the thread name), or a
//! symbol name out of a backtrace. None of them can hold a value from the home it was written in —
//! not a project's directory, not a site's name, not a password. That is what lets one of these be
//! attached to a public bug report without being read first.
//!
//! **The panic message is deliberately not here.** It is `format!`-ed from whatever was in scope at
//! the moment of a bug, which is the one string in this product nobody reviewed: an `unwrap()` on
//! `mixengine_core::Error::Io` renders the path that error carries. It goes to `daemon.log`
//! instead, where it is on the user's own machine and beside the paths that log has always carried.
//!
//! **Not a redaction pass**, for the reason [`bundle_api`](crate::Part)'s own header gives about
//! one: a filter is a guess that a pattern matched, and it invites the next reader to believe a
//! file is filtered rather than clean. What this module owes instead is the field list below —
//! short enough to read in one screen, which is where the guarantee actually lives.
//!
//! Decided in
//! [ADR 0022](../../../.claude/decisions/0022-a-crash-report-is-recorded-by-default-and-sent-by-nothing.md).

use crate::{DaemonVersion, Timestamp};

/// The number [`CrashReport::format`] carries, so a reader that does not know a shape stops rather
/// than guessing at one — [`MANIFEST_FORMAT`](crate::MANIFEST_FORMAT)'s reasoning.
pub const CRASH_FORMAT: u32 = 1;

/// Where in **this repository's own source** a panic was raised.
///
/// [`file`](Self::file) is `std::panic::Location::file`, which is the path as it was written in the
/// source tree — `crates/mixengine-daemon/src/…` — and is a `&'static str` baked into the binary
/// rather than a directory on anybody's disk.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(deny_unknown_fields)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS), ts(export))]
pub struct CrashLocation {
    /// The source file, as this repository spells it.
    pub file: String,

    /// The line.
    pub line: u32,

    /// The column.
    pub column: u32,
}

/// One panic, as much of it as may travel.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(deny_unknown_fields)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS), ts(export))]
pub struct CrashReport {
    /// [`CRASH_FORMAT`].
    pub format: u32,

    /// When the hook ran, which is within microseconds of the panic.
    pub recorded_at: Timestamp,

    /// What was running, and what it spoke.
    pub daemon: DaemonVersion,

    /// `std::env::consts::OS`.
    pub os: String,

    /// `std::env::consts::ARCH`.
    pub arch: String,

    /// The panicking thread's name, or [`None`] when it had none.
    ///
    /// Every thread name reachable here is a literal from `std` or `tokio` — nothing in this
    /// workspace names a thread — so this is one of the three safe kinds of field the module header
    /// lists rather than an exception to them.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub thread: Option<String>,

    /// Where it was raised, or [`None`] when `std` reported none.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub location: Option<CrashLocation>,

    /// The backtrace, as symbol names alone.
    ///
    /// **Symbol names and nothing else**: the `at <path>:<line>` lines a rendered backtrace carries
    /// are the one place a build machine's directories appear, and they are dropped before a report
    /// is built. Empty is an ordinary answer rather than a failure — a stripped build on a system
    /// with no unwind tables has a location and no frames, which is still most of what a reader
    /// needs.
    pub frames: Vec<String>,
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ProtocolVersion;

    fn sample() -> CrashReport {
        CrashReport {
            format: CRASH_FORMAT,
            recorded_at: Timestamp(1_757_000_000_000),
            daemon: DaemonVersion {
                version: "0.1.0".to_owned(),
                protocol: ProtocolVersion(1),
            },
            os: "windows".to_owned(),
            arch: "x86_64".to_owned(),
            thread: Some("tokio-runtime-worker".to_owned()),
            location: Some(CrashLocation {
                file: "crates/mixengine-daemon/src/services/mod.rs".to_owned(),
                line: 412,
                column: 9,
            }),
            frames: vec!["mixengine_daemon::services::start".to_owned()],
        }
    }

    /// The whole of what a report is, in both directions.
    #[test]
    fn a_report_round_trips() {
        let encoded = serde_json::to_string(&sample()).expect("a report serialises");
        let back: CrashReport = serde_json::from_str(&encoded).expect("and comes back");

        assert_eq!(back, sample());
    }

    /// An absent thread and an absent location are absent on the wire rather than null, which is
    /// this crate's rule: a fact nobody established is missing, not empty.
    #[test]
    fn what_std_did_not_report_is_absent_rather_than_null() {
        let bare = CrashReport {
            thread: None,
            location: None,
            frames: Vec::new(),
            ..sample()
        };

        let encoded = serde_json::to_string(&bare).expect("a report serialises");

        assert!(!encoded.contains("thread"), "{encoded}");
        assert!(!encoded.contains("location"), "{encoded}");
        assert!(encoded.contains("\"frames\":[]"), "{encoded}");
    }

    /// A report edited by hand into a shape this build does not know is refused, rather than read
    /// with the unknown half silently dropped — [`DiagnosticsBundle`](crate::DiagnosticsBundle)'s
    /// rule, for the same reason.
    #[test]
    fn an_unknown_field_is_refused_rather_than_ignored() {
        let mut value = serde_json::to_value(sample()).expect("a report serialises");
        value["message"] = serde_json::json!("this is not a field a report has");

        assert!(serde_json::from_value::<CrashReport>(value).is_err());
    }
}
