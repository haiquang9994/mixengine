//! What arrives on `GET /events`.
//!
//! The vocabulary in `.claude/architecture/daemon-and-ipc.md` — `JobProgress`, `LogLine` and the
//! rest — is declared here **one variant at a time, as the code that emits it lands**. Writing them
//! all up front would mean inventing identifier types before the code that issues them has an
//! opinion (`JobId`, `MetricsSample`), and publishing a wire contract nothing can produce.
//! [`DaemonEvent::ServiceStateChanged`] is the first to arrive that way, with T14.
//!
//! What is here is the part of the stream that belongs to the stream itself, and the rule the
//! architecture states plainly: **events are best-effort and must never be the only way state is
//! learned.** A client that reconnects, or that is told to [`DaemonEvent::Resync`], calls the
//! matching `*.list` and rebuilds what it knows.

use crate::ServiceTransition;

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

    /// A service moved: started, became ready, went unhealthy, crashed, gave up.
    ///
    /// Carries the [`ServiceTransition`] that was persisted, unchanged and unrepacked — the daemon
    /// publishes the value `mixengine-core` handed back from the transaction that wrote the row.
    /// A second description of the same event, built beside the first, is a second description that
    /// can be wrong.
    ///
    /// A newtype variant rather than inline fields so that the persisted value and the published
    /// one are the same type and not merely the same shape. Internally tagged, so it still arrives
    /// as one flat object: `{"type":"service_state_changed","service":"caddy",…}`.
    ServiceStateChanged(ServiceTransition),
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The newtype variant has to flatten, or the GUI's one `onmessage` handler would have to
    /// unwrap a nested object for this variant and not for the others.
    #[test]
    fn a_state_change_arrives_as_one_flat_object() {
        let event = DaemonEvent::ServiceStateChanged(ServiceTransition {
            service: crate::ServiceId::parse("caddy").unwrap(),
            from: crate::ServiceState::Running,
            to: crate::ServiceState::Degraded,
            reason: crate::StateReason::Unhealthy,
            at: crate::Timestamp(1_760_000_000_000),
        });

        let encoded = serde_json::to_string(&event).unwrap();
        assert_eq!(
            encoded,
            r#"{"type":"service_state_changed","service":"caddy","from":"running","to":"degraded","reason":{"kind":"unhealthy"},"at":1760000000000}"#
        );
        assert_eq!(
            serde_json::from_str::<DaemonEvent>(&encoded).unwrap(),
            event
        );
    }

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
