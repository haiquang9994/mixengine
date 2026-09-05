//! `daemon.bundle` — one archive a person can attach to a bug report. Roadmap task **T93**.
//!
//! **Five members, and the list is closed.** What goes in is
//! [`Part`]'s five variants and never a walk of the home: `certs/` will hold
//! the internal CA's private key, `data/` holds the user's databases, and `run/` is what stands
//! between a local process and this daemon. A sweep written today omits all three because whoever
//! wrote it remembered them; a closed list removes the remembering.
//!
//! **What was left out is named, with its reason.** A bundle silent about where it did not look is
//! a bundle claiming it looked everywhere — see [`DECIDED_AGAINST`].
//!
//! **A part that could not be read is an omission and not the end of the call.** A bundle is wanted
//! exactly when things are failing, so an archive lost because one reading failed would be an
//! archive lost at the worst possible moment. That is not the same as a part that is *empty*: a
//! daemon which has logged nothing is an answer, and a read that failed is a failure — the
//! distinction `Keyring::secret` already draws between `Ok(None)` and `Err`.
//!
//! **Everything that touches the disk happens on one blocking thread.** Reading a megabyte of log,
//! deflating five members and unlinking old archives are all blocking calls, and
//! `.claude/standards/rust.md` puts anything that can hang off the runtime's threads.

use std::io::{Read as _, Seek as _, SeekFrom, Write as _};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::SystemTime;

use mixengine_platform::{ElevationSupport, Host, OrphanGuarantee};
use mixengine_proto::{
    BundleReport, DaemonStatus, DaemonVersion, DoctorReport, Error, LogExcerpt, MANIFEST_FORMAT,
    Manifest, Member, Omission, Part, PlatformFacts, ReservedRange, Timestamp,
};
use zip::write::SimpleFileOptions;

use crate::error::ToWire as _;

/// How much of `daemon.log` travels.
///
/// A megabyte, because this file is attached to a message somebody sends. The whole log is 10 MB
/// before it rotates (T4), and an archive that carried all of it would be one nobody could email —
/// which would make the bundle's own purpose fail on the machines that produced the most output.
const LOG_TAIL_BYTES: u64 = 1 << 20;

/// How many archives are kept.
///
/// More than one because a support conversation compares two runs; three rather than every one ever
/// taken because `cache/` is not somewhere a user goes looking for things to delete.
const BUNDLES_KEPT: usize = 3;

/// Where they are written, inside [`Paths::cache`](mixengine_core::Paths::cache).
const DIRECTORY: &str = "diagnostics";

/// What this build refuses to carry, and why — the T93 design, D4.
///
/// **A table rather than three pushes**, so that adding a member and removing its reason are one
/// edit, and so the reasons read together the way the person receiving a bundle reads them.
const DECIDED_AGAINST: &[(&str, &str)] = &[
    (
        "etc/",
        "the rendered configuration is the one surface a person edits by hand, which is the one \
         surface the secret-free guarantee does not cover",
    ),
    (
        "data/, certs/, run/",
        "the private directories: the user's databases, the internal CA's private key, and what \
         stands between a local process and this daemon",
    ),
    (
        "services, sites and projects",
        "not among the four things this bundle was asked for; secret-free by contract, so this is \
         the cheapest member to add when somebody wants it",
    ),
];

/// The `daemon.bundle` half of the API.
#[derive(Debug)]
pub(crate) struct Bundles {
    /// This machine, for the facts `platform.json` can read for free.
    host: Arc<dyn Host>,

    /// The home: where the log is read from, and where the archive is written.
    paths: mixengine_core::Paths,

    /// This home's crash reports — roadmap task **T91**.
    crashes: crate::crash::Reports,
}

impl Bundles {
    /// The one of these the API holds.
    pub(crate) fn new(
        host: Arc<dyn Host>,
        paths: &mixengine_core::Paths,
        crashes: crate::crash::Reports,
    ) -> Arc<Self> {
        Arc::new(Self {
            host,
            paths: paths.clone(),
            crashes,
        })
    }

