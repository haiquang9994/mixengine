//! What this daemon leaves behind when it hits a bug in itself. Roadmap task **T91**.
//!
//! **Three documents described this before it existed.** `.claude/standards/rust.md` says the RPC
//! layer turns a panic into `internal`; [`api::rpc`](crate::api::rpc) says the message *"has already
//! gone to the log through the panic hook"*; `Cargo.toml`'s release profile keeps symbol names
//! because *"a daemon crash report is worthless without function names"*. There was no hook, and
//! `spawn_detached` gives the real daemon `Stdio::null()` for its stderr — so until this module the
//! message went nowhere at all.
//!
//! **The report carries no message and the log does.** Everything in a
//! [`CrashReport`] is a constant of this build, a literal from `std` or `tokio`, or a symbol name,
//! which is what makes the file attachable to a public bug report without being read first. The
//! message is `format!`-ed from whatever was in scope and can carry a path, so it goes to
//! `daemon.log` — where paths a person chose already are, and always have been.
//!
//! **Nothing here sends anything anywhere.** See
//! `.claude/decisions/0022-a-crash-report-is-recorded-by-default-and-sent-by-nothing.md`.

use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::SystemTime;

use mixengine_proto::{
    CRASH_FORMAT, Check, CrashLocation, CrashReport, DaemonVersion, Outcome, Timestamp,
};

/// How many reports are kept.
///
/// A constant rather than a key, for the reason `BUNDLES_KEPT` is one in
/// [`diagnostics`](crate::diagnostics): it is here to bound a disk, not to be tuned. Twenty of a few
/// kilobytes each is what a crash loop through a restart costs, and it is more distinct failures
/// than anybody reads at once.
const KEPT: usize = 20;

/// The most frames one report carries.
const MAX_FRAMES: usize = 64;

/// The most characters one frame — or one thread name — carries.
const MAX_FRAME_CHARS: usize = 512;

/// Tells two panics raised in the same millisecond of this process apart.
///
/// **Not decoration.** Two threads panicking inside one millisecond is exactly what a crash loop
/// looks like, and a name without this would have the second report overwrite the first.
static SEQUENCE: AtomicU64 = AtomicU64::new(0);

thread_local! {
    /// Whether this thread is already inside the hook.
    ///
    /// A panic raised *by* the hook would otherwise re-enter it for ever. This is the whole of the
    /// guard: a second entry delegates to the previous hook and returns.
    static INSIDE: std::cell::Cell<bool> = const { std::cell::Cell::new(false) };
}

/// This home's crash reports: where they go, whether they are written, and what wrote them.
///
/// Cheap to clone, and cloned rather than shared behind an `Arc`, because one of them is moved into
/// a panic hook that outlives every other holder and can be reached from any thread.
#[derive(Debug, Clone)]
pub(crate) struct Reports {
    /// `<root>/logs/crashes/`, wherever `[paths] logs` put it.
    directory: PathBuf,

    /// `[crash] enabled`.
    enabled: bool,

    /// What every report this process writes says wrote it.
    version: DaemonVersion,
}

/// What is on disk, read from the file names alone.
///
/// **From the names**, because that is all `mix doctor` needs, and opening twenty files to answer
/// one line would be twenty reads for two numbers.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct Recorded {
    /// How many reports there are.
    pub(crate) count: usize,

    /// When the newest was written, or [`None`] when no name could be read as a moment.
    pub(crate) newest: Option<Timestamp>,
}

impl Reports {
    /// This home's reports.
    ///
    /// **A pure function of its two arguments**, which is what lets `main` build one to install the
    /// hook from and `serve` build the same one for the API without the two being kept in step by
    /// hand: there is no third input and no identity to share. The version is not an argument for
    /// the same reason — it is two compile-time constants of this build, exactly as `Api::version`
    /// reads them.
    pub(crate) fn new(paths: &mixengine_core::Paths, enabled: bool) -> Self {
        Self {
            directory: paths.crashes(),
            enabled,
            version: DaemonVersion {
                version: env!("CARGO_PKG_VERSION").to_owned(),
                protocol: mixengine_proto::PROTOCOL_VERSION,
            },
        }
    }

    /// Whether a report is written at all.
    pub(crate) fn enabled(&self) -> bool {
        self.enabled
    }

    /// Where they go.
    pub(crate) fn directory(&self) -> &Path {
        &self.directory
    }

