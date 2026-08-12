//! Everything a service says, split into lines and kept where the last of it can be read back.
//!
//! **This is the half of log capture T15 needs and no more.** Per-service files, size rotation,
//! `LogLine` events on the daemon's stream and `GET /logs/{id}?follow=1` are roadmap task T16, and
//! they are built on top of what is here rather than beside it: a [`Capture`] already splits the
//! lines, timestamps them and hands them to whoever subscribed, so T16 adds a subscriber that
//! writes to a file and a second one that publishes.
//!
//! Two things in T15 need it, which is why it comes first. A crash-loop cutoff attaches the last
//! lines a service printed to the failure it reports — that is the difference between a GUI that
//! says "failed" and one that says "address already in use" — and `ReadyCheck::LogPattern` is a
//! service announcing itself on a stream nobody can poll.
//!
//! # Threads, not tasks
//!
//! One thread per stream, reading a blocking pipe, and that is deliberate rather than a shortcut.
//! `spawn_supervised` hands back the standard library's pipes, and an anonymous pipe on Windows
//! cannot be read with overlapped I/O at all — `tokio::process` gets around that by creating named
//! pipes for its own children, which is not what this crate spawns. So the choice is a thread per
//! stream or a polling loop that would either add latency to every line or spin; a blocking read on
//! a thread is what `.claude/standards/rust.md` means by "a dedicated task".
//!
//! The threads end when the pipe reaches end of file, which happens when every process holding the
//! write end has exited — not when the service does. A worker it forked, or a grandchild that
//! inherited its stdout, keeps a thread alive after the service itself is gone. Stopping a service
//! closes that gap by killing its whole group; a service that *crashed* has had no such stop, which
//! is why [`Capture::finish`] waits with a deadline rather than joining the threads outright.

use std::collections::VecDeque;
use std::io::{self, BufRead, BufReader, Read};
use std::sync::mpsc::{self, RecvTimeoutError};
use std::sync::{Arc, Mutex};
use std::time::{Duration, SystemTime};

use mixengine_platform::process::Supervised;
use mixengine_proto::{LogPolicy, ServiceId, Timestamp};
use tokio::sync::broadcast;

/// The longest run of bytes that becomes one line.
///
/// A service that prints a megabyte without a newline — a stack trace from a runtime with no line
/// discipline, a binary blob written to the wrong descriptor — must not be able to hold the whole
/// ring in one entry, and must not be able to make this thread allocate without bound while it is
/// still arriving. Past this, the run is emitted as a line and the rest continues in the next one:
/// the output is preserved, the framing is not.
const MAX_LINE_BYTES: usize = 8 * 1024;

/// How many lines a subscriber may fall behind before it starts missing them.
///
/// Only ever reached by a subscriber that has stopped reading; the ready check reads in a loop and
/// the ring is written by this thread. Falling behind is reported to the subscriber as a gap rather
/// than as an error — see [`Capture::subscribe`].
const BACKLOG: usize = 256;

/// Which of a service's two streams a line came from.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Stream {
    /// Standard output.
    Stdout,
    /// Standard error, which is where most services put everything.
    Stderr,
}

impl Stream {
    /// The tag this stream is written with, in a file and in an event.
    #[must_use]
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Stdout => "stdout",
            Self::Stderr => "stderr",
        }
    }
}

impl std::fmt::Display for Stream {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.as_str())
    }
}

/// One line a service printed.
///
/// The text has no trailing newline and no trailing `\r`: a Windows service writing CRLF and a Unix
/// one writing LF have to produce the same line, or every pattern a user writes needs two versions.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LogLine {
    /// Which stream it arrived on.
    pub stream: Stream,

    /// When this process read it, which is not quite when the service wrote it — a pipe holds
    /// tens of kilobytes, so a service that blocked on a full pipe has its backlog timestamped as it
    /// drains. Close enough to order lines by and honest about what it measures.
    pub at: Timestamp,

    /// The line itself, lossily decoded: a service that writes a stray byte gets a replacement
    /// character rather than having the line dropped.
    pub text: String,
}