    /// Take one bundle.
    ///
    /// **`status` arrives as a [`Result`] on purpose.** `Api::status` is fallible — the elevation
    /// queue can refuse a read — and a bundle that failed outright because of it would be an archive
    /// lost at exactly the moment somebody needed one. So its error becomes an [`Omission`] and the
    /// call succeeds with four members.
    ///
    /// **Both readings are passed in rather than taken here.** Two constructions of one document are
    /// two things to keep in step, and the copy inside an archive is the one nobody would notice had
    /// drifted from what `daemon.status` answers.
    ///
    /// # Errors
    ///
    /// The wire form of [`mixengine_core::Error::Io`] when the archive's directory cannot be created
    /// or the archive itself cannot be written. Nothing else fails the call.
    ///
    /// **Converted here rather than at the handler**, unlike most of this daemon: `mixengine_core`'s
    /// error is large enough that returning it through two frames is a `clippy::result_large_err` on
    /// every one of them, and the path and the action — which are the whole value of that type here —
    /// survive the conversion intact.
    pub(crate) async fn take(
        &self,
        doctor: &DoctorReport,
        status: Result<DaemonStatus, Error>,
        version: DaemonVersion,
    ) -> Result<BundleReport, Error> {
        let taken_at = Timestamp::from_system_time(SystemTime::now());

        let mut parts: Vec<(Part, Vec<u8>)> = Vec::new();
        let mut omitted: Vec<Omission> = Vec::new();

        encoded(&mut parts, &mut omitted, Part::Doctor, doctor);

        match status {
            Ok(status) => encoded(&mut parts, &mut omitted, Part::Status, &status),
            Err(error) => omitted.push(Omission {
                name: Part::Status.file_name().to_owned(),
                because: format!(
                    "this daemon could not report its own status: {}",
                    error.message
                ),
            }),
        }

        encoded(
            &mut parts,
            &mut omitted,
            Part::Platform,
            &platform(self.host.as_ref(), version.clone()),
        );

        omitted.extend(DECIDED_AGAINST.iter().map(|(name, because)| Omission {
            name: (*name).to_owned(),
            because: (*because).to_owned(),
        }));

        let directory = self.paths.cache().join(DIRECTORY);
        let log = self.paths.daemon_log_file().to_path_buf();
        let home = self.paths.root().display().to_string();

        let written = Written {
            directory,
            log,
            taken_at,
            home,
            version,
            parts,
            omitted,
            crashes: self.crashes.clone(),
        };

        tokio::task::spawn_blocking(move || written.pack())
            .await
            .map_err(|error| io("write", Path::new(DIRECTORY), std::io::Error::other(error)))?
    }
}

/// Everything the blocking half needs, and nothing it would have to reach for.
///
/// One struct rather than seven arguments into a closure: the closure runs on another thread and
/// owns what it was given, so what it needs has to be decided *here*, where a reviewer can see the
/// whole of it. `paths` is deliberately absent — a `Paths` in this struct would be a way to read a
/// directory nobody listed.
#[derive(Debug)]
struct Written {
    /// `<root>/cache/diagnostics/`.
    directory: PathBuf,

    /// The daemon's own log file.
    log: PathBuf,

    /// The moment this bundle names itself after.
    taken_at: Timestamp,

    /// The home it describes.
    home: String,

    /// The daemon that took it.
    version: DaemonVersion,

    /// The members already serialised, in no particular order — [`Part::ALL`] decides the packing.
    parts: Vec<(Part, Vec<u8>)>,

    /// What has been left out so far. The log's own accounting joins it inside [`Written::pack`].
    omitted: Vec<Omission>,

    /// This home's crash reports, read inside [`Written::pack`] — roadmap task **T91**.
    ///
    /// **Here rather than read in [`Bundles::take`]** for the reason this struct exists: reading
    /// twenty small files is blocking work, and it belongs on the thread the log tail and the
    /// deflate are already on. It is not the `paths` this struct deliberately refuses either — it
    /// names one directory and can reach no other.
    crashes: crate::crash::Reports,
}