    /// Install the panic hook for the rest of this process's life.
    ///
    /// **The order of the three steps is about a deadlock**, and it is the one real hazard here. A
    /// panic hook runs *before* unwinding, on the panicking thread, while every lock that thread
    /// holds is still held — so a panic raised inside the logging sink would have step 3 lock a
    /// mutex this thread already owns, and hang the daemon. It is unlikely (`RotatingFile` returns
    /// its errors rather than panicking) and it is not preventable from here, so the answer is to
    /// put the write that matters first and to name the hazard rather than pretend it away.
    ///
    /// **Step 3 runs inside the span the request opened**, which is what puts `method` and
    /// `request_id` on the line for free — and is why the report itself does not carry them.
    pub(crate) fn install(&self) {
        let reports = self.clone();
        let previous = std::panic::take_hook();

        std::panic::set_hook(Box::new(move |info| {
            if INSIDE.with(|inside| inside.replace(true)) {
                previous(info);
                return;
            }
            let _leaving = Leaving;

            let current = std::thread::current();
            let thread = current.name();
            let at = info.location();

            // 1. The evidence, first.
            let wrote = reports.enabled.then(|| {
                let backtrace = std::backtrace::Backtrace::force_capture().to_string();

                reports.record(
                    Timestamp::from_system_time(SystemTime::now()),
                    thread,
                    at.map(|at| (at.file(), at.line(), at.column())),
                    &backtrace,
                )
            });

            // 2. Whoever is watching a terminal, which is nobody once `--detach` has run.
            previous(info);

            // 3. `daemon.log`. Last, and carrying step 1's failure rather than swallowing it —
            // there was nowhere to say it until now.
            tracing::error!(
                thread = thread.unwrap_or("unnamed"),
                location = at.map_or_else(|| "unknown".to_owned(), ToString::to_string),
                report = match &wrote {
                    None => "not written: crash reports are off in config.toml".to_owned(),
                    Some(Ok(path)) => path.display().to_string(),
                    Some(Err(error)) => format!("not written: {error}"),
                },
                "the daemon panicked: {}",
                info.payload_as_str()
                    .unwrap_or("a payload that is not a string")
            );
        }));
    }

    /// Write one report, and prune what that made too many.
    ///
    /// Separated from the hook so that everything the hook does with a disk is reachable from a
    /// test: what is left in the closure is three accessors on a `PanicHookInfo`, which no test can
    /// construct.
    ///
    /// **Written to `.tmp` and renamed.** A report the process was killed halfway through writing is
    /// a file that will not parse, and the bundle would then have to decide what to do about one. A
    /// rename is one syscall and removes the question. A `.tmp` left behind by a kill *between* the
    /// two is ignored by everything that reads this directory and is deliberately not pruned:
    /// deleting one could take a concurrent hook's half-written file with it.
    ///
    /// # Errors
    ///
    /// [`std::io::Error`] when the report will not serialise, or the directory, the file or the
    /// rename will not go through.
    fn record(
        &self,
        at: Timestamp,
        thread: Option<&str>,
        location: Option<(&str, u32, u32)>,
        backtrace: &str,
    ) -> std::io::Result<PathBuf> {
        let report = CrashReport {
            format: CRASH_FORMAT,
            recorded_at: at,
            daemon: self.version.clone(),
            os: std::env::consts::OS.to_owned(),
            arch: std::env::consts::ARCH.to_owned(),
            thread: thread.map(|name| name.chars().take(MAX_FRAME_CHARS).collect()),
            location: location.map(|(file, line, column)| CrashLocation {
                file: file.to_owned(),
                line,
                column,
            }),
            frames: frames(backtrace),
        };

        let bytes = serde_json::to_vec_pretty(&report).map_err(std::io::Error::other)?;

        std::fs::create_dir_all(&self.directory)?;

        let name = file_name(
            at,
            std::process::id(),
            SEQUENCE.fetch_add(1, Ordering::Relaxed),
        );
        let written = self.directory.join(&name);
        let staging = self.directory.join(format!("{name}.tmp"));

        std::fs::write(&staging, &bytes)?;
        std::fs::rename(&staging, &written)?;

        prune(&self.directory, KEPT);

        Ok(written)
    }

    /// How many reports there are, and when the newest was written.
    pub(crate) fn recorded(&self) -> Recorded {
        let found = files(&self.directory);

        Recorded {
            count: found.len(),
            newest: found.last().and_then(|path| millis(path)).map(Timestamp),
        }
    }
}

