//! Where the daemon's own diagnostics go.
//!
//! Two sinks, always both: `logs/daemon.log`, because the daemon normally runs detached and
//! somebody will ask what happened hours later, and stderr, because during development somebody is
//! watching it right now. The file never carries colour — escape codes baked into `daemon.log`
//! would make "copy diagnostics" (T66) produce something no bug report can use.
//!
//! Format and level are decided at `main` and passed in. Nothing below this module reads the
//! environment.

mod rotating;

use std::io::{self, Write as _};
use std::num::NonZeroUsize;
use std::path::Path;
use std::sync::{Arc, Mutex, MutexGuard, PoisonError};

use mixengine_core::config::{LogFormat, LogLevel};
use tracing::Subscriber;
use tracing_subscriber::filter::LevelFilter;
use tracing_subscriber::fmt::MakeWriter;
use tracing_subscriber::fmt::format::Writer;
use tracing_subscriber::fmt::time::{FormatTime as _, SystemTime};
use tracing_subscriber::layer::SubscriberExt as _;

use rotating::{Note, RotatingFile};

/// Size at which `daemon.log` is moved aside — the same 10 MB the supervisor gives each service
/// log, so one answer covers `logs/` as a whole.
const MAX_BYTES: u64 = 10 * 1024 * 1024;

/// How many rotated copies to keep beside the live file: 60 MB of daemon log in the worst case,
/// which is a fortnight of an ordinary daemon and an afternoon of a crash loop.
const KEEP: NonZeroUsize = NonZeroUsize::new(5).expect("five is not zero");

/// What a rotation failure says. The daemon keeps logging either way, so the sentence has to be
/// about the file, not about the line that noticed.
const NOTE_MESSAGE: &str = "log rotation failed; this file will keep growing";

/// The `target` a rotation failure carries, so it filters and greps like any other line from here.
const NOTE_TARGET: &str = module_path!();

/// Everything the log needs to know, resolved by the caller.
#[derive(Debug, Clone, Copy)]
pub(crate) struct Options<'a> {
    /// `logs/daemon.log`, wherever `[paths] logs` put it.
    pub file: &'a Path,
    /// How much to log.
    pub level: LogLevel,
    /// How to shape each line.
    pub format: LogFormat,
    /// Whether stderr is a terminal that can render escape codes.
    pub colour: bool,
}

/// Install the daemon's subscriber for the rest of the process's life.
///
/// # Errors
///
/// [`io::Error`] when `daemon.log` cannot be opened. Deliberately fatal: a daemon that cannot
/// write its log is one nobody can support, and it is a symptom (a full disk, a `[paths] logs`
/// override onto an unmounted volume) that everything else is about to hit too.
///
/// # Panics
///
/// If a subscriber has already been installed, which only a second call could do.
pub(crate) fn init(options: &Options<'_>) -> io::Result<()> {
    let file = RotatingFile::open(
        options.file.to_path_buf(),
        MAX_BYTES,
        KEEP,
        note_for(options.format),
    )?;

    tracing::subscriber::set_global_default(subscriber(options, Sink::new(file)))
        .expect("the daemon installs its subscriber exactly once, here");

    Ok(())
}

/// How to word a rotation failure so that it reads like the lines around it.
///
/// The file is the only place this can be said from — an event would deadlock, see
/// [`RotatingFile::take_complaint`] — so the wording has to obey `log.format` by hand. Getting it
/// wrong is not cosmetic: a collector parsing one object per line stops on a line of prose.
fn note_for(format: LogFormat) -> Note {
    match format {
        LogFormat::Text => text_note,
        LogFormat::Json => json_note,
    }
}

/// `<timestamp> ERROR <target>: <message> error=<os error>`, the shape `fmt`'s default format
/// gives every other line in the file.
fn text_note(error: &io::Error) -> String {
    format!(
        "{} ERROR {NOTE_TARGET}: {NOTE_MESSAGE} error={error}\n",
        now()
    )
}

/// The same thing as one JSON object, with the keys the `json` formatter uses.
///
/// Built with `serde_json` rather than `format!`: an OS error message carries whatever the OS put
/// in it — a path with backslashes on Windows, a quoted file name — and escaping that by hand is
/// how a log stops being parseable at the one moment somebody needs to parse it.
fn json_note(error: &io::Error) -> String {
    let line = serde_json::json!({
        "timestamp": now(),
        "level": "ERROR",
        "fields": { "message": NOTE_MESSAGE, "error": error.to_string() },
        "target": NOTE_TARGET,
    });

    format!("{line}\n")
}

/// The clock `fmt` uses, borrowed rather than reimplemented so the note cannot drift into a
/// different timestamp format from the lines above it.
fn now() -> String {
    let mut formatted = String::new();
    // Infallible in practice: the only error `SystemTime` can return is one from the writer, and
    // writing to a `String` does not fail. An empty timestamp is still a parseable line.
    let _ = SystemTime.format_time(&mut Writer::new(&mut formatted));

    formatted
}

