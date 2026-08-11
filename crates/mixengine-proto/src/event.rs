//! What arrives on `GET /events`.
//!
//! The vocabulary in `.claude/architecture/daemon-and-ipc.md` — `ServiceStateChanged`,
//! `JobProgress`, `LogLine` and the rest — is deliberately **not** declared here yet. Every one of
//! those variants names a type that does not exist (`ServiceId`, `JobId`, `MetricsSample`), so
//! writing them now would mean inventing identifier types before the code that issues them has an
//! opinion, and publishing a wire contract nothing can produce. Each variant arrives with the phase
//! that first emits it.
//!
//! What is here is the part of the stream that belongs to the stream itself, and the rule the
//! architecture states plainly: **events are best-effort and must never be the only way state is
//! learned.** A client that reconnects, or that is told to [`DaemonEvent::Resync`], calls the
//! matching `*.list` and rebuilds what it knows.

/// One message on the event stream.
///
/// Internally tagged, so the discriminator travels inside the JSON object rather than in the SSE
/// `event:` line. That gives the GUI one `onmessage` handler that switches on `type` instead of one
/// `addEventListener` per variant — and it means a variant added in a later phase reaches an older
/// client as an object it can recognise and ignore, rather than as an event type it never
/// subscribed to and silently never sees.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
#[non_exhaustive]
pub enum DaemonEvent {
    /// This receiver fell behind and messages were dropped for it.
    ///
    /// The stream is bounded and a slow consumer is dropped rather than buffered without limit, so
    /// this is the honest thing to send instead of a gap nobody mentions: whatever the client
    /// believes about the daemon's state may now be wrong, and the way to fix that is to ask again
    /// rather than to wait for the next event.
    Resync {
        /// How many messages this receiver missed. For a log line, not for logic — the answer to a
        /// resync is the same whether one was dropped or a thousand.
        missed: u64,
    },
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn an_event_carries_its_own_discriminator() {
        let event = DaemonEvent::Resync { missed: 12 };

        assert_eq!(
            serde_json::to_string(&event).unwrap(),
            r#"{"type":"resync","missed":12}"#
        );
        assert_eq!(
            serde_json::from_str::<DaemonEvent>(r#"{"type":"resync","missed":12}"#).unwrap(),
            event
        );
    }
}
