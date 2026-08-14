//! `GET /events` — the daemon telling clients what changed, as Server-Sent Events.
//!
//! **Best-effort, and the architecture says so in as many words:** a client that reconnects, or one
//! that is handed a [`DaemonEvent::Resync`], calls the matching `*.list` and rebuilds what it knows.
//! Nothing here is a queue and nothing here is durable — an event missed while the GUI was closed is
//! missed for good, which is fine precisely because it is never the only way state is learned.
//!
//! **A slow consumer is dropped, not buffered.** The channel is bounded, and a receiver that falls
//! behind it loses the messages it did not read. That is a design decision and not a limitation: the
//! alternative is a client that stops reading and takes the daemon's memory with it. What such a
//! receiver gets instead is a `Resync` naming how many it missed, which is more useful than the
//! events themselves would have been — by then its picture is stale in ways no replay would fix.

use std::time::Duration;

use mixengine_proto::DaemonEvent;
use tokio::sync::broadcast;

/// How many events a receiver may fall behind by before it is told to resync.
///
/// The 1024 from `.claude/architecture/daemon-and-ipc.md`. Large enough that an ordinary burst — a
/// blueprint applying, a dozen services coming up at once — never trips it, small enough that a
/// client which has stopped reading costs a bounded amount of memory.
const CAPACITY: usize = 1024;

/// How long the stream may stay silent before it says something anyway.
///
/// A comment frame, not an event: SSE reserves lines beginning with `:` for exactly this, and every
/// client discards them. It exists because a stream that sends nothing is indistinguishable from one
/// whose connection died halfway, and neither end finds out until something is finally published.
/// Fifteen seconds is well inside the idle timeouts that intermediaries impose — there are none on a
/// local socket, but a client's own read timeout is real.
pub(super) const HEARTBEAT: Duration = Duration::from_secs(15);

/// The publishing half: what the rest of the daemon holds.
///
/// One stream per daemon, and a clone is another handle onto **the same** stream rather than a
/// second one — which is what T19's registry needs: a runner outlives every request and cannot
/// borrow from the [`Api`](super::Api) that serves one. The last handle dropping ends every
/// subscription, which is what shutdown wants and what [`Subscription::next`] reports as the end of
/// the stream.
///
/// Holding no receiver of its own is deliberate: a daemon nobody is watching should cost nothing,
/// and [`Events::publish`] is a no-op there because it ignores "no receivers", not because someone
/// is pretending to listen.
#[derive(Debug, Clone)]
pub(crate) struct Events {
    sender: broadcast::Sender<DaemonEvent>,
}

impl Events {
    /// A stream with nobody listening yet.
    pub(crate) fn new() -> Self {
        Self {
            sender: broadcast::Sender::new(CAPACITY),
        }
    }

    /// Tell every connected client something happened.
    ///
    /// Never fails and never blocks. `broadcast::Sender::send` reports "no receivers", which is the
    /// normal state of a daemon nobody has open — it is not a failure and there is nothing to log
    /// about it.
    pub(crate) fn publish(&self, event: DaemonEvent) {
        let _ = self.sender.send(event);
    }

    /// One client's view of the stream, from now on.
    ///
    /// Events published before this call are not delivered, which is the whole best-effort
    /// contract: the client's next move is a `*.list`, and an event from before it asked would
    /// describe a change already reflected in the answer.
    pub(crate) fn subscribe(&self) -> Subscription {
        Subscription {
            receiver: self.sender.subscribe(),
        }
    }
}

/// One connected client's receiver.
#[derive(Debug)]
pub(crate) struct Subscription {
    receiver: broadcast::Receiver<DaemonEvent>,
}

impl Subscription {
    /// The next frame to write, or `None` once the daemon is shutting down.
    ///
    /// Lag is turned into a [`DaemonEvent::Resync`] here rather than being reported to the caller,
    /// because there is exactly one right response to it and it is the same every time. The frame
    /// after a resync is the next real event — `recv` resets the receiver to the oldest message it
    /// still holds, so nothing is skipped twice.
    pub(crate) async fn next(&mut self) -> Option<Frame> {
        match self.receiver.recv().await {
            Ok(event) => Some(Frame::Event(event)),

            Err(broadcast::error::RecvError::Lagged(missed)) => {
                tracing::warn!(
                    missed,
                    "an event subscriber fell behind — telling it to resync"
                );
                Some(Frame::Event(DaemonEvent::Resync { missed }))
            }

            // Every sender is gone, which can only mean the daemon is on its way out.
            Err(broadcast::error::RecvError::Closed) => None,
        }
    }