impl Written {
    /// Read the log, write the archive, and tidy up behind it.
    ///
    /// # Errors
    ///
    /// An I/O failure naming the directory or the file it could not write — see
    /// [`Bundles::take`].
    fn pack(mut self) -> Result<BundleReport, Error> {
        let (log, excerpt) = tail(&self.log, LOG_TAIL_BYTES);
        self.parts.push((Part::DaemonLog, log));

        // **Roadmap task T91.** An empty array is an answer and not a hole: a home that has never
        // crashed says so, exactly as a check that found nothing wrong is the evidence that it ran.
        let (reports, unreadable) = self.crashes.load();
        encoded(&mut self.parts, &mut self.omitted, Part::Crashes, &reports);

        for name in unreadable {
            self.omitted.push(Omission {
                name,
                because: "this crash report could not be read".to_owned(),
            });
        }

        // Said rather than left to be inferred from an empty array, which would read as "nothing
        // has ever gone wrong here" on the one home where that is not what it means.
        if !self.crashes.enabled() {
            self.omitted.push(Omission {
                name: "logs/crashes/".to_owned(),
                because: "crash reports are switched off in config.toml, so this home records none"
                    .to_owned(),
            });
        }

        let manifest = Manifest {
            format: MANIFEST_FORMAT,
            taken_at: self.taken_at,
            home: self.home,
            daemon: self.version,
            parts: Part::ALL.to_vec(),
            omitted: self.omitted.clone(),
            daemon_log: excerpt,
        };
        encoded(
            &mut self.parts,
            &mut self.omitted,
            Part::Manifest,
            &manifest,
        );

        mixengine_core::paths::create_dir(&self.directory).map_err(|error| error.to_wire())?;
        let path = self.directory.join(archive_name(self.taken_at));
        let file = std::fs::File::create(&path).map_err(|source| io("create", &path, source))?;

        let mut writer = zip::ZipWriter::new(file);
        let options =
            SimpleFileOptions::default().compression_method(zip::CompressionMethod::Deflated);
        let mut members = Vec::new();

        for part in Part::ALL {
            let Some((_, bytes)) = self.parts.iter().find(|(which, _)| *which == part) else {
                continue;
            };

            let wrote = writer
                .start_file(part.file_name(), options)
                .and_then(|()| writer.write_all(bytes).map_err(Into::into));

            if let Err(source) = wrote {
                return Err(io("write", &path, std::io::Error::other(source)));
            }

            members.push(Member {
                part,
                bytes: bytes.len() as u64,
            });
        }

        writer
            .finish()
            .map_err(|source| io("write", &path, std::io::Error::other(source)))?;

        let bytes = std::fs::metadata(&path).map(|file| file.len()).unwrap_or(0);

        prune(&self.directory, BUNDLES_KEPT);

        Ok(BundleReport {
            path: path.display().to_string(),
            bytes,
            taken_at: self.taken_at,
            members,
            omitted: self.omitted,
        })
    }
}

/// A failure to write the archive, in the shape a client reads.
///
/// [`mixengine_core::Error::Io`] and then straight through `to_wire`, so that what reaches the
/// screen is this workspace's one sentence for a path that would not open — including the path
/// itself, which on a `[paths]` override pointing at an unmounted disk *is* the answer.
fn io(action: &'static str, path: &Path, source: std::io::Error) -> Error {
    mixengine_core::Error::Io {
        action,
        path: path.to_path_buf(),
        source,
    }
    .to_wire()
}

/// Serialise one member, or record why it is not there.
///
/// Serialising a type this workspace defined can only fail on something no `mixengine-proto` type
/// has, so this is a `Result` for the same reason the RPC layer's encoder is: nothing here decides
/// that a failure is impossible and then panics when it is not.
fn encoded<T: serde::Serialize>(
    parts: &mut Vec<(Part, Vec<u8>)>,
    omitted: &mut Vec<Omission>,
    part: Part,
    value: &T,
) {
    match serde_json::to_vec_pretty(value) {
        Ok(bytes) => parts.push((part, bytes)),
        Err(error) => omitted.push(Omission {
            name: part.file_name().to_owned(),
            because: format!("this daemon could not serialise it: {error}"),
        }),
    }
}

/// `platform.json` — the facts the doctor's judgement was made from.
///
/// **Nothing here probes.** `daemon.doctor` has already asked the resolver and port access, and a
/// second probe for this file would let one archive hold two answers about one machine. What is
/// read here is a compile-time constant, a per-OS constant, or a table.
fn platform(host: &dyn Host, daemon: DaemonVersion) -> PlatformFacts {
    let guarantee = mixengine_platform::orphan_guarantee();

    PlatformFacts {
        os: std::env::consts::OS.to_owned(),
        arch: std::env::consts::ARCH.to_owned(),
        family: std::env::consts::FAMILY.to_owned(),
        daemon,
        orphan_guarantee: match guarantee {
            OrphanGuarantee::Total => "total",
            OrphanGuarantee::ImmediateChild => "immediate_child",
            OrphanGuarantee::None => "none",
        }
        .to_owned(),
        orphan_because: guarantee.because().to_owned(),
        elevation: match host.elevation().probe() {
            ElevationSupport::Available => "available".to_owned(),
            ElevationSupport::Unavailable { reason } => format!("unavailable: {reason}"),
        },
        reserved_ports: host.reserved_ports().reserved().ok().map(|ranges| {
            ranges
                .into_iter()
                .map(|range| ReservedRange {
                    start: range.start,
                    end: range.end,
                })
                .collect()
        }),
    }
}