/// Build the subscriber over an already-open log file.
///
/// Boxed because the two formats produce two different types, and separate from [`init`] so tests
/// can run one under [`tracing::subscriber::with_default`] instead of claiming the process-global
/// one.
fn subscriber(options: &Options<'_>, file: Sink) -> Box<dyn Subscriber + Send + Sync> {
    // One filter for the whole subscriber rather than one per layer: a line that is worth writing
    // to the file is worth showing on stderr, and `max_level_hint` then lets `tracing` skip the
    // callsite entirely instead of formatting an event both layers would drop.
    let level = LevelFilter::from_level(match options.level {
        LogLevel::Error => tracing::Level::ERROR,
        LogLevel::Warn => tracing::Level::WARN,
        LogLevel::Info => tracing::Level::INFO,
        LogLevel::Debug => tracing::Level::DEBUG,
        LogLevel::Trace => tracing::Level::TRACE,
    });

    match options.format {
        LogFormat::Text => Box::new(
            tracing_subscriber::registry()
                .with(level)
                .with(
                    tracing_subscriber::fmt::layer()
                        .with_ansi(false)
                        .with_writer(file),
                )
                .with(
                    tracing_subscriber::fmt::layer()
                        .with_ansi(options.colour)
                        .with_writer(io::stderr),
                ),
        ),
        // Both sinks, not just the file: `MIXENGINE_LOG_FORMAT=json` is set by whoever is
        // collecting the output, and on a systemd or launchd machine that is stderr.
        LogFormat::Json => Box::new(
            tracing_subscriber::registry()
                .with(level)
                .with(tracing_subscriber::fmt::layer().json().with_writer(file))
                .with(
                    tracing_subscriber::fmt::layer()
                        .json()
                        .with_writer(io::stderr),
                ),
        ),
    }
}

/// The log file, shared by the layer that writes it and locked per event.
///
/// `tracing-subscriber` can make a writer out of a `Mutex` on its own, but that implementation
/// panics on a poisoned lock — one panic anywhere near the logger would take every later log call
/// down with it, including the ones reporting the original panic. A poisoned rotating file is
/// still a perfectly good file.
#[derive(Debug, Clone)]
struct Sink(Arc<Mutex<RotatingFile>>);

impl Sink {
    fn new(file: RotatingFile) -> Self {
        Self(Arc::new(Mutex::new(file)))
    }
}

impl<'a> MakeWriter<'a> for Sink {
    type Writer = SinkWriter<'a>;

    fn make_writer(&'a self) -> Self::Writer {
        SinkWriter(self.0.lock().unwrap_or_else(PoisonError::into_inner))
    }
}

/// The lock, held for exactly as long as one event takes to write.
#[derive(Debug)]
struct SinkWriter<'a>(MutexGuard<'a, RotatingFile>);

impl io::Write for SinkWriter<'_> {
    fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
        self.0.write(buf)
    }

    fn flush(&mut self) -> io::Result<()> {
        self.0.flush()
    }
}

impl Drop for SinkWriter<'_> {
    /// Give a rotation failure its second sink.
    ///
    /// Here rather than in the file, because "both sinks, always" is this module's rule and not a
    /// file's business, and here rather than one frame further out because this is the last place
    /// that still knows a complaint was produced. Written straight to the handle: `eprintln!`
    /// panics when stderr cannot be written, which a detached daemon (T9) would then do from
    /// inside its own logger, and a failure to say that rotation failed is not worth a process.
    fn drop(&mut self) {
        if let Some(note) = self.0.take_complaint() {
            let _ = io::stderr().write_all(note.as_bytes());
        }
    }
}

#[cfg(test)]
mod tests {
    use std::path::PathBuf;

    use tempfile::TempDir;

    use super::*;

    struct Log {
        home: TempDir,
        path: PathBuf,
    }

    impl Log {
        fn new() -> Self {
            let home = TempDir::new().unwrap();
            let path = home.path().join("daemon.log");

            Self { home, path }
        }

        /// Run `body` with a subscriber of this shape installed on the current thread only —
        /// `set_global_default` would be a one-shot per test binary.
        fn capture(&self, level: LogLevel, format: LogFormat, body: impl FnOnce()) -> String {
            let file =
                RotatingFile::open(self.path.clone(), MAX_BYTES, KEEP, note_for(format)).unwrap();
            let options = Options {
                file: &self.path,
                level,
                format,
                colour: false,
            };

            tracing::subscriber::with_default(subscriber(&options, Sink::new(file)), body);

            std::fs::read_to_string(&self.path).unwrap()
        }
    }

