//! What a client reads on `GET /logs/{id}`: one service's output, from before it connected and from
//! after.
//!
//! **This is not a second capture.** `mixengine-supervisor` reads the pipes, splits the lines,
//! writes `current.log` and keeps the ring a crash-loop cutoff quotes; all of that belongs to *one
//! run* of the process and is dropped with it. What lives here is the half a client needs and that
//! one cannot give: a place to read from that outlives any single run, so a `mix service logs -f`
//! left open across a crash, a backoff and a restart keeps printing rather than ending three times.
//!
//! **Why the daemon holds a ring of its own.** A subscription is only ever "from now on", and a log
//! panel that opened with nothing would be useless — the lines somebody wants are the ones printed
//! *before* they looked. Handing over that tail and the subscription in the same breath is what makes
//! the seam impossible: [`ServiceLog::read`] takes both under one lock, so no line can arrive between
//! the two and be lost, and none can be delivered twice. That is the whole reason this ring is not
//! the supervisor's — a snapshot taken there and a subscription taken here could not be made one
//! decision across two crates.
//!
//! **The file answers only when this daemon has nothing of its own.** `current.log` is plain text
//! and carries no timestamp and no stream tag — deliberately, so that whoever reads MariaDB's log
//! reads this one with the same tools — so lines recovered from it are
//! [`LogFrame::Historic`] and say plainly that only their text
//! survived. It is read when the ring is empty and never mixed with it: a daemon that has been up
//! since the service started has the better answer already, and stitching the two would mean
//! guessing where one ends in a file something else is still appending to.
//!
//! See `.claude/decisions/0009-logs-travel-on-their-own-stream.md` for why none of this is an event.

use std::collections::HashMap;
use std::collections::VecDeque;
use std::io::{self, Read as _, Seek as _, SeekFrom};
use std::path::Path;
use std::sync::{Arc, Mutex};

use mixengine_proto::{LogFrame, LogLine, LogSubject};
use tokio::sync::broadcast;

use mixengine_supervisor::logs::CURRENT_LOG_FILE_NAME;

/// How many frames a connected client may fall behind by before it starts missing lines.
///
/// Only reached by a client that has stopped reading: the endpoint writes into a response body hyper
/// polls as fast as the client drains it, so falling behind here means a reader that is slower than
/// the *service*, sustained. It is told what it lost — see [`LogFrame::Gap`] — rather than being
/// buffered without bound, which is the one thing a daemon supervising somebody else's program
/// cannot afford to do.
const BACKLOG: usize = 1024;

/// How much of the end of `current.log` is read when the ring is empty.
///
/// A bound rather than a line count, because the file is rotated at ten megabytes by default and a
/// service that prints one very long line should not be able to make a connecting client read all of
/// it. Whatever whole lines fit in this are what the tail is taken from.
const HISTORIC_BYTES: u64 = 256 * 1024;

/// Every service's output, whether or not anything is supervising it right now.
///
/// One per daemon, held by the registry. An entry is made the first time a service runs or the first
/// time somebody asks to read it, and it is dropped when the runner ends with nobody watching — see
/// [`Logs::forget_if_unwatched`]. An entry nobody is watching costs a receiver-less broadcast and
/// whatever the ring last held, which is the price of `mix service logs mariadb` still explaining a
/// service that failed ten minutes ago.
/// **Keyed by subject rather than by service** — roadmap task **T78a**, its design's D13. A job that
/// runs somebody else's command needs the same ring, the same subscription and the same
/// back-pressure a service's output needs, and giving it a second surface would be two
/// implementations of ADR 0009 to keep in step.
#[derive(Debug, Default)]
pub(crate) struct Logs {
    subjects: Mutex<HashMap<LogSubject, Arc<ServiceLog>>>,
}

impl Logs {
    /// A daemon that has seen nothing yet.
    pub(crate) fn new() -> Self {
        Self::default()
    }

    /// This subject's log, created if it has none.
    fn of(&self, subject: &LogSubject) -> Arc<ServiceLog> {
        Arc::clone(
            lock(&self.subjects)
                .entry(subject.clone())
                .or_insert_with(|| Arc::new(ServiceLog::new())),
        )
    }

