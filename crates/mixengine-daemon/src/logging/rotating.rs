//! `daemon.log`, kept to a bounded size.
//!
//! Size-based rather than time-based, and enforced here rather than by logrotate or the Task
//! Scheduler: the same rule has to hold on all three operating systems, and a daemon that runs for
//! months on one machine and twenty minutes on another produces wildly different amounts of log
//! per day but the same amount per event.
//!
//! Nothing here buffers. A log file whose last few lines are still sitting in a `BufWriter` when
//! the process dies is missing exactly the lines that explain why, and a daemon writes few enough
//! of them for the syscall not to matter.

use std::fs::{File, OpenOptions};
use std::io::{self, Write};
use std::num::NonZeroUsize;
use std::path::{Path, PathBuf};

/// Shapes the line a failed rotation leaves behind.
///
/// Supplied by the caller because this type knows about bytes and the caller knows about
/// `log.format`: a plain-text complaint in a file the collector is parsing as one JSON object per
/// line is a parse error at exactly the moment something is already wrong.
pub(crate) type Note = fn(&io::Error) -> String;

/// An append-only file that moves itself aside once it grows past `max_bytes`.
#[derive(Debug)]
pub(crate) struct RotatingFile {
    /// The live file, `logs/daemon.log`.
    path: PathBuf,
    /// The size at which the next line starts a new file.
    max_bytes: u64,
    /// How many rotated copies to keep beside it.
    keep: NonZeroUsize,
    /// How to word a rotation failure, in whatever shape the rest of the file is written in.
    note: Note,
    /// `None` before the first write and between a rotation and the write that follows it — the
    /// handle has to be closed for the rename to be allowed on Windows.
    file: Option<File>,
    /// Bytes in [`Self::path`], tracked rather than queried: `metadata()` per log line would be a
    /// syscall per line, and this process is the only writer.
    written: u64,
    /// Whether the current run of rotation failures has already been recorded in the log.
    complained: bool,
    /// The complaint the last write produced, waiting to be collected by [`Self::take_complaint`].
    complaint: Option<String>,
}

impl RotatingFile {
    /// Open `path`, appending to whatever is already there.
    ///
    /// # Errors
    ///
    /// [`io::Error`] when the file cannot be opened — reported now rather than at the first log
    /// line, so a daemon that cannot write its log says so while somebody is still watching its
    /// stderr.
    pub(crate) fn open(
        path: PathBuf,
        max_bytes: u64,
        keep: NonZeroUsize,
        note: Note,
    ) -> io::Result<Self> {
        let mut rotating = Self {
            path,
            max_bytes,
            keep,
            note,
            file: None,
            written: 0,
            complained: false,
            complaint: None,
        };

        rotating.handle()?;

        Ok(rotating)
    }

    /// The open file, opening it if this is the first write or the last one rotated.
    fn handle(&mut self) -> io::Result<&mut File> {
        if self.file.is_none() {
            // `append`, not `write`: a second writer — `mix doctor`, or a second daemon that has
            // not yet lost the single-instance race (T9) — then interleaves whole lines instead of
            // overwriting from a stale offset.
            let file = OpenOptions::new()
                .create(true)
                .append(true)
                .open(&self.path)?;

            // Whatever is in the file counts towards the limit, so a daemon restarted every few
            // minutes cannot keep appending to a file that is already over it.
            self.written = file.metadata()?.len();
            self.file = Some(file);
        }

        Ok(self
            .file
            .as_mut()
            .expect("the branch above leaves a file behind"))
    }

    /// Move the live file aside and shift the rotated copies up one.
    ///
    /// Never fails the write that triggered it: see [`Self::complain`].
    fn rotate(&mut self) {
        // The handle goes first, and this is not a detail: Rust opens files without
        // `FILE_SHARE_DELETE`, so Windows refuses to rename one this very process still holds
        // open. With the two lines the other way round, three of the tests below fail on Windows
        // with "access denied" and none of them on Linux or macOS.
        self.file = None;

        match self.shift() {
            Ok(()) => {
                // Nothing is left at `path` for the next line to measure itself against.
                // [`Self::handle`] would re-read this from the new file anyway — but only if it
                // manages to open one, and the case where it does not is a full disk, which fails
                // `create` while the `rename` above needed no space at all. Left holding the old
                // file's size, the next line would rotate again, and the one after that, until
                // the history explaining the full disk had been shifted off the end of `keep`.
                self.written = 0;
                self.complained = false;
            }
            Err(error) => self.complain(&error),
        }
    }

    /// `daemon.log.4` → `daemon.log.5`, …, `daemon.log` → `daemon.log.1`.
    ///
    /// Oldest first, so nothing is overwritten before it has been moved. The copy that falls off
    /// the end is not deleted first: `rename` replaces its destination on every platform MixEngine
    /// supports, which is one syscall rather than two and leaves no window in which the history is
    /// one file short.
    fn shift(&self) -> io::Result<()> {
        for index in (1..self.keep.get()).rev() {
            move_aside(&self.numbered(index), &self.numbered(index + 1))?;
        }

        move_aside(&self.path, &self.numbered(1))
    }