    #[test]
    fn the_file_gets_the_event_and_its_fields() {
        let log = Log::new();

        let written = log.capture(LogLevel::Info, LogFormat::Text, || {
            tracing::info!(port = 8080, "listening");
        });

        assert!(written.contains("listening"), "{written}");
        assert!(written.contains("port=8080"), "{written}");
        assert!(written.contains("INFO"), "{written}");
    }

    #[test]
    fn the_file_never_carries_colour() {
        let log = Log::new();

        // `colour` is about stderr; a terminal-detecting daemon must not put escape codes in the
        // file it hands to a bug report.
        let file = RotatingFile::open(log.path.clone(), MAX_BYTES, KEEP, text_note).unwrap();
        let options = Options {
            file: &log.path,
            level: LogLevel::Info,
            format: LogFormat::Text,
            colour: true,
        };

        tracing::subscriber::with_default(subscriber(&options, Sink::new(file)), || {
            tracing::error!("something went wrong");
        });

        let written = std::fs::read_to_string(&log.path).unwrap();
        assert!(!written.contains('\u{1b}'), "{written:?}");
        assert!(written.contains("something went wrong"), "{written}");
    }

    #[test]
    fn json_is_one_object_per_line() {
        let log = Log::new();

        let written = log.capture(LogLevel::Info, LogFormat::Json, || {
            tracing::info!(port = 8080, "listening");
            tracing::warn!("and again");
        });

        let lines: Vec<&str> = written.lines().collect();
        assert_eq!(lines.len(), 2, "{written}");

        let first: serde_json::Value = serde_json::from_str(lines[0]).unwrap();
        assert_eq!(first["fields"]["message"], "listening");
        assert_eq!(first["fields"]["port"], 8080);
        assert_eq!(first["level"], "INFO");
        assert!(first["timestamp"].is_string(), "{first}");

        serde_json::from_str::<serde_json::Value>(lines[1]).unwrap();
    }

    #[test]
    fn a_quieter_level_leaves_the_line_out_of_the_file() {
        let log = Log::new();

        let written = log.capture(LogLevel::Warn, LogFormat::Text, || {
            tracing::info!("routine");
            tracing::warn!("degraded");
        });

        assert!(!written.contains("routine"), "{written}");
        assert!(written.contains("degraded"), "{written}");
    }

    #[test]
    fn a_rotation_failure_stays_one_json_object_per_line() {
        // The whole point of shaping the note by format. A collector reading `daemon.log` as JSON
        // meets this line exactly when something has already gone wrong, and a line of prose there
        // is a parse error on top of a rotation failure.
        let log = Log::new();
        let real = log.capture(LogLevel::Info, LogFormat::Json, || {
            tracing::info!("listening")
        });
        let real: serde_json::Value = serde_json::from_str(real.trim_end()).unwrap();

        let note = json_note(&io::Error::other("the file is held by another process"));

        assert!(note.ends_with('\n'), "{note:?}");
        assert_eq!(note.lines().count(), 1, "{note:?}");

        let parsed: serde_json::Value = serde_json::from_str(&note).unwrap();
        assert_eq!(parsed["level"], "ERROR");
        assert_eq!(parsed["fields"]["message"], NOTE_MESSAGE);
        assert_eq!(
            parsed["fields"]["error"],
            "the file is held by another process"
        );
        assert!(parsed["timestamp"].is_string(), "{parsed}");

        // Not a fixed list of keys: the point is that a collector configured for the lines
        // `tracing` writes recognises this one too, so the comparison is against a real line.
        let keys = |value: &serde_json::Value| -> Vec<String> {
            let mut keys: Vec<String> = value.as_object().unwrap().keys().cloned().collect();
            keys.sort();
            keys
        };
        assert_eq!(keys(&parsed), keys(&real));
    }

    #[test]
    fn a_rotation_failure_reads_like_a_text_line() {
        let note = text_note(&io::Error::other("no space left on device"));

        assert!(note.ends_with('\n'), "{note:?}");
        assert_eq!(note.lines().count(), 1, "{note:?}");
        assert!(note.contains("ERROR"), "{note}");
        assert!(note.contains(NOTE_TARGET), "{note}");
        assert!(note.contains(NOTE_MESSAGE), "{note}");
        assert!(note.contains("no space left on device"), "{note}");
        // The file never carries colour, and neither does anything written into it by hand.
        assert!(!note.contains('\u{1b}'), "{note:?}");
    }

    #[test]
    fn the_log_lands_where_the_caller_pointed_it() {
        // The whole reason `Options::file` is a path and not a directory: a `[paths] logs`
        // override moves the daemon's own log with it.
        let log = Log::new();

        log.capture(LogLevel::Info, LogFormat::Text, || tracing::info!("here"));

        assert!(log.path.is_file());
        assert!(log.path.starts_with(log.home.path()));
    }
}
