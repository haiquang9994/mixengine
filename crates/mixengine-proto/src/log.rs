//! One line a service printed.
//!
//! Here rather than in `mixengine-supervisor`, for the reason
//! `.claude/decisions/0006-servicespec-in-proto-and-secret-free.md` gives for `ServiceSpec`: proto
//! owns the vocabulary, and a line that is captured, kept in a ring, written to a file and served on
//! `GET /logs/{id}` is one value rather than four descriptions of one. T14 set the same precedent
//! for [`ServiceTransition`](crate::ServiceTransition) — the row that is persisted and the event
//! that is emitted are the same value, so an event describing something that did not happen cannot
//! be built.
//!
//! What is *not* here is the service the line came from. A capture belongs to one service, and
//! repeating its id on every line would spend a field per line on an answer the caller already had;
//! the id is in the path of the endpoint that served it.
//!
//! **A line is never a `DaemonEvent`** —
//! `.claude/decisions/0009-logs-travel-on-their-own-stream.md`. What a client reads is a
//! [`LogFrame`], on a connection it opened for one service.

use crate::{JobId, ServiceId, Timestamp};

/// Which of a service's two streams a line came from.
///
/// **Closed, where most of this crate is `non_exhaustive`.** A process has exactly two output
/// streams and that is fixed by the three operating systems rather than by this protocol, so a
/// client is safe to match both arms and be done — the same reasoning that closes
/// [`ServiceState`](crate::ServiceState) while the reasons around it stay open.
#[derive(
    Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord, serde::Serialize, serde::Deserialize,
)]
#[serde(rename_all = "snake_case")]
#[cfg_attr(feature = "ts", derive(ts_rs::TS), ts(export))]
pub enum Stream {
    /// Standard output.
    Stdout,

    /// Standard error, which is where most services put everything.
    Stderr,
}

impl Stream {
    /// The tag this stream travels under, in an event and in a `mix service logs` prefix.
    ///
    /// The same spelling as the wire form, checked by a test rather than trusted.
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
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS), ts(export))]
pub struct LogLine {
    /// Which stream it arrived on.
    pub stream: Stream,

    /// When the supervisor read it, which is not quite when the service wrote it — a pipe holds tens
    /// of kilobytes, so a service that blocked on a full pipe has its backlog timestamped as it
    /// drains. Close enough to order lines by and honest about what it measures.
    pub at: Timestamp,

    /// The line itself, lossily decoded: a service that writes a stray byte gets a replacement
    /// character rather than having the line dropped.
    pub text: String,
}

/// Whose output a log stream carries — roadmap task **T78a**, its design's D13.
///
/// **A second kind of subject rather than a second surface.** The ring, the frames, the [`Gap`] a
/// slow reader is told about and the per-connection back-pressure are the ones
/// [ADR 0009](https://github.com/mixnz/mixengine/blob/master/.claude/decisions/0009-logs-travel-on-their-own-stream.md)
/// argued for a service's output, and a blueprint's `[scaffold]` command needs every one of them for
/// the same reason: how much it prints is decided by somebody else's program.
///
/// The route says which kind it is — `GET /logs/service/{id}` and `GET /logs/job/{id}`, two segments
/// always — so nothing has to decide whether a first segment is a package name or the word `job`.
///
/// [`Gap`]: LogFrame::Gap
#[derive(Debug, Clone, PartialEq, Eq, Hash, serde::Serialize, serde::Deserialize)]
#[serde(tag = "subject", rename_all = "snake_case")]
#[cfg_attr(feature = "ts", derive(ts_rs::TS), ts(export))]
pub enum LogSubject {
    /// One service's output, from before the reader connected and from after.
    Service {
        /// Which service.
        id: ServiceId,
    },

    /// One job's — today only an apply running a blueprint's own command.
    Job {
        /// Which job.
        id: JobId,
    },
}

impl std::fmt::Display for LogSubject {
    /// What the route holding it looks like, which is also what a message names.
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Service { id } => write!(f, "service/{id}"),
            // The number rather than `JobId`'s own `#7`, because this is a path segment and a
            // route with a `#` in it is a fragment to whatever parses it next.
            Self::Job { id } => write!(f, "job/{}", id.0),
        }
    }
}

