//! One line a service printed.
//!
//! Here rather than in `mixengine-supervisor`, for the reason
//! `.claude/decisions/0006-servicespec-in-proto-and-secret-free.md` gives for `ServiceSpec`: proto
//! owns the vocabulary, and a line that is captured, kept in a ring, written to a file and published
//! on `GET /events` is one value rather than four descriptions of one. T14 set the same precedent
//! for [`ServiceTransition`](crate::ServiceTransition) — the row that is persisted and the event
//! that is emitted are the same value, so an event describing something that did not happen cannot
//! be built.
//!
//! What is *not* here is the service the line came from. A capture belongs to one service, and
//! repeating its id on every line would spend a field per line on an answer the caller already had;
//! the id joins the line where it stops being implied — in the event and in the endpoint (T16b).

use crate::Timestamp;

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