/// What `mix doctor` says about this home's crash reports — roadmap task **T91**.
///
/// **A [`Note`](Outcome::Note) and never a [`Problem`](Outcome::Problem), and that is not a
/// stylistic choice.** `mix doctor` exits non-zero on a problem, so one crash recorded once would
/// fail every `mix doctor` in every script from then on. It is also not a fault of the *machine*,
/// which is what a [`ProblemId`](mixengine_proto::ProblemId) and the repair keyed off it are for —
/// so no condition is added and `daemon.doctor_repair` gains nothing to decline.
///
/// **Nothing deletes them.** [`KEPT`] bounds them, and a repair that threw away evidence nobody had
/// read yet would be the wrong kind of tidy.
pub(crate) fn check(reports: &Reports) -> Check {
    let outcome = if reports.enabled() {
        let recorded = reports.recorded();

        if recorded.count == 0 {
            Outcome::Ok {}
        } else {
            Outcome::Note {
                because: format!(
                    "this home has {} crash {} in {}, the newest from {}. Nothing sends them \
                     anywhere: they carry none of your paths and no passwords, and `mix doctor \
                     --bundle` is what packs them into an archive you can attach to a bug report",
                    recorded.count,
                    if recorded.count == 1 {
                        "report"
                    } else {
                        "reports"
                    },
                    reports.directory().display(),
                    recorded
                        .newest
                        .map_or_else(|| "an unreadable name".to_owned(), Timestamp::to_rfc3339),
                ),
            }
        }
    } else {
        Outcome::Skipped {
            because: "crash reports are switched off in config.toml, so this home records none"
                .to_owned(),
        }
    };

    Check {
        name: "crash reports".to_owned(),
        outcome,
    }
}

/// Clears the re-entrancy flag however the hook ends.
#[derive(Debug)]
struct Leaving;

impl Drop for Leaving {
    fn drop(&mut self) {
        INSIDE.with(|inside| inside.set(false));
    }
}

/// The symbol names in a rendered backtrace, and nothing else.
///
/// `std::backtrace::Backtrace` offers no structured access on stable, so this reads its `Display`:
/// a frame header is `<n>: <symbol>`, and every `at <path>:<line>:<col>` continuation is dropped
/// with everything else that is not one.
///
/// **The parse is best-effort and the guarantee does not rest on it.** A frame that still contains a
/// path separator is dropped, which a Rust symbol never is — so a change to that format costs
/// frames rather than the promise this module exists to keep.
fn frames(rendered: &str) -> Vec<String> {
    rendered
        .lines()
        .filter_map(|line| {
            let (number, symbol) = line.trim_start().split_once(": ")?;
            if number.is_empty() || !number.bytes().all(|byte| byte.is_ascii_digit()) {
                return None;
            }

            let symbol = symbol.trim();
            if symbol.is_empty() || symbol.contains('/') || symbol.contains('\\') {
                return None;
            }

            Some(symbol.chars().take(MAX_FRAME_CHARS).collect())
        })
        .take(MAX_FRAMES)
        .collect()
}

/// What one report is called.
///
/// Milliseconds rather than the ISO form [`Timestamp`] can already print, because that form carries
/// `:` and Windows will not have one in a file name. Zero-padded so that the names sort by the
/// moment they name, which is what lets [`prune`] and [`Reports::recorded`] read a directory listing
/// rather than twenty files.
fn file_name(at: Timestamp, pid: u32, sequence: u64) -> String {
    format!("crash-{:013}-{pid}-{sequence}.json", at.0.max(0))
}

/// The moment a name carries, or [`None`] when it is not one of ours.
fn millis(path: &Path) -> Option<i64> {
    path.file_name()?
        .to_str()?
        .strip_prefix("crash-")?
        .split('-')
        .next()?
        .parse()
        .ok()
}

/// Every report in a directory, oldest first, and nothing that is not one.
///
/// `.json` only: a `.tmp` is a write that was interrupted, and is not a report.
fn files(directory: &Path) -> Vec<PathBuf> {
    let Ok(entries) = std::fs::read_dir(directory) else {
        return Vec::new();
    };

    let mut found: Vec<PathBuf> = entries
        .flatten()
        .map(|entry| entry.path())
        .filter(|path| {
            path.extension()
                .is_some_and(|extension| extension == "json")
                && path
                    .file_name()
                    .and_then(|name| name.to_str())
                    .is_some_and(|name| name.starts_with("crash-"))
        })
        .collect();

    found.sort();
    found
}

