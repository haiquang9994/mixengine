//! `daemon.bundle`'s types — the wire answer and the archive's own schema. Roadmap task **T93**.
//!
//! **The archive's schema lives here and not in the daemon**, although [`Manifest`] and
//! [`PlatformFacts`] never travel on the wire. A bundle is read by whoever was sent one, which is
//! rarely the machine that produced it and need not be a MixEngine build at all; putting the shape
//! in the crate every client already links is what lets a client open one without a daemon.
//!
//! **What is *not* here is a redaction pass.**
//! `.claude/decisions/0006-servicespec-in-proto-and-secret-free.md` keeps a credential out of the
//! spec, the database and the log at the type level, and names this bundle while doing so. A filter
//! layered on top would be a guess that a pattern matched — and worse than nothing, because it would
//! invite the next reader to believe the log is filtered rather than clean.
//!
//! What this module owes instead is [`Part`]: a closed list, so that nothing arrives in an archive
//! because a directory happened to hold it.

use crate::{DaemonVersion, Timestamp};

/// What a caller asks for.
///
/// Empty, and a struct rather than no parameter at all, so that the first option to arrive is a
/// field and not an API change. The shape [`DoctorRepair`](crate::DoctorRepair) started with.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, serde::Serialize, serde::Deserialize)]
#[serde(deny_unknown_fields)]
pub struct DiagnosticsBundle {}

/// Everything this build puts in a bundle.
///
/// **Closed, and never a directory walk.** `<root>/certs/` holds the internal CA's private key,
/// `<root>/data/` the user's databases, and `<root>/run/` what stands between a local process and
/// the daemon. A sweep written today omits all three because whoever wrote it remembered; a sweep
/// still omits them next year only if everyone who ever adds a file to the home happens to think
/// about an archive somebody emails. A variant is something a person wrote down.
///
/// The compiler enforces most of that: a variant added here stops [`file_name`](Self::file_name)
/// compiling, and stops the daemon's own `match` compiling with it. What it cannot enforce is
/// [`ALL`](Self::ALL) — see the test in this module for where that gap is caught instead.
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Part {
    /// What this bundle is, what is in it, and what was left out.
    Manifest,

    /// `daemon.doctor`'s answer, as the API sends it.
    ///
    /// The report and not a rendering of it: an archive carrying `mix doctor`'s text would change
    /// when that command's margins change, and would lose the [`ProblemId`](crate::ProblemId) a
    /// reader wants to search for.
    Doctor,

    /// `daemon.status`, as the API sends it.
    Status,

    /// The facts a reader needs to place the rest on a machine.
    Platform,

    /// The tail of `daemon.log`.
    DaemonLog,
}

impl Part {
    /// Every part, in the order they are packed.
    ///
    /// [`Manifest`](Self::Manifest) is **last**: it names the parts, and which parts there are is
    /// not settled until the ones before it have been written.
    pub const ALL: [Self; 5] = [
        Self::Doctor,
        Self::Status,
        Self::Platform,
        Self::DaemonLog,
        Self::Manifest,
    ];

    /// The name this part has inside the archive.
    #[must_use]
    pub fn file_name(self) -> &'static str {
        match self {
            Self::Manifest => "manifest.json",
            Self::Doctor => "doctor.json",
            Self::Status => "status.json",
            Self::Platform => "platform.json",
            Self::DaemonLog => "daemon.log",
        }
    }
}

/// The number [`Manifest::format`] carries, so a reader that does not know a shape stops rather
/// than guessing at one.
pub const MANIFEST_FORMAT: u32 = 1;

/// One part that was written, and what it cost.
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct Member {
    /// Which part.
    pub part: Part,

    /// Its size before compression.
    pub bytes: u64,
}

/// Something this bundle does not carry, and why.
///
/// **A field and not a comment in a document nobody reads.** A bundle silent about where it did not
/// look is a bundle claiming it looked everywhere, and the person reading the bug report is the one
/// who pays for that.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct Omission {
    /// What was left out, named as a person would say it: `"etc/"`, `"status.json"`.
    pub name: String,

    /// Why. Either a decision, or the error that prevented it — never an apology.
    pub because: String,
}

/// Where the log excerpt begins, in numbers.
///
/// **An excerpt silent about where it starts is an excerpt claiming to be the whole log.** The
/// difference between "nothing was logged before this" and "1.4 GB was logged before this" is what a
/// reader needs, and it is three numbers.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, serde::Serialize, serde::Deserialize)]
pub struct LogExcerpt {
    /// Bytes of `daemon.log` in the archive.
    pub included_bytes: u64,

    /// Bytes of the current file older than the cut, the partial line dropped at it included.
    pub skipped_bytes: u64,

    /// Rotated files beside it that are not here.
    pub rotated_files: u32,
}