/// The archive's name: `diagnostics-20260824T141530.472Z.zip`.
///
/// Derived from [`Timestamp::to_rfc3339`] rather than formatted a second way — one spelling of a
/// moment in this workspace — with the separators removed so that it is a file name on Windows too,
/// and the millisecond appended because two bundles taken inside one second must not be one file.
fn archive_name(taken_at: Timestamp) -> String {
    let stamp = taken_at.to_rfc3339();
    let compact: String = stamp.chars().filter(|c| *c != '-' && *c != ':').collect();
    let millis = taken_at.0.rem_euclid(1_000);

    format!(
        "diagnostics-{}.{millis:03}Z.zip",
        compact.trim_end_matches('Z')
    )
}

/// The last `limit` bytes of `path`, beginning at a line boundary.
///
/// **A cut at a byte offset lands mid-line**, and a fragment that reads like a malformed record is
/// worse than one line fewer — so the partial first line is dropped, and counted as skipped rather
/// than quietly lost.
///
/// **An absent file is an answer and not a failure.** A daemon that has logged nothing yields an
/// empty member and an accounting of zeroes, which is a fact; an omission there would say something
/// was withheld.
fn tail(path: &Path, limit: u64) -> (Vec<u8>, LogExcerpt) {
    let nothing = (Vec::new(), LogExcerpt::default());

    let Ok(mut file) = std::fs::File::open(path) else {
        return nothing;
    };
    let Ok(length) = file.metadata().map(|file| file.len()) else {
        return nothing;
    };

    let from = length.saturating_sub(limit);
    if from > 0 && file.seek(SeekFrom::Start(from)).is_err() {
        return nothing;
    }

    let mut bytes = Vec::new();
    if file.read_to_end(&mut bytes).is_err() {
        return nothing;
    }

    let mut skipped = from;
    if from > 0 {
        match bytes.iter().position(|byte| *byte == b'\n') {
            Some(newline) => {
                bytes.drain(..=newline);
                skipped += newline as u64 + 1;
            }
            // A megabyte with no newline in it is one line, and half of it is not a line at all.
            None => {
                skipped += bytes.len() as u64;
                bytes.clear();
            }
        }
    }

    let excerpt = LogExcerpt {
        included_bytes: bytes.len() as u64,
        skipped_bytes: skipped,
        rotated_files: rotated(path),
    };

    (bytes, excerpt)
}

/// How many rotated files sit beside `path` and are not in the archive.
fn rotated(path: &Path) -> u32 {
    let (Some(directory), Some(name)) = (path.parent(), path.file_name()) else {
        return 0;
    };

    let Ok(entries) = std::fs::read_dir(directory) else {
        return 0;
    };

    entries
        .filter_map(Result::ok)
        .filter(|entry| {
            let beside = entry.file_name();
            beside != name
                && beside
                    .to_string_lossy()
                    .starts_with(name.to_string_lossy().as_ref())
        })
        .count()
        .try_into()
        .unwrap_or(u32::MAX)
}