    /// The next frame, or a heartbeat if nothing arrives in time.
    ///
    /// The timeout is what keeps a silent stream from being indistinguishable from a dead one. It
    /// is safe to restart on every call because [`Subscription::next`] is cancel safe:
    /// `broadcast::Receiver::recv` either takes a message or leaves it for the next call, so a
    /// heartbeat never consumes an event.
    pub(crate) async fn next_or_heartbeat(&mut self) -> Option<Frame> {
        match tokio::time::timeout(HEARTBEAT, self.next()).await {
            Ok(frame) => frame,
            Err(_elapsed) => Some(Frame::Heartbeat),
        }
    }
}

/// One chunk written to a client.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum Frame {
    /// A real event.
    Event(DaemonEvent),

    /// A comment, so that both ends learn about a broken connection while it is still idle.
    Heartbeat,
}

impl Frame {
    /// The bytes of this frame, terminated the way SSE terminates one.
    ///
    /// No `event:` line: the discriminator is inside the JSON object (see
    /// [`DaemonEvent`]), which gives the GUI one `onmessage` handler instead of one
    /// `addEventListener` per variant — and means a variant added in a later phase reaches an older
    /// client as an object it can ignore rather than as an event type it never subscribed to.
    ///
    /// An event that cannot be serialised is written as a heartbeat rather than as a broken frame:
    /// it is unreachable — every variant is a plain `mixengine-proto` type — and a truncated `data:`
    /// line would desynchronise the parser at the other end for the rest of the connection.
    pub(crate) fn encode(&self) -> Vec<u8> {
        match self {
            Self::Event(event) => match serde_json::to_vec(event) {
                Ok(json) => {
                    let mut frame = Vec::with_capacity(json.len() + 8);
                    frame.extend_from_slice(b"data: ");
                    frame.extend_from_slice(&json);
                    frame.extend_from_slice(b"\n\n");
                    frame
                }

                Err(error) => {
                    tracing::error!(%error, "an event could not be encoded and was dropped");
                    Self::Heartbeat.encode()
                }
            },

            Self::Heartbeat => b": keep-alive\n\n".to_vec(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn an_event_is_one_data_line_carrying_its_own_type() {
        let frame = Frame::Event(DaemonEvent::Resync { missed: 3 }).encode();

        assert_eq!(
            String::from_utf8(frame).unwrap(),
            "data: {\"type\":\"resync\",\"missed\":3}\n\n"
        );
    }

    #[test]
    fn a_heartbeat_is_a_comment_every_client_discards() {
        assert_eq!(
            String::from_utf8(Frame::Heartbeat.encode()).unwrap(),
            ": keep-alive\n\n"
        );
    }

    #[tokio::test]
    async fn a_subscriber_receives_what_is_published_after_it_subscribed() {
        let events = Events::new();
        let mut subscription = events.subscribe();

        events.publish(DaemonEvent::Resync { missed: 1 });

        assert_eq!(
            subscription.next().await,
            Some(Frame::Event(DaemonEvent::Resync { missed: 1 }))
        );
    }

    #[tokio::test]
    async fn publishing_with_nobody_listening_is_not_a_failure() {
        // The normal state of a daemon with no GUI open. If this were an error it would be one
        // every caller had to ignore, at every call site.
        Events::new().publish(DaemonEvent::Resync { missed: 0 });
    }

    #[tokio::test]
    async fn a_subscriber_that_falls_behind_is_told_to_resync_rather_than_buffered() {
        let events = Events::new();
        let mut subscription = events.subscribe();

        // One more than the channel can hold, so the oldest is dropped before anything is read.
        for missed in 0..=CAPACITY {
            events.publish(DaemonEvent::Resync {
                missed: missed as u64,
            });
        }

        let frame = subscription.next().await.expect("the stream is still open");
        assert!(
            matches!(frame, Frame::Event(DaemonEvent::Resync { missed }) if missed > 0),
            "a lagging receiver is told how much it lost: {frame:?}"
        );

        // And the stream carries on from the oldest message still held, rather than ending.
        assert!(subscription.next().await.is_some());
    }

    #[tokio::test]
    async fn the_stream_ends_when_the_daemon_does() {
        let events = Events::new();
        let mut subscription = events.subscribe();

        drop(events);

        assert_eq!(subscription.next().await, None);
    }

    #[tokio::test(start_paused = true)]
    async fn a_silent_stream_sends_a_heartbeat_rather_than_nothing() {
        let events = Events::new();
        let mut subscription = events.subscribe();

        // Tokio's paused clock: no real time passes, but the timeout fires exactly as it would
        // after fifteen seconds of silence.
        assert_eq!(
            subscription.next_or_heartbeat().await,
            Some(Frame::Heartbeat)
        );

        // A heartbeat consumes nothing — the event published now is still delivered.
        events.publish(DaemonEvent::Resync { missed: 7 });
        assert_eq!(
            subscription.next_or_heartbeat().await,
            Some(Frame::Event(DaemonEvent::Resync { missed: 7 }))
        );
    }
}