/// A range this operating system will not let anything bind.
///
/// Mirrored here rather than shared with `mixengine_platform::PortRange`, for the reason every other
/// mirrored vocabulary in this crate carries: `mixengine-platform` depends on this crate, and not
/// the other way round.
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct ReservedRange {
    /// The first port, included.
    pub start: u16,

    /// The last port, included.
    pub end: u16,
}

/// `platform.json` — the facts the doctor's judgement was made from.
///
/// **Only what is free to read.** `daemon.doctor` has already probed the resolver and port access,
/// and probing them again for this file would let one archive hold two answers about one machine.
/// What is here is a compile-time constant, a per-OS constant, or the read of a table.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct PlatformFacts {
    /// `std::env::consts::OS`.
    ///
    /// The operating system's *version* is deliberately absent: reading it is a system call, which
    /// belongs behind a `mixengine-platform` capability that does not exist yet.
    pub os: String,

    /// `std::env::consts::ARCH`.
    pub arch: String,

    /// `std::env::consts::FAMILY`.
    pub family: String,

    /// What this daemon is, and what it speaks.
    pub daemon: DaemonVersion,

    /// What this system promises about a killed daemon's descendants — `total`, `immediate_child`
    /// or `none`. See `.claude/decisions/0007-supervised-child-owns-a-process-group.md`.
    pub orphan_guarantee: String,

    /// The same answer as the sentence a person reads.
    pub orphan_because: String,

    /// Whether a prompt can be raised here, and what is missing when it cannot.
    pub elevation: String,

    /// What this system has taken out of circulation, or [`None`] when it could not be read.
    pub reserved_ports: Option<Vec<ReservedRange>>,
}

/// `manifest.json` — what this bundle is.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct Manifest {
    /// [`MANIFEST_FORMAT`].
    pub format: u32,

    /// When it was taken.
    pub taken_at: Timestamp,

    /// The home it was taken from.
    pub home: String,

    /// The daemon that took it.
    pub daemon: DaemonVersion,

    /// Every part in this archive, this one included.
    pub parts: Vec<Part>,

    /// What was left out, and why.
    pub omitted: Vec<Omission>,

    /// Where the log excerpt begins.
    pub daemon_log: LogExcerpt,
}

/// `daemon.bundle`'s answer.
///
/// **The sizes are here and not in [`Manifest`].** The manifest is written last and cannot state its
/// own size from inside itself; rather than one member carrying a hole, the counts live where every
/// one of them is known — in the value the call returns, after the archive has been closed.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct BundleReport {
    /// Where it was written, absolute.
    pub path: String,

    /// The finished archive's size on disk.
    pub bytes: u64,

    /// When it was taken.
    pub taken_at: Timestamp,

    /// Each part that was written, and the bytes it contributed, in packing order.
    pub members: Vec<Member>,

    /// What was left out, and why.
    pub omitted: Vec<Omission>,
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The packing order carries every variant, and each has a name of its own.
    ///
    /// **This is where the compiler stops helping.** A variant added to [`Part`] fails to compile in
    /// [`Part::file_name`] and in the daemon's own match, which is what makes somebody look — but
    /// nothing forces them to add it to [`Part::ALL`] as well, and a part in the enum and not in the
    /// packing order is a part no archive ever contains. The length below is the literal they have
    /// to come back and change.
    #[test]
    fn every_part_is_packed_once_and_named_once() {
        assert_eq!(Part::ALL.len(), 5);

        let names: std::collections::BTreeSet<&str> =
            Part::ALL.iter().map(|part| part.file_name()).collect();
        assert_eq!(names.len(), Part::ALL.len(), "{names:?}");

        assert_eq!(
            Part::ALL.last(),
            Some(&Part::Manifest),
            "the manifest names the parts, so it is written after them"
        );
    }

    /// A part travels as the word a person would search an archive for.
    #[test]
    fn a_part_travels_as_its_own_word() {
        let encoded = serde_json::to_value(Part::DaemonLog).expect("a part serialises");
        assert_eq!(encoded, serde_json::json!("daemon_log"));
    }

    /// The ask accepts nothing it does not know, so a misspelled option is refused rather than
    /// silently ignored — which for an option that bounds what goes into an archive would be the
    /// caller believing they had narrowed it.
    #[test]
    fn an_unknown_option_is_refused_rather_than_ignored() {
        let asked: Result<DiagnosticsBundle, _> =
            serde_json::from_value(serde_json::json!({ "tail": 10 }));
        assert!(asked.is_err());

        let empty: DiagnosticsBundle =
            serde_json::from_value(serde_json::json!({})).expect("no options is the ordinary ask");
        assert_eq!(empty, DiagnosticsBundle::default());
    }
}