    /// The log a runner feeds while it has a captured process, sized by that service's own policy.
    ///
    /// `keep` is `LogPolicy::ring_lines` from the spec, and it is applied here rather than fixed
    /// because a service told to keep nothing in memory means it: the ring shrinks to what the
    /// policy allows and a client then sees only what arrives after it connects.
    pub(crate) fn feeding(&self, subject: &LogSubject, keep: usize) -> Arc<ServiceLog> {
        let log = self.of(subject);

        log.keep(keep);

        log
    }

    /// The log a connecting client reads.
    pub(crate) fn reading(&self, subject: &LogSubject) -> Arc<ServiceLog> {
        self.of(subject)
    }

    /// Drop this service's log if nobody is reading it.
    ///
    /// Called where a runner ends. A client with a `follow` open keeps the entry — that is what lets
    /// the stream carry on when the service is started again — and one that has gone leaves the
    /// daemon holding nothing for a service that is no longer running.
    ///
    /// **The ring goes with it**, which is deliberate and is the one cost: `mix service logs` on a
    /// stopped service answers from memory only while somebody is still watching it. The lines are
    /// in `current.log` either way, and that is where a later reader is sent.
    pub(crate) fn forget_if_unwatched(&self, subject: &LogSubject) {
        let mut subjects = lock(&self.subjects);

        if subjects.get(subject).is_some_and(|log| log.watchers() == 0) {
            subjects.remove(subject);
        }
    }
}

/// One service's output: the last lines, and where the next ones go.
#[derive(Debug)]
pub(crate) struct ServiceLog {
    /// The last `keep` lines, oldest first.
    ///
    /// **Taken under the same lock as [`ServiceLog::lines`] is published on**, which is the property
    /// the whole type exists for: a reader takes its tail and its subscription without a line being
    /// able to slip between them.
    ring: Mutex<Ring>,

    /// Every frame from now on, for whoever is connected.
    lines: broadcast::Sender<LogFrame>,
}

/// The kept lines and how many of them are kept.
#[derive(Debug, Default)]
struct Ring {
    lines: VecDeque<LogLine>,

    /// From the running service's `LogPolicy`, or zero for a service this daemon has not started —
    /// which keeps nothing until a runner says otherwise, rather than guessing a size for a service
    /// whose spec has not been read.
    keep: usize,
}

impl ServiceLog {
    fn new() -> Self {
        Self {
            ring: Mutex::new(Ring::default()),
            lines: broadcast::Sender::new(BACKLOG),
        }
    }

    /// Record one line: into the ring and out to everybody connected, in that order and under one
    /// lock.
    ///
    /// The lock is held across the publish on purpose. Without it a line could be sent to the stream
    /// while a connecting client is between reading the ring and subscribing — the one gap that
    /// would lose a line with nothing to report it.
    pub(crate) fn record(&self, line: LogLine) {
        let mut ring = lock(&self.ring);

        // Checked before the push, so the ring never briefly holds one more than the policy allows,
        // and a policy of zero keeps nothing at all.
        while ring.lines.len() >= ring.keep {
            if ring.lines.pop_front().is_none() {
                break;
            }
        }

        if ring.keep > 0 {
            ring.lines.push_back(line.clone());
        }

        // Fails only when nobody is connected, which is the ordinary state of a running service.
        let _ = self.lines.send(LogFrame::Line(line));
    }

    /// Tell everybody connected that the daemon itself lost lines.
    ///
    /// Not the same gap as a slow client's: this one is the relay between a capture and this log
    /// falling behind a service that printed faster than it could be forwarded, and it is the whole
    /// stream's loss rather than one reader's. Said in the stream anyway, because a hole nobody
    /// mentions is the failure this frame exists to prevent.
    pub(crate) fn missed(&self, lines: u64) {
        let _ = self.lines.send(LogFrame::Gap { missed: lines });
    }

    /// What this service's ring keeps, from the policy of the spec that is running.
    fn keep(&self, keep: usize) {
        lock(&self.ring).keep = keep;
    }

    /// How many connected clients there are, which is what decides whether this log outlives its
    /// runner.
    fn watchers(&self) -> usize {
        self.lines.receiver_count()
    }