/// Leave the newest `keep` and remove the rest.
///
/// Failures are ignored: this runs inside a panic hook, where there is nothing left to report a
/// failed unlink to, and where a disk that will not delete is not the accident being recorded.
fn prune(directory: &Path, keep: usize) {
    for stale in files(directory).iter().rev().skip(keep) {
        let _ = std::fs::remove_file(stale);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn reports(directory: &Path) -> Reports {
        Reports {
            directory: directory.to_path_buf(),
            enabled: true,
            version: DaemonVersion {
                version: "0.1.0".to_owned(),
                protocol: mixengine_proto::PROTOCOL_VERSION,
            },
        }
    }

    /// The `at …` continuation lines are where a build machine's directories live, and dropping them
    /// is the whole reason this function exists rather than `Backtrace::to_string()` going into the
    /// file as it is.
    #[test]
    fn a_continuation_line_carrying_a_path_is_dropped() {
        let rendered = "\
   0: mixengine_daemon::crash::tests::sample
             at C:\\Users\\someone\\project\\crates\\mixengine-daemon\\src\\crash.rs:12:5
   1: core::ops::function::FnOnce::call_once
             at /rustc/abcdef/library/core/src/ops/function.rs:250:5
";

        assert_eq!(
            frames(rendered),
            [
                "mixengine_daemon::crash::tests::sample",
                "core::ops::function::FnOnce::call_once",
            ]
        );
    }

    /// The guard, and not the parse, is what makes "no paths" true: a Rust symbol never contains a
    /// separator, so dropping any frame that does costs nothing today and keeps the guarantee if
    /// `std`'s format changes under us.
    #[test]
    fn a_frame_that_still_looks_like_a_path_is_dropped() {
        let rendered = "   0: /usr/lib/libc.so.6: something\n   1: mixengine_daemon::main\n";

        assert_eq!(frames(rendered), ["mixengine_daemon::main"]);
    }

    /// A real capture survives the filter. The claim is about `std`'s behaviour, so
    /// `.claude/standards/rust.md` asks for a test rather than a sentence.
    #[test]
    fn a_real_backtrace_survives_the_filter_with_no_separator_in_it() {
        let rendered = std::backtrace::Backtrace::force_capture().to_string();
        let found = frames(&rendered);

        assert!(!found.is_empty(), "{rendered}");
        assert!(found.len() <= MAX_FRAMES);
        for frame in &found {
            assert!(!frame.contains('/'), "{frame}");
            assert!(!frame.contains('\\'), "{frame}");
            assert!(frame.chars().count() <= MAX_FRAME_CHARS, "{frame}");
        }
    }

    /// One panic, one file — and its name sorts by when it happened.
    #[test]
    fn a_record_is_one_file_named_after_the_moment() {
        let home = tempfile::TempDir::new().expect("a temporary directory");
        let reports = reports(home.path());

        let path = reports
            .record(
                Timestamp(1_757_000_000_000),
                Some("tokio-runtime-worker"),
                Some(("crates/mixengine-daemon/src/services/mod.rs", 412, 9)),
                "   0: mixengine_daemon::services::start\n",
            )
            .expect("a report is written");

        let name = path
            .file_name()
            .expect("a name")
            .to_string_lossy()
            .into_owned();
        assert!(name.starts_with("crash-1757000000000-"), "{name}");
        assert!(name.ends_with(".json"), "{name}");

        let written: CrashReport =
            serde_json::from_slice(&std::fs::read(&path).expect("readable")).expect("a report");
        assert_eq!(written.format, CRASH_FORMAT);
        assert_eq!(written.thread.as_deref(), Some("tokio-runtime-worker"));
        assert_eq!(written.location.expect("a location").line, 412);
        assert_eq!(written.frames, ["mixengine_daemon::services::start"]);
    }

    /// Two panics in the same millisecond of one process is what a crash loop looks like, and
    /// without the counter in the name the second report would overwrite the first.
    #[test]
    fn two_records_in_the_same_millisecond_are_two_files() {
        let home = tempfile::TempDir::new().expect("a temporary directory");
        let reports = reports(home.path());
        let at = Timestamp(1_757_000_000_000);

        let first = reports.record(at, None, None, "").expect("written");
        let second = reports.record(at, None, None, "").expect("written");

        assert_ne!(first, second);
        assert_eq!(files(home.path()).len(), 2);
    }

    /// A crash loop must not fill a disk.
    #[test]
    fn only_the_newest_are_kept() {
        let home = tempfile::TempDir::new().expect("a temporary directory");
        let reports = reports(home.path());
        let last = KEPT as i64 + 4;

        for millis in 0..=last {
            reports
                .record(Timestamp(1_757_000_000_000 + millis), None, None, "")
                .expect("written");
        }

        assert_eq!(files(home.path()).len(), KEPT);

        assert_eq!(
            reports.recorded(),
            Recorded {
                count: KEPT,
                newest: Some(Timestamp(1_757_000_000_000 + last)),
            }
        );
    }

    /// The one guarantee this whole task is for: whatever the panic said, none of it is in the file.
    #[test]
    fn nothing_a_panic_said_reaches_the_file() {
        let home = tempfile::TempDir::new().expect("a temporary directory");
        let reports = reports(home.path());

        let path = reports
            .record(
                Timestamp(1_757_000_000_000),
                Some("tokio-runtime-worker"),
                Some(("crates/mixengine-daemon/src/sites.rs", 8, 1)),
                "   0: mixengine_daemon::sites::create\n",
            )
            .expect("written");

        let text = std::fs::read_to_string(&path).expect("readable");

        assert!(!text.contains("C:\\Users"), "{text}");
        assert!(!text.contains("hunter2"), "{text}");
        assert!(!text.contains("message"), "{text}");
    }

    /// A `.tmp` is a write that was interrupted, and nothing reads one or counts one.
    #[test]
    fn an_interrupted_write_is_not_a_report() {
        let home = tempfile::TempDir::new().expect("a temporary directory");
        std::fs::write(
            home.path().join("crash-1757000000000-1-0.json.tmp"),
            b"{ half",
        )
        .expect("a temporary directory is writable");

        assert_eq!(
            reports(home.path()).recorded(),
            Recorded {
                count: 0,
                newest: None
            }
        );
    }

    /// Recording switched off is `Skipped` and not `Ok`: a home that records nothing must not read
    /// as one that was examined and found clean.
    #[test]
    fn recording_switched_off_is_skipped() {
        let home = tempfile::TempDir::new().expect("a temporary directory");
        let mut reports = reports(home.path());
        reports.enabled = false;

        assert!(matches!(check(&reports).outcome, Outcome::Skipped { .. }));
    }

    /// A home that has never crashed is `Ok`.
    #[test]
    fn no_reports_is_ok() {
        let home = tempfile::TempDir::new().expect("a temporary directory");

        assert_eq!(check(&reports(home.path())).outcome, Outcome::Ok {});
    }

    /// A recorded crash is a note that says where to look and what to do, and never a problem.
    #[test]
    fn a_recorded_crash_is_a_note_that_names_the_directory() {
        let home = tempfile::TempDir::new().expect("a temporary directory");
        let reports = reports(home.path());
        reports
            .record(Timestamp(1_757_000_000_000), None, None, "")
            .expect("written");

        let Outcome::Note { because } = check(&reports).outcome else {
            panic!("a recorded crash is a note");
        };

        assert!(because.contains("1 crash report"), "{because}");
        assert!(because.contains("mix doctor --bundle"), "{because}");
        assert!(
            because.contains(&home.path().display().to_string()),
            "{because}"
        );
    }

    /// The installed hook records the panic it was installed for.
    ///
    /// **The previous hook is put back before the temporary directory goes**, so that nothing later
    /// in this test binary writes into a path that no longer exists. Other tests in this binary may
    /// panic while ours is installed and leave a report of their own here, which is why this looks
    /// for the one raised below rather than counting files.
    #[test]
    fn the_installed_hook_records_a_real_panic() {
        let home = tempfile::TempDir::new().expect("a temporary directory");
        let reports = reports(home.path());

        reports.install();
        let panicked = std::panic::catch_unwind(|| panic!("a panic raised on purpose, in a test"));
        let _ = std::panic::take_hook();

        assert!(panicked.is_err());

        let raised_here = files(home.path()).into_iter().any(|path| {
            std::fs::read(path)
                .ok()
                .and_then(|bytes| serde_json::from_slice::<CrashReport>(&bytes).ok())
                .and_then(|report| report.location)
                .is_some_and(|at| at.file.ends_with("crash.rs"))
        });

        assert!(raised_here, "{:?}", files(home.path()));
    }
}