/// The recent output of one service, and a subscription to the rest of it.
///
/// Created by [`Capture::start`] and owned by whatever owns the [`Supervised`] it was made from.
/// Dropping it does not stop the reader threads — they end at end of file — but it does drop the
/// ring, so the lines are gone.
#[derive(Debug)]
pub struct Capture {
    /// The last `ring_lines` lines, oldest first.
    ring: Arc<Mutex<VecDeque<LogLine>>>,

    /// Every line, as it arrives, for anything watching rather than looking back.
    lines: broadcast::Sender<LogLine>,

    /// Goes quiet when every reader thread has ended, and is how [`finish`](Self::finish) waits for
    /// that without being able to hang on it.
    ///
    /// **A receiver rather than the `JoinHandle`s**, because `JoinHandle::join` has no deadline and
    /// this wait needs one — see [`finish`](Self::finish) for the process that can hold a pipe open
    /// forever. Each thread owns a clone of the matching sender and drops it on the way out; nothing
    /// is ever sent, so the only thing this can report is that the last of them has gone.
    ///
    /// `None` once it has been waited for, and for a [`detached`](Self::detached) capture, which has
    /// no threads to wait for at all.
    ///
    /// **The `Mutex` is for `Sync` and nothing else.** A `Receiver` is `Send` but not `Sync`, and a
    /// `Capture` that is not `Sync` cannot be held by reference across an `await` in a `Send`
    /// future — which is exactly what `ready::wait(&Capture)` does inside the daemon. The lock is
    /// never contended, because the only thing that takes it holds `&mut self`.
    done: Option<Mutex<mpsc::Receiver<Never>>>,
}

/// The message no reader thread ever sends.
///
/// [`Capture::done`] carries no data — the only event it reports is its own senders being dropped —
/// and an uninhabited type is how that is said once rather than repeated in a comment at each end.
#[derive(Debug)]
enum Never {}

impl Capture {
    /// Start reading both of `service`'s streams.
    ///
    /// **Takes the streams**, so this may be called once per `Supervised` and nothing else may read
    /// them afterwards. That is the point: a pipe holds tens of kilobytes and then the service
    /// blocks on its next line, which looks exactly like a service that has hung, so the obligation
    /// `spawn_supervised` documents is discharged here for every service the supervisor starts.
    ///
    /// `service` names the service in the tracing context of the reader threads, so a decoding or
    /// I/O failure inside one is attributable without a line of its own to look at.
    ///
    /// A stream whose reader thread could not be started is **not** captured, and the service runs
    /// on without it. That is the lesser of the two failures: a machine out of threads is one the
    /// daemon has to keep supervising, and the alternative — a panic in the middle of a start — would
    /// take down the supervisor of every other service to report that one of them lost its log. What
    /// it costs is stated at the point it happens, in the one `tracing::error!` below.
    pub fn start(supervised: &mut Supervised, service: &ServiceId, policy: LogPolicy) -> Self {
        let ring = Arc::new(Mutex::new(VecDeque::with_capacity(usize::from(
            policy.ring_lines,
        ))));
        let (lines, _) = broadcast::channel(BACKLOG);
        let (ended, done) = mpsc::channel();

        let streams = [
            (Stream::Stdout, supervised.take_stdout().map(Source::Stdout)),
            (Stream::Stderr, supervised.take_stderr().map(Source::Stderr)),
        ];

        for (stream, source) in streams {
            let Some(source) = source else {
                continue;
            };

            let sink = Sink {
                ring: Arc::clone(&ring),
                lines: lines.clone(),
                keep: usize::from(policy.ring_lines),
                stream,
            };
            let named = service.clone();
            // Moved into the thread and dropped when it returns; that drop is the whole signal.
            let ended = ended.clone();

            let started = std::thread::Builder::new()
                .name(format!("logs {service} {stream}"))
                .spawn(move || {
                    pump(source, &sink, &named);
                    drop(ended);
                });

            if let Err(error) = started {
                tracing::error!(
                    service = service.as_str(),
                    stream = stream.as_str(),
                    %error,
                    // The read end goes with the closure, so this is not a stream that silently
                    // fills: the service's next write to it fails outright. Said plainly, because
                    // the log line explaining a service that died is the one worth having.
                    "cannot start a thread to read a service's output; this stream is unobserved \
                     and the service's next write to it will fail"
                );
            }
        }

        // The last sender this side holds. Dropped here, so `done` is disconnected exactly when the
        // threads are finished rather than never.
        drop(ended);

        Self {
            ring,
            lines,
            done: Some(Mutex::new(done)),
        }
    }