    /// The last `tail` lines, and everything after them.
    ///
    /// **One call rather than two**, and that is the point: a client that asked for a tail and then
    /// subscribed would either lose whatever was printed in between or see it twice, with no way to
    /// tell which. Both are taken under the ring's lock, so there is no in between.
    pub(crate) fn read(&self, tail: usize) -> (Vec<LogFrame>, broadcast::Receiver<LogFrame>) {
        let ring = lock(&self.ring);
        let subscription = self.lines.subscribe();

        let from = ring.lines.len().saturating_sub(tail);
        let recent = ring
            .lines
            .iter()
            .skip(from)
            .cloned()
            .map(LogFrame::Line)
            .collect();

        (recent, subscription)
    }
}

/// The last whole lines of a service's `current.log`, for a daemon whose ring is empty.
///
/// Blocking, and called through `spawn_blocking` — reading the end of a ten-megabyte file is not
/// work for a runtime thread that has connections to serve.
///
/// **Read backwards from the end**, not forwards from the start: what a client wants is the last
/// `tail` lines, and a rotated file at its full size is ten megabytes of what it did not ask for. The
/// first line of what is read is dropped unless the window happens to begin at the very start of the
/// file, because a window that lands mid-line would otherwise report half a sentence as a whole one.
///
/// An unreadable file is no lines rather than an error: a service that has never run has none, and a
/// log directory that cannot be read is a fact about the machine that a client asking for output
/// cannot act on. The sentence for it is in `daemon.log`, where the daemon's own voice belongs.
pub(crate) fn historic(directory: &Path, tail: usize) -> Vec<LogFrame> {
    let path = directory.join(CURRENT_LOG_FILE_NAME);

    let read = || -> io::Result<Vec<u8>> {
        let mut file = std::fs::File::open(&path)?;
        let end = file.seek(SeekFrom::End(0))?;
        let from = end.saturating_sub(HISTORIC_BYTES);

        file.seek(SeekFrom::Start(from))?;

        let mut bytes = Vec::with_capacity(usize::try_from(end - from).unwrap_or(0));
        file.take(HISTORIC_BYTES).read_to_end(&mut bytes)?;

        Ok(bytes)
    };

    let bytes = match read() {
        Ok(bytes) => bytes,

        Err(error) if error.kind() == io::ErrorKind::NotFound => return Vec::new(),

        Err(error) => {
            tracing::warn!(
                path = %path.display(),
                %error,
                "cannot read a service's log file for a client asking for its output"
            );

            return Vec::new();
        }
    };

    let whole_file = bytes.len() < usize::try_from(HISTORIC_BYTES).unwrap_or(usize::MAX);
    let text = String::from_utf8_lossy(&bytes);
    let mut lines: Vec<&str> = text.lines().collect();

    // The window began mid-line unless it began at the start of the file.
    if !whole_file && !lines.is_empty() {
        lines.remove(0);
    }

    let from = lines.len().saturating_sub(tail);

    lines[from..]
        .iter()
        .map(|line| LogFrame::Historic {
            text: (*line).to_owned(),
        })
        .collect()
}

/// A poisoned lock here means a task panicked while recording a line; the lines already in the ring
/// are still the lines the service printed, so this takes them and carries on rather than spreading
/// the panic to a daemon that has services to supervise.
fn lock<T>(mutex: &Mutex<T>) -> std::sync::MutexGuard<'_, T> {
    mutex.lock().unwrap_or_else(|poisoned| {
        mutex.clear_poison();
        poisoned.into_inner()
    })
}

#[cfg(test)]
mod tests {
    use mixengine_proto::{JobId, ServiceId, Stream, Timestamp};

    use super::*;

    /// One line, as a service would have printed it.
    fn line(text: &str) -> LogLine {
        LogLine {
            stream: Stream::Stdout,
            at: Timestamp(1_760_000_000_000),
            text: text.to_owned(),
        }
    }

    /// The text of whatever frames these are, so a test reads as what a client would see.
    fn texts(frames: &[LogFrame]) -> Vec<&str> {
        frames
            .iter()
            .map(|frame| match frame {
                LogFrame::Line(line) => line.text.as_str(),
                LogFrame::Historic { text } => text.as_str(),
                LogFrame::Gap { .. } => "<gap>",
                _ => "<unknown>",
            })
            .collect()
    }

    #[test]
    fn a_reader_gets_the_tail_it_asked_for_and_no_more() {
        let log = ServiceLog::new();
        log.keep(10);

        for text in ["one", "two", "three"] {
            log.record(line(text));
        }

        let (tail, _) = log.read(2);

        assert_eq!(texts(&tail), ["two", "three"]);
    }