/// One message on `GET /logs/service/{id}` or `GET /logs/job/{id}`.
///
/// Two variants, because a stream of lines has one thing to say that is not a line: that some were
/// lost. A client that fell behind the service's own output is told how many rather than handed a
/// gap nobody mentions — the same honesty [`DaemonEvent::Resync`](crate::DaemonEvent::Resync) offers
/// on the event stream, and for the same reason: the alternative is a log panel that silently shows
/// output with a hole in it, which is worse than one that says where the hole is.
///
/// Internally tagged, like every other framed type here, so a client has one handler and a variant
/// added later arrives as an object it can ignore.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
#[non_exhaustive]
#[cfg_attr(feature = "ts", derive(ts_rs::TS), ts(export))]
pub enum LogFrame {
    /// Something the service printed.
    ///
    /// A newtype variant so that the value served is the value captured rather than a second shape
    /// built beside it; it still arrives flat, as `{"type":"line","stream":…,"at":…,"text":…}`.
    Line(LogLine),

    /// A line recovered from `current.log`, of which only the text survived.
    ///
    /// **The file is the service's own output and carries nothing of MixEngine's** — no timestamp,
    /// no stream tag — because it is read by whoever reads MariaDB's or Caddy's log, with their
    /// tools. So a line read back out of it cannot honestly be a [`LogFrame::Line`]: it is not known
    /// whether it was printed on stdout or stderr, or when. This variant says that rather than
    /// picking a stream and a moment that would look like readings and be guesses.
    ///
    /// Sent only where the daemon has nothing of its own — a service that was running before this
    /// daemon started, or one whose runner has ended and whose ring went with it.
    Historic {
        /// The line, exactly as it is in the file.
        text: String,
    },

    /// This reader fell behind the service and lines were dropped for it.
    ///
    /// **Not a failure of the connection**, and the stream carries on afterwards from what is still
    /// buffered. It means the service printed faster than this client read for a moment, which is
    /// the ordinary consequence of a bounded buffer and the deliberate alternative to letting a slow
    /// reader stall the process it is watching.
    Gap {
        /// How many lines this reader missed.
        missed: u64,
    },
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_line_is_one_flat_object() {
        let line = LogLine {
            stream: Stream::Stderr,
            at: Timestamp(1_760_000_000_000),
            text: "Address already in use".to_owned(),
        };

        let encoded = serde_json::to_string(&line).unwrap();
        assert_eq!(
            encoded,
            r#"{"stream":"stderr","at":1760000000000,"text":"Address already in use"}"#
        );
        assert_eq!(serde_json::from_str::<LogLine>(&encoded).unwrap(), line);
    }

    /// A frame carries its own discriminator, and a line stays flat inside one.
    #[test]
    fn a_frame_says_which_of_the_two_it_is() {
        let line = LogFrame::Line(LogLine {
            stream: Stream::Stdout,
            at: Timestamp(1_760_000_000_000),
            text: "ready".to_owned(),
        });

        let encoded = serde_json::to_string(&line).unwrap();
        assert_eq!(
            encoded,
            r#"{"type":"line","stream":"stdout","at":1760000000000,"text":"ready"}"#
        );
        assert_eq!(serde_json::from_str::<LogFrame>(&encoded).unwrap(), line);

        let gap = LogFrame::Gap { missed: 12 };
        assert_eq!(
            serde_json::to_string(&gap).unwrap(),
            r#"{"type":"gap","missed":12}"#
        );
        assert_eq!(
            serde_json::from_str::<LogFrame>(r#"{"type":"gap","missed":12}"#).unwrap(),
            gap
        );
    }

    /// The tag a file or a CLI prefix is written with is the tag the wire uses, or the GUI would be
    /// filtering on one spelling and `mix service logs` printing another.
    #[test]
    fn the_tag_and_the_wire_form_are_one_spelling() {
        for stream in [Stream::Stdout, Stream::Stderr] {
            assert_eq!(
                serde_json::to_string(&stream).unwrap(),
                format!("\"{}\"", stream.as_str())
            );
            assert_eq!(stream.to_string(), stream.as_str());
        }
    }
}