    /// A capture with nothing attached to it, holding the default ring.
    ///
    /// For a `Supervised` whose streams somebody else already took, and for the tests of everything
    /// that *reads* a capture — a ready check waiting for a pattern needs lines, not a process.
    /// [`finish`](Self::finish) returns at once and the ring fills only through what is put in it.
    #[must_use]
    pub fn detached() -> Self {
        Self {
            ring: Arc::new(Mutex::new(VecDeque::new())),
            lines: broadcast::channel(BACKLOG).0,
            done: None,
        }
    }

    /// Put a line in as though a service had printed it on stdout.
    ///
    /// Test-only, and deliberately so: the supervisor's own lines are `tracing` output and belong in
    /// `daemon.log`, not in a service's. What this exists for is the tests of the readers — a ready
    /// check has to be provable without a process behind it. Sized by the default policy, which is
    /// the one [`detached`](Self::detached) builds.
    #[cfg(test)]
    pub(crate) fn record(&self, text: impl Into<String>) {
        Sink {
            ring: Arc::clone(&self.ring),
            lines: self.lines.clone(),
            keep: usize::from(LogPolicy::default().ring_lines),
            stream: Stream::Stdout,
        }
        .accept(text.into());
    }

    /// The last `at_most` lines, oldest first.
    ///
    /// What a crash-loop cutoff attaches to the failure it reports, and what the GUI's log panel
    /// answers from. Fewer than asked for is the ordinary answer for a service that has just
    /// started.
    #[must_use]
    pub fn recent(&self, at_most: usize) -> Vec<LogLine> {
        let ring = lock(&self.ring);
        let from = ring.len().saturating_sub(at_most);

        ring.iter().skip(from).cloned().collect()
    }

    /// Every line from now on.
    ///
    /// **A subscriber that stops reading misses lines rather than blocking the service.** The
    /// channel holds a few hundred of them, and a receiver that falls behind gets
    /// `RecvError::Lagged(n)` and then carries on from what is still buffered — which for a ready
    /// check means "keep waiting", not "give up": the pattern may have been in the lines that were
    /// skipped, and the timeout is what ends the wait either way. Dropping lines is the right
    /// failure here, because the alternative is a full channel stalling the thread that drains the
    /// service's pipe, and a stalled reader stops the service itself.
    #[must_use]
    pub fn subscribe(&self) -> broadcast::Receiver<LogLine> {
        self.lines.subscribe()
    }