    /// The seam this whole type exists to remove: what is in the tail is not in the stream, and
    /// what is in the stream was not in the tail.
    #[tokio::test]
    async fn the_tail_and_the_stream_do_not_overlap_and_do_not_gap() {
        let log = ServiceLog::new();
        log.keep(10);

        log.record(line("before"));

        let (tail, mut stream) = log.read(10);

        log.record(line("after"));

        assert_eq!(texts(&tail), ["before"]);
        assert_eq!(
            stream.recv().await.unwrap(),
            LogFrame::Line(line("after")),
            "the line published after the read is the first one on the stream"
        );
    }

    /// A service whose policy keeps nothing in memory keeps nothing here either.
    #[test]
    fn a_ring_of_zero_lines_keeps_nothing() {
        let log = ServiceLog::new();
        log.keep(0);

        log.record(line("gone"));

        assert!(log.read(10).0.is_empty());
    }

    /// A daemon that has never started a service has no size to keep by, and must not invent one.
    #[test]
    fn a_service_this_daemon_has_not_started_keeps_nothing_until_it_does() {
        let logs = Logs::new();
        let id = LogSubject::Service {
            id: ServiceId::parse("caddy").unwrap(),
        };

        logs.reading(&id).record(line("stray"));
        assert!(logs.reading(&id).read(10).0.is_empty());

        logs.feeding(&id, 5).record(line("kept"));
        assert_eq!(texts(&logs.reading(&id).read(10).0), ["kept"]);
    }

    #[test]
    fn a_log_nobody_is_watching_is_forgotten_when_its_runner_ends() {
        let logs = Logs::new();
        let id = LogSubject::Service {
            id: ServiceId::parse("caddy").unwrap(),
        };

        logs.feeding(&id, 5).record(line("kept"));
        logs.forget_if_unwatched(&id);

        assert!(
            logs.reading(&id).read(10).0.is_empty(),
            "the entry was dropped, so the ring went with it"
        );
    }

    /// **A job's output is its own ring** — roadmap task **T78a**, its design's D13. Two subjects
    /// that a single string would have spelled the same way are two logs here.
    #[test]
    fn a_job_and_a_service_do_not_share_a_ring() {
        let logs = Logs::new();
        let job = LogSubject::Job { id: JobId(1) };
        let service = LogSubject::Service {
            id: ServiceId::parse("caddy").unwrap(),
        };

        logs.feeding(&job, 5).record(line("from the command"));

        assert!(
            logs.reading(&service).read(10).0.is_empty(),
            "a service is not told what a job printed"
        );
        assert_eq!(texts(&logs.reading(&job).read(10).0), ["from the command"]);
    }

    /// The case the entry has to survive: a `follow` open across a service that is not running.
    #[tokio::test]
    async fn a_log_somebody_is_watching_survives_its_runner_and_carries_on() {
        let logs = Logs::new();
        let id = LogSubject::Service {
            id: ServiceId::parse("caddy").unwrap(),
        };

        let (_, mut stream) = logs.reading(&id).read(0);

        logs.forget_if_unwatched(&id);

        // The service starts again, under a new runner.
        logs.feeding(&id, 5).record(line("restarted"));

        assert_eq!(
            stream.recv().await.unwrap(),
            LogFrame::Line(line("restarted")),
            "the same connection keeps printing across a restart"
        );
    }

    #[test]
    fn the_file_answers_with_what_survived_of_a_line_and_says_so() {
        let directory = tempfile::tempdir().unwrap();
        std::fs::write(
            directory.path().join(CURRENT_LOG_FILE_NAME),
            "first\nsecond\nthird\n",
        )
        .unwrap();

        let frames = historic(directory.path(), 2);

        assert_eq!(texts(&frames), ["second", "third"]);
        assert!(
            frames
                .iter()
                .all(|frame| matches!(frame, LogFrame::Historic { .. })),
            "a line read back from the file has no stream and no timestamp to claim"
        );
    }

    #[test]
    fn a_service_that_has_never_run_has_no_file_and_that_is_not_a_failure() {
        let directory = tempfile::tempdir().unwrap();

        assert!(historic(directory.path(), 10).is_empty());
    }
}