    /// `logs/daemon.log.3`, and so on.
    fn numbered(&self, index: usize) -> PathBuf {
        let mut name = self.path.clone().into_os_string();
        name.push(format!(".{index}"));

        PathBuf::from(name)
    }

    /// Record a failed rotation *in the log itself*, once per run of failures.
    ///
    /// The alternative is to return the error from [`Write::write`], where `tracing` discards it —
    /// the line that triggered the rotation would be lost, and the reason for losing it with it.
    /// So the file grows past its limit instead, and says why. A retry happens on the next line
    /// anyway; the flag only keeps a rename that keeps failing (a file locked by a backup agent,
    /// an antivirus scanner, a full disk) from filling the log with copies of its own complaint.
    fn complain(&mut self, error: &io::Error) {
        if self.complained {
            return;
        }
        self.complained = true;

        let note = (self.note)(error);

        // Nothing left to try if even this cannot be written: it *is* the error path of the
        // logger, and the daemon has a service to supervise.
        let recorded = match self.handle() {
            Ok(file) => file.write_all(note.as_bytes()).is_ok(),
            Err(_) => false,
        };

        if recorded {
            self.written = self.written.saturating_add(note.len() as u64);
        }

        // Whether or not the file took it — a note about a file that is failing is worth least in
        // that same file.
        self.complaint = Some(note);
    }

    /// The complaint the last write produced, if any, and only once.
    ///
    /// Handed out rather than emitted here: this type owns one file, and the note has to reach the
    /// *other* sink as well. `tracing::error!` is the obvious way to say it and is the one way it
    /// cannot be said — the event would go straight back into this writer, whose mutex the write
    /// that produced the complaint is still holding, and the daemon would deadlock inside its own
    /// logger.
    pub(crate) fn take_complaint(&mut self) -> Option<String> {
        self.complaint.take()
    }
}

impl Write for RotatingFile {
    /// Write one formatted event, whole.
    ///
    /// `tracing` hands over a complete line per call, and the whole of it is written or none of
    /// it, so rotation always happens on a line boundary and no event is ever split across two
    /// files.
    fn write(&mut self, line: &[u8]) -> io::Result<usize> {
        // An empty file is never rotated: a single line longer than the limit — a panic with a
        // long backtrace, a config error quoting the file — would otherwise be rotated away
        // before being written and every file in the history would be empty.
        if self.written > 0 && self.written.saturating_add(line.len() as u64) > self.max_bytes {
            self.rotate();
        }

        let file = self.handle()?;
        file.write_all(line)?;
        self.written = self.written.saturating_add(line.len() as u64);

        Ok(line.len())
    }

    fn flush(&mut self) -> io::Result<()> {
        match self.file.as_mut() {
            Some(file) => file.flush(),
            None => Ok(()),
        }
    }
}