    /// Wait up to `within` for both streams to reach end of file. `true` if they did.
    ///
    /// For a caller that has stopped the service and wants everything it said on the way out before
    /// it reports why it stopped. For a group that really has been stopped this returns at once.
    ///
    /// # Why it takes a deadline
    ///
    /// **End of file is not the service exiting — it is the last process holding the write end
    /// exiting.** A crashed service is precisely the case where those differ: nobody killed its
    /// group, so a worker it forked, or a grandchild that inherited its stdout, still holds the pipe
    /// open and this wait would never end. That is also the moment a crash-loop cutoff wants the
    /// tail, so an unbounded wait here would hang the supervisor at the one point it has something
    /// to report. Waiting is worth a moment and never worth the daemon.
    ///
    /// A wait that runs out costs the last few lines and nothing else: the threads are left to end
    /// on their own at end of file, and everything they had already read is in the ring. `false` is
    /// worth logging and is not worth failing a stop over.
    ///
    /// # Blocking
    ///
    /// This blocks the calling thread for up to `within`. A caller inside the async runtime goes
    /// through `spawn_blocking`, as `.claude/standards/rust.md` requires of anything that waits.
    ///
    /// Takes `&mut self` rather than `self`, because reading the ring is the whole reason to wait
    /// for the last lines: a crash-loop cutoff finishes the capture and *then* asks for the last two
    /// hundred lines. Calling it twice is harmless — the second call has nothing left to wait for
    /// and answers `true`.
    pub fn finish(&mut self, within: Duration) -> bool {
        let Some(done) = self.done.take() else {
            return true;
        };
        // Owned, so the lock this unwraps is one nothing else can be holding.
        let done = done
            .into_inner()
            .unwrap_or_else(std::sync::PoisonError::into_inner);

        match done.recv_timeout(within) {
            // Every sender has been dropped, so every reader thread has returned. Nothing is ever
            // sent on this channel, so there is no other way for it to disconnect.
            Err(RecvTimeoutError::Disconnected) => true,
            Err(RecvTimeoutError::Timeout) => {
                // Put back, so a caller that wants to wait again — a stop that killed the group
                // after a first wait ran out — can.
                self.done = Some(Mutex::new(done));

                false
            }
            Ok(never) => match never {},
        }
    }
}

/// One of the two streams, kept as an enum so the reader thread is one function rather than two.
#[derive(Debug)]
enum Source {
    Stdout(std::process::ChildStdout),
    Stderr(std::process::ChildStderr),
}

impl Read for Source {
    fn read(&mut self, buffer: &mut [u8]) -> io::Result<usize> {
        match self {
            Self::Stdout(stream) => stream.read(buffer),
            Self::Stderr(stream) => stream.read(buffer),
        }
    }
}

/// Where a reader thread puts what it read.
#[derive(Debug)]
struct Sink {
    ring: Arc<Mutex<VecDeque<LogLine>>>,
    lines: broadcast::Sender<LogLine>,
    keep: usize,
    stream: Stream,
}

impl Sink {
    /// Record one line, evicting the oldest if the ring is full.
    fn accept(&self, text: String) {
        let line = LogLine {
            stream: self.stream,
            at: Timestamp::from_system_time(SystemTime::now()),
            text,
        };

        {
            let mut ring = lock(&self.ring);

            // Checked before the push rather than after, so the ring never holds `keep + 1` lines
            // even briefly — and a policy of zero keeps nothing at all rather than one.
            while ring.len() >= self.keep {
                if ring.pop_front().is_none() {
                    break;
                }
            }

            if self.keep > 0 {
                ring.push_back(line.clone());
            }
        }

        // Fails only when nothing is subscribed, which is the ordinary case for a service nobody is
        // watching.
        let _ = self.lines.send(line);
    }
}

/// Read one stream to its end, one line at a time.
fn pump(source: Source, sink: &Sink, service: &ServiceId) {
    let mut lines = Lines::over(BufReader::new(source));
    let mut line = Vec::new();

    loop {
        match lines.next(&mut line) {
            Ok(Framing::End) => return,
            Ok(Framing::Line) => sink.accept(decode(&line)),
            Err(error) => {
                // Not swallowed and not fatal to anything else: the service goes on running and is
                // still supervised, it is only unobserved from here on. A pipe that fails to read is
                // rare enough that the one line it produces is worth having.
                tracing::warn!(
                    service = service.as_str(),
                    stream = sink.stream.as_str(),
                    %error,
                    "stopped reading a service's output"
                );
                return;
            }
        }
    }
}

/// What one read produced.
enum Framing {
    /// A line, with its terminator already consumed and not included.
    Line,
    /// End of file, with nothing left over.
    End,
}

/// One stream, read a line at a time with a ceiling on how long a line may be.
///
/// `BufRead::read_until` would be this without the ceiling, and the ceiling is the point: a service
/// that never writes a newline would otherwise grow one buffer until the machine ran out of memory,
/// with nothing recorded from it at all.
struct Lines<R> {
    reader: R,

