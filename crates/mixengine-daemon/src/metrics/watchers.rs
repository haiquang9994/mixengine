//! How many clients are watching, and how the loop hears about it — roadmap task **T71**.
//!
//! **The count of watchers is the count of open `GET /metrics` connections, and nothing else.** That
//! is what makes "sampled only while watched" a property of the mechanism rather than of anyone's
//! care: a client that dies without saying goodbye still closes its socket, and a socket closing is
//! the unsubscribe. An explicit `metrics.subscribe`/`metrics.unsubscribe` pair would leave a client
//! that crashed sampling this machine every second for as long as the daemon ran.
//!
//! **A `watch` channel rather than a flag the loop reads each tick.** A client opening the stream
//! must get its first frame at once; a flag consulted at the top of an iteration would make it wait
//! out the rest of a sixty-second sleep first, and a client closing the last stream would leave the
//! machine on the fast rate for the same length of time.

use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};

use tokio::sync::watch;

/// Who is watching, shared by the API and the loop.
#[derive(Debug, Clone)]
pub(crate) struct Watchers {
    inner: Arc<Inner>,
}

/// The shared half. Separate from [`Watchers`] so that a clone is another handle onto the same
/// count rather than a second count.
#[derive(Debug)]
struct Inner {
    /// How many streams are open.
    open: AtomicUsize,

    /// Raised whenever that count changes, so the loop can stop sleeping.
    signal: watch::Sender<usize>,
}

/// One open stream. **Dropping it is the unsubscribe.**
#[derive(Debug)]
pub(crate) struct Watch {
    inner: Arc<Inner>,
}

impl Watchers {
    /// Nobody watching yet.
    pub(crate) fn new() -> Self {
        Self {
            inner: Arc::new(Inner {
                open: AtomicUsize::new(0),
                signal: watch::Sender::new(0),
            }),
        }
    }

    /// Register one open stream.
    pub(crate) fn watch(&self) -> Watch {
        let open = self.inner.open.fetch_add(1, Ordering::SeqCst) + 1;

        // Nobody listening is the ordinary state — the loop subscribes only while it sleeps.
        let _ = self.inner.signal.send(open);

        Watch {
            inner: Arc::clone(&self.inner),
        }
    }

    /// Whether the fast rate is what this daemon owes right now.
    pub(crate) fn fast(&self) -> bool {
        self.inner.open.load(Ordering::SeqCst) > 0
    }

    /// A handle the loop keeps, which resolves whenever the count changes.
    ///
    /// **Taken once and held, never made fresh per wait.** A receiver created at the moment of
    /// waiting treats the count as it stands as already seen, so a watcher that arrived between the
    /// last tick and that moment would be missed — and the client that opened the stream would wait
    /// out a whole sixty-second sleep for its first frame, which is the one thing the signal exists
    /// to prevent.
    pub(crate) fn signal(&self) -> Signal {
        Signal {
            receiver: self.inner.signal.subscribe(),
        }
    }
}

/// The loop's end of the signal.
#[derive(Debug)]
pub(crate) struct Signal {
    receiver: watch::Receiver<usize>,
}

impl Signal {
    /// Wait until the number of watchers changes.
    ///
    /// Cancel safe, which is what lets the loop `select!` this against a sleep: a change that
    /// arrives while the other branch wins is still unseen at the next call.
    pub(crate) async fn changed(&mut self) {
        // The sender outlives every receiver — [`Watchers`] holds it — so this only resolves on a
        // real change.
        let _ = self.receiver.changed().await;
    }
}

impl Drop for Watch {
    fn drop(&mut self) {
        let open = self
            .inner
            .open
            .fetch_sub(1, Ordering::SeqCst)
            .saturating_sub(1);
        let _ = self.inner.signal.send(open);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_daemon_nobody_is_watching_samples_slowly() {
        assert!(!Watchers::new().fast());
    }

    #[test]
    fn one_open_stream_is_enough_to_go_fast() {
        let watchers = Watchers::new();
        let _watch = watchers.watch();

        assert!(watchers.fast());
    }

    #[test]
    fn the_last_stream_closing_puts_it_back() {
        let watchers = Watchers::new();

        {
            let _first = watchers.watch();
            let _second = watchers.watch();
            assert!(watchers.fast());
        }

        assert!(
            !watchers.fast(),
            "a client that dies without saying goodbye still closes its socket, and that is the \
             unsubscribe"
        );
    }

    #[tokio::test]
    async fn a_new_watcher_wakes_the_loop_rather_than_waiting_out_its_sleep() {
        let watchers = Watchers::new();

        // Held before the watcher arrives, exactly as the loop holds it: this is the property being
        // asserted, not a convenience of the test.
        let mut signal = watchers.signal();
        let _watch = watchers.watch();

        tokio::time::timeout(std::time::Duration::from_secs(5), signal.changed())
            .await
            .expect("the loop is woken rather than left to time out");

        assert!(watchers.fast());
    }

    #[tokio::test]
    async fn a_watcher_that_arrived_while_the_loop_was_busy_is_not_missed() {
        // The race the persistent receiver exists for: the count changes twice before anything
        // waits, and the wait must still resolve rather than block until a third change.
        let watchers = Watchers::new();
        let mut signal = watchers.signal();

        let watch = watchers.watch();
        drop(watch);

        tokio::time::timeout(std::time::Duration::from_secs(5), signal.changed())
            .await
            .expect("a change that happened before the wait is still a change to be told about");
    }
}