/// `rename`, treating "there is nothing there" as success.
///
/// A log that has not rotated `keep` times yet has no `daemon.log.5` to move, and a fresh install
/// has no `daemon.log` either — neither is a failure worth reporting.
fn move_aside(from: &Path, to: &Path) -> io::Result<()> {
    match std::fs::rename(from, to) {
        Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(()),
        result => result,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Deliberately tiny: the rotation rule is about crossing a boundary, and a test that has to
    /// write ten megabytes to reach one proves nothing extra.
    const LIMIT: u64 = 32;

    fn keep(count: usize) -> NonZeroUsize {
        NonZeroUsize::new(count).expect("the tests never ask for a history of zero files")
    }

    /// Deliberately not the daemon's own wording: what these tests check is that the note comes
    /// from the caller, not that this module still contains a particular sentence. The two real
    /// ones are checked where they are written, in the parent module.
    fn note(error: &io::Error) -> String {
        format!("rotation failed: {error}\n")
    }

    fn open(directory: &Path, count: usize) -> RotatingFile {
        RotatingFile::open(directory.join("daemon.log"), LIMIT, keep(count), note).unwrap()
    }

    fn read(path: &Path) -> String {
        std::fs::read_to_string(path).unwrap_or_else(|_| panic!("{} is missing", path.display()))
    }

    #[test]
    fn an_existing_file_is_appended_to_rather_than_truncated() {
        let home = tempfile::TempDir::new().unwrap();
        let path = home.path().join("daemon.log");
        std::fs::write(&path, b"from the previous run\n").unwrap();

        let mut log = RotatingFile::open(path.clone(), LIMIT, keep(2), note).unwrap();
        log.write_all(b"now\n").unwrap();

        assert_eq!(read(&path), "from the previous run\nnow\n");
    }

    #[test]
    fn a_line_that_would_cross_the_limit_starts_a_new_file() {
        // Also the Windows test: the rename happens while this process still holds the file open,
        // which the OS refuses unless the handle is dropped first.
        let home = tempfile::TempDir::new().unwrap();
        let mut log = open(home.path(), 2);

        log.write_all(b"0123456789012345678901234567890\n").unwrap();
        log.write_all(b"the line that does not fit\n").unwrap();

        assert_eq!(
            read(&home.path().join("daemon.log")),
            "the line that does not fit\n"
        );
        assert_eq!(
            read(&home.path().join("daemon.log.1")),
            "0123456789012345678901234567890\n"
        );
    }

    #[test]
    fn a_line_is_never_split_across_two_files() {
        let home = tempfile::TempDir::new().unwrap();
        let mut log = open(home.path(), 2);

        log.write_all(b"short\n").unwrap();
        // Four times the limit on its own — a backtrace, or a rendered config file in an error.
        let long = format!("{}\n", "x".repeat(LIMIT as usize * 4));
        log.write_all(long.as_bytes()).unwrap();

        assert_eq!(read(&home.path().join("daemon.log")), long);
        assert_eq!(read(&home.path().join("daemon.log.1")), "short\n");
    }

    #[test]
    fn only_the_configured_number_of_rotated_files_survives() {
        let home = tempfile::TempDir::new().unwrap();
        let mut log = open(home.path(), 3);

        // Every line fills the file on its own, so this is five rotations, not five lines.
        for generation in 0..6 {
            log.write_all(format!("{generation} {}\n", "-".repeat(LIMIT as usize)).as_bytes())
                .unwrap();
        }

        // Newest first: the live file, then the three kept copies, then nothing.
        assert!(read(&home.path().join("daemon.log")).starts_with('5'));
        assert!(read(&home.path().join("daemon.log.1")).starts_with('4'));
        assert!(read(&home.path().join("daemon.log.2")).starts_with('3'));
        assert!(read(&home.path().join("daemon.log.3")).starts_with('2'));
        assert!(
            !home.path().join("daemon.log.4").exists(),
            "the history grew past `keep`"
        );
    }

    #[test]
    fn a_rotation_that_cannot_happen_costs_no_log_lines() {
        let home = tempfile::TempDir::new().unwrap();
        let mut log = open(home.path(), 1);

        // A directory where the rotated copy has to go: `rename` cannot replace one with a file on
        // any of the three platforms, and it is the closest stand-in for the real cause — a file
        // another process is holding open, which only Windows would refuse.
        std::fs::create_dir(home.path().join("daemon.log.1")).unwrap();

        log.write_all(b"0123456789012345678901234567890\n").unwrap();
        log.write_all(b"the line that does not fit\n").unwrap();
        log.write_all(b"and the one after it\n").unwrap();

        let live = read(&home.path().join("daemon.log"));
        assert!(live.contains("the line that does not fit"), "{live}");
        assert!(live.contains("and the one after it"), "{live}");
        assert_eq!(
            live.matches("rotation failed").count(),
            1,
            "the complaint belongs in the log once per run of failures, not once per line: {live}"
        );
    }

    #[test]
    fn a_rotation_leaves_no_size_behind_for_the_next_one_to_find() {
        // White-box on purpose: the size is what decides the *next* rotation, and the path this
        // protects — the open after a successful rename failing on a full disk — is the one thing
        // no portable filesystem trick can provoke between two writes. Left as it was, every
        // following line would rotate again and walk the history off the end of `keep`, deleting
        // the evidence of the full disk one generation per line.
        let home = tempfile::TempDir::new().unwrap();
        let mut log = open(home.path(), 3);
        log.write_all(b"0123456789012345678901234567890\n").unwrap();

        log.rotate();

        assert_eq!(log.written, 0);
    }

    #[test]
    fn a_failed_rotation_is_offered_to_the_other_sink_once() {
        // The file the complaint is written to is the file that is failing, and under a collector
        // nobody is reading it — stderr is. Collected by the caller, which owns both sinks.
        let home = tempfile::TempDir::new().unwrap();
        let mut log = open(home.path(), 1);
        std::fs::create_dir(home.path().join("daemon.log.1")).unwrap();

        log.write_all(b"0123456789012345678901234567890\n").unwrap();
        assert_eq!(log.take_complaint(), None, "nothing has failed yet");

        log.write_all(b"the line that does not fit\n").unwrap();
        let complaint = log
            .take_complaint()
            .expect("the rename could not have worked");
        assert!(complaint.contains("rotation failed"), "{complaint}");

        log.write_all(b"and the one after it\n").unwrap();
        assert_eq!(
            log.take_complaint(),
            None,
            "a complaint is handed over once, not on every line of a run of failures"
        );
    }
}