    /// The last line ended at the cap rather than at a terminator.
    ///
    /// **The state exists for one byte.** If the next thing on the stream is the newline that would
    /// have ended that line, it belongs to it and not to what follows — otherwise a service printing
    /// exactly [`MAX_LINE_BYTES`] and a newline produces the line it wrote *and an empty one behind
    /// it*, which it never wrote. It cannot be handled while reading the long line, because at that
    /// point the newline has not arrived yet: a pipe hands over what it has, and a full buffer is
    /// exactly the case where the next byte is still in flight.
    cut: bool,
}

impl<R: BufRead> Lines<R> {
    fn over(reader: R) -> Self {
        Self { reader, cut: false }
    }

    /// Read the next line into `line`, replacing what was there.
    ///
    /// A final fragment with no newline is a line. A service killed mid-sentence said something, and
    /// dropping it would lose the last thing printed before a crash — which is the line the user
    /// most wants to see.
    fn next(&mut self, line: &mut Vec<u8>) -> io::Result<Framing> {
        line.clear();

        loop {
            let available = loop {
                match self.reader.fill_buf() {
                    Ok(bytes) => break bytes,
                    // A signal arrived mid-read; nothing was consumed, so asking again is the whole
                    // fix.
                    Err(error) if error.kind() == io::ErrorKind::Interrupted => {}
                    Err(error) => return Err(error),
                }
            };

            if available.is_empty() {
                self.cut = false;

                return Ok(if line.is_empty() {
                    Framing::End
                } else {
                    Framing::Line
                });
            }

            if self.cut {
                self.cut = false;

                if available[0] == b'\n' {
                    self.reader.consume(1);
                    continue;
                }
            }

            let room = MAX_LINE_BYTES - line.len();

            if let Some(end) = available.iter().position(|&byte| byte == b'\n')
                && end <= room
            {
                line.extend_from_slice(&available[..end]);
                self.reader.consume(end + 1);

                return Ok(Framing::Line);
            }

            let taken = available.len().min(room);
            line.extend_from_slice(&available[..taken]);
            self.reader.consume(taken);

            if line.len() == MAX_LINE_BYTES {
                self.cut = true;

                return Ok(Framing::Line);
            }
        }
    }
}

/// Turn the bytes of one line into text somebody can read.
///
/// Lossy rather than fallible: a service that writes one byte of Latin-1 into an otherwise UTF-8 log
/// should cost that byte, not the line. The trailing `\r` goes because a service built for Windows
/// writes CRLF and one built for Unix does not, and a pattern in a spec should not have to know
/// which.
fn decode(line: &[u8]) -> String {
    let text = String::from_utf8_lossy(line);

    match text.strip_suffix('\r') {
        Some(without) => without.to_owned(),
        None => text.into_owned(),
    }
}