/// Keep the newest `keep` archives and remove the rest.
///
/// **Fails nothing.** The bundle the caller asked for exists by the time this runs, and refusing to
/// hand it over because an old file would not delete is the wrong trade — so a failure is a line in
/// the log and not an error in the answer.
fn prune(directory: &Path, keep: usize) {
    let Ok(entries) = std::fs::read_dir(directory) else {
        return;
    };

    let mut archives: Vec<PathBuf> = entries
        .filter_map(Result::ok)
        .map(|entry| entry.path())
        .filter(|path| {
            path.file_name()
                .map(|name| name.to_string_lossy())
                .is_some_and(|name| name.starts_with("diagnostics-") && name.ends_with(".zip"))
        })
        .collect();

    // The name is the moment, zero-padded, so lexical order is chronological order.
    archives.sort_unstable();

    for stale in archives.iter().rev().skip(keep) {
        if let Err(error) = std::fs::remove_file(stale) {
            tracing::warn!(path = %stale.display(), %error, "an old diagnostics bundle would not delete");
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A cut into the middle of a file begins at the next line, and every byte is accounted for.
    #[test]
    fn an_excerpt_begins_at_a_line_boundary_and_counts_what_it_skipped() {
        let directory = tempfile::tempdir().expect("a temporary directory");
        let log = directory.path().join("daemon.log");
        // Twenty lines of ten bytes each. A limit of 95 lands inside one of them.
        let written: String = (0..20).map(|n| format!("line-{n:03}\n")).collect();
        std::fs::write(&log, &written).expect("a log");

        let (bytes, excerpt) = tail(&log, 95);

        assert!(
            bytes.starts_with(b"line-"),
            "{}",
            String::from_utf8_lossy(&bytes)
        );
        assert_eq!(
            excerpt.included_bytes + excerpt.skipped_bytes,
            written.len() as u64,
            "every byte is either included or counted as skipped"
        );
        assert_eq!(excerpt.included_bytes, bytes.len() as u64);
        assert!(excerpt.skipped_bytes > 0, "a 95-byte window into 200 skips");
    }

    /// A file smaller than the bound is carried whole, and nothing is claimed to be missing.
    ///
    /// The control for the test above: without it, an excerpt that dropped everything would satisfy
    /// "begins at a line boundary" too.
    #[test]
    fn a_log_shorter_than_the_bound_is_carried_whole() {
        let directory = tempfile::tempdir().expect("a temporary directory");
        let log = directory.path().join("daemon.log");
        std::fs::write(&log, b"one line\n").expect("a log");

        let (bytes, excerpt) = tail(&log, LOG_TAIL_BYTES);

        assert_eq!(bytes, b"one line\n");
        assert_eq!(excerpt.skipped_bytes, 0);
        assert_eq!(excerpt.included_bytes, 9);
    }

    /// A daemon that has logged nothing yields an empty member rather than an omission — an absent
    /// log is an answer, and a read that failed would be a failure.
    #[test]
    fn a_home_with_no_log_yet_yields_an_empty_member() {
        let directory = tempfile::tempdir().expect("a temporary directory");

        let (bytes, excerpt) = tail(&directory.path().join("daemon.log"), LOG_TAIL_BYTES);

        assert!(bytes.is_empty());
        assert_eq!(excerpt, LogExcerpt::default());
    }

    /// Rotated files are counted and not carried.
    #[test]
    fn the_files_beside_the_log_are_counted_and_not_carried() {
        let directory = tempfile::tempdir().expect("a temporary directory");
        let log = directory.path().join("daemon.log");
        std::fs::write(&log, b"now\n").expect("a log");
        std::fs::write(directory.path().join("daemon.log.1"), b"before\n").expect("a rotation");
        std::fs::write(directory.path().join("daemon.log.2"), b"earlier\n").expect("a rotation");

        let (bytes, excerpt) = tail(&log, LOG_TAIL_BYTES);

        assert_eq!(bytes, b"now\n", "only the current file travels");
        assert_eq!(excerpt.rotated_files, 2);
    }

    /// Four bundles leave three, and they are the three newest.
    #[test]
    fn pruning_keeps_the_three_newest() {
        let directory = tempfile::tempdir().expect("a temporary directory");
        let names = [
            "diagnostics-20260824T141530.100Z.zip",
            "diagnostics-20260824T141530.200Z.zip",
            "diagnostics-20260824T141531.000Z.zip",
            "diagnostics-20260824T141532.000Z.zip",
        ];
        for name in names {
            std::fs::write(directory.path().join(name), b"x").expect("a bundle");
        }

        prune(directory.path(), 3);

        let left: std::collections::BTreeSet<String> = std::fs::read_dir(directory.path())
            .expect("the directory reads")
            .filter_map(Result::ok)
            .map(|entry| entry.file_name().to_string_lossy().into_owned())
            .collect();

        assert_eq!(left.len(), 3, "{left:?}");
        assert!(
            !left.contains(names[0]),
            "the oldest is the one that went: {left:?}"
        );
    }

    /// Nothing else in the directory is touched, which is the half a count cannot show.
    #[test]
    fn pruning_removes_only_what_it_wrote() {
        let directory = tempfile::tempdir().expect("a temporary directory");
        std::fs::write(directory.path().join("notes.txt"), b"mine").expect("somebody else's file");

        prune(directory.path(), 0);

        assert!(directory.path().join("notes.txt").exists());
    }

    /// The name sorts chronologically and is a file name on every OS this build runs on.
    #[test]
    fn the_name_is_sortable_and_is_a_file_name() {
        let earlier = archive_name(Timestamp(1_756_042_530_100));
        let later = archive_name(Timestamp(1_756_042_530_200));

        assert!(earlier < later, "{earlier} < {later}");
        assert!(
            !earlier.contains(':'),
            "{earlier} has to be a file name on Windows too"
        );
        assert!(
            earlier.starts_with("diagnostics-") && earlier.ends_with(".zip"),
            "{earlier}"
        );
    }
}