/// A poisoned ring means a reader thread panicked while holding it; the lines that are in there are
/// still the lines the service printed, so this takes them and carries on rather than spreading the
/// panic to a supervisor that has a service to stop.
fn lock<T>(mutex: &Mutex<T>) -> std::sync::MutexGuard<'_, T> {
    mutex.lock().unwrap_or_else(|poisoned| {
        mutex.clear_poison();
        poisoned.into_inner()
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A `Capture` is shared by reference across tasks and must stay `Send + Sync`.
    ///
    /// Stated here rather than left to be discovered, because the field that carries the reader
    /// threads' completion is the kind that quietly is not: `mpsc::Receiver` is `Send` and not
    /// `Sync`, and the failure it produces is a `ready::wait` future that is no longer `Send` —
    /// reported at whichever `tokio::spawn` happens to be nearest, which is nowhere near the field
    /// that caused it.
    const _: fn() = || {
        fn assert<T: Send + Sync>() {}

        assert::<Capture>();
    };

    /// Frame `input` the way a reader thread does.
    ///
    /// Through a `BufReader` with a small buffer on purpose: a pipe hands over whatever has arrived,
    /// so a line almost never appears in one piece, and a framing test that fed the whole input at
    /// once would only exercise the easy path. Sixty-four bytes is small enough that every case here
    /// crosses a buffer boundary.
    fn framed(input: &[u8]) -> Vec<String> {
        let mut lines = Lines::over(BufReader::with_capacity(64, input));
        let mut line = Vec::new();
        let mut framed = Vec::new();

        loop {
            match lines
                .next(&mut line)
                .expect("a slice cannot fail to be read")
            {
                Framing::End => return framed,
                Framing::Line => framed.push(decode(&line)),
            }
        }
    }

    #[test]
    fn output_is_split_on_newlines_and_keeps_neither_terminator() {
        assert_eq!(framed(b"one\ntwo\n"), ["one", "two"]);
        assert_eq!(framed(b"one\r\ntwo\r\n"), ["one", "two"]);
    }

    /// The last thing a service prints before it crashes usually has no newline after it.
    #[test]
    fn a_final_fragment_with_no_newline_is_still_a_line() {
        assert_eq!(framed(b"finished\nsegfault"), ["finished", "segfault"]);
    }

    #[test]
    fn an_empty_line_is_a_line_and_end_of_file_is_not() {
        assert_eq!(framed(b"\n\n"), ["", ""]);
        assert_eq!(framed(b""), [] as [String; 0]);
    }

    /// A `\r` in the middle of a line is data — `\r` only terminates when a `\n` follows it.
    #[test]
    fn a_carriage_return_inside_a_line_survives() {
        assert_eq!(framed(b"a\rb\n"), ["a\rb"]);
    }

    #[test]
    fn a_line_that_never_ends_is_cut_rather_than_grown_forever() {
        let long = vec![b'x'; MAX_LINE_BYTES * 2 + 3];

        let lines = framed(&long);

        assert_eq!(lines.len(), 3, "a run twice the cap is three lines");
        assert_eq!(lines[0].len(), MAX_LINE_BYTES);
        assert_eq!(lines[1].len(), MAX_LINE_BYTES);
        assert_eq!(lines[2].len(), 3, "the remainder is the last of them");
    }

    /// A newline exactly at the cap boundary must not produce an empty line after it.
    #[test]
    fn a_line_that_is_exactly_the_cap_is_one_line() {
        let mut input = vec![b'x'; MAX_LINE_BYTES];
        input.push(b'\n');

        assert_eq!(
            framed(&input),
            [String::from_utf8(vec![b'x'; MAX_LINE_BYTES]).unwrap()]
        );
    }

    /// The other half of the same rule: only a newline is swallowed after a cut, and only one.
    #[test]
    fn a_cut_line_does_not_eat_the_first_byte_of_the_next_one() {
        let mut input = vec![b'x'; MAX_LINE_BYTES];
        input.extend_from_slice(b"tail\n");

        assert_eq!(
            framed(&input),
            [
                String::from_utf8(vec![b'x'; MAX_LINE_BYTES]).unwrap(),
                "tail".to_owned()
            ]
        );
    }

    #[test]
    fn a_byte_that_is_not_utf8_costs_the_byte_and_not_the_line() {
        assert_eq!(framed(b"caf\xe9 open\n"), ["caf\u{fffd} open"]);
    }

    #[test]
    fn the_ring_keeps_the_last_lines_and_no_more() {
        let sink = Sink {
            ring: Arc::new(Mutex::new(VecDeque::new())),
            lines: broadcast::channel(BACKLOG).0,
            keep: 3,
            stream: Stream::Stdout,
        };

        for line in 1..=5 {
            sink.accept(format!("line {line}"));
        }

        let kept: Vec<String> = lock(&sink.ring)
            .iter()
            .map(|line| line.text.clone())
            .collect();

        assert_eq!(kept, ["line 3", "line 4", "line 5"]);
    }

    /// A policy that asks for no ring gets none, rather than one line by accident.
    #[test]
    fn a_ring_of_zero_lines_keeps_nothing() {
        let sink = Sink {
            ring: Arc::new(Mutex::new(VecDeque::new())),
            lines: broadcast::channel(BACKLOG).0,
            keep: 0,
            stream: Stream::Stdout,
        };

        sink.accept("line".to_owned());

        assert!(lock(&sink.ring).is_empty());
    }
}
